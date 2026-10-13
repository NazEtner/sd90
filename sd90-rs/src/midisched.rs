//! デバイスフレーム基準の MIDI 送出。
//!
//! ホストの `Instant` を基準に送ってはいけない。SD-90 の水晶とホストの時計は
//! ppm でずれており、数分で数十 ms たまる。基準にできるのは**取り込み済み
//! フレーム数**だけで、これはデバイスの水晶そのものなので原理的にずれない。
//! ただし更新は 1 転送 = 8ms に 1 回しか来ないので、その間だけホストの時計で
//! 補間する。位相はデバイスクロックで合わせ、間を埋めるのにホストを使う形。
//!
//! (sd90-seq の render.rs で先に確かめた手口。実測ずれは平均 0.1〜1.4ms /
//!  最大 4ms で、最大値が 4ms 付近に張り付くのは 8ms の更新粒度そのもの。)
//!
//! RT スレッド (VST3 の `process`) は 12 バイトのレコードをリングに積むだけで、
//! 実際の待ちと USB 転送はこのモジュールのスレッドが引き受ける。

use crate::device::SAMPLE_RATE;
use crate::iso::CaptureCtl;
use crate::ring::Ring;
use crate::stream::Shared;
use crate::usbmidi::{UsbMidi, for_each_packet};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, OnceLock};
use std::thread::{JoinHandle, Thread};
use std::time::{Duration, Instant};

/// 1 レコード = u64 デバイスフレーム + USB-MIDI の 4 バイトパケット。
const RECORD_BYTES: usize = 12;
/// 同時刻とみなして 1 転送にまとめる幅。1ms。
///
/// 和音の頭では数十イベントがほぼ同じ位置に来る。まとめないと USB の転送回数
/// だけで遅れるので、ここは必ずまとめる。
const BATCH_FRAMES: u64 = SAMPLE_RATE as u64 / 1000;
/// 1 転送に詰める上限。EP は 32B だが WinUSB が勝手に分割してくれる。
const MAX_XFER: usize = 256;

/// デバイスクロックの読み口。取り込みワーカが誰であっても時計は 1 つ。
pub trait FrameSource: Send + Sync {
    fn frames(&self) -> u64;
}

impl FrameSource for Shared {
    fn frames(&self) -> u64 {
        self.frames_captured.load(Relaxed)
    }
}

impl FrameSource for CaptureCtl {
    fn frames(&self) -> u64 {
        self.frames.load(Relaxed)
    }
}

/// 取り込み済みフレーム数を、8ms の更新間隔の内側まで補間して読む時計。
pub struct Clock {
    src: Arc<dyn FrameSource>,
    last: u64,
    at: Instant,
}

impl Clock {
    pub fn new(src: Arc<dyn FrameSource>) -> Self {
        Self {
            last: src.frames(),
            at: Instant::now(),
            src,
        }
    }

    pub fn now(&mut self) -> u64 {
        let f = self.src.frames();
        if f != self.last {
            self.last = f;
            self.at = Instant::now();
        }
        self.last + (self.at.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as u64
    }

    /// `target` フレームまで待つ。遠いうちは寝て、近づいたら譲るだけにする。
    /// `abort` が立ったら待たずに戻る。
    pub fn wait_until(&mut self, target: u64, abort: &AtomicBool) {
        loop {
            if abort.load(Relaxed) {
                return;
            }
            let now = self.now();
            if now >= target {
                return;
            }
            let ms = (target - now) as f64 / SAMPLE_RATE as f64 * 1000.0;
            if ms > 2.0 {
                std::thread::sleep(Duration::from_millis((ms - 1.5) as u64));
            } else {
                std::thread::yield_now();
            }
        }
    }
}

/// RT スレッドから送出スレッドへの片道キュー。
///
/// 積むのは「いつ (デバイスフレーム) 何を (4 バイトパケット)」だけ。
/// 過去のフレームを指定すれば「今すぐ」の意味になる。
pub struct MidiSched {
    q: Ring,
    waiter: OnceLock<Thread>,
}

impl MidiSched {
    /// `records` は 2 の冪。48kHz で 1 ブロック分の和音を数ブロック分ためられれば十分。
    pub fn new(records: usize) -> Self {
        Self {
            q: Ring::new(records, RECORD_BYTES),
            waiter: OnceLock::new(),
        }
    }

    /// 積めなかった (キューが溢れた) 回数。
    pub fn dropped(&self) -> usize {
        self.q.overruns()
    }

    /// RT スレッド専用。確保もロックもしない。
    pub fn push_packet(&self, frame: u64, pkt: [u8; 4]) {
        let mut rec = [0u8; RECORD_BYTES];
        rec[..8].copy_from_slice(&frame.to_le_bytes());
        rec[8..].copy_from_slice(&pkt);
        self.q.push(&rec);
    }

    /// 完全な MIDI メッセージ 1 個を、指定フレームに積む。
    pub fn push_msg(&self, frame: u64, msg: &[u8], cable: u8) {
        for_each_packet(msg, cable, |p| self.push_packet(frame, p));
    }

    /// 積み終わったら呼ぶ。送出スレッドが寝ていれば起こす。
    pub fn wake(&self) {
        if let Some(t) = self.waiter.get() {
            t.unpark();
        }
    }

    /// 全 ch の All Sound Off / Reset All Controllers を今すぐ積む。
    /// 停止時とデバイス解放前に必ず通すこと。
    pub fn panic_now(&self, cables: &[u8]) {
        for &cable in cables {
            for ch in 0..16u8 {
                self.push_msg(0, &[0xB0 | ch, 120, 0], cable);
                self.push_msg(0, &[0xB0 | ch, 121, 0], cable);
            }
        }
        self.wake();
    }
}

/// 送出スレッド。`MidiSched` に積まれたパケットを、指定フレームまで待って
/// IF2 の bulk OUT に流す。`UsbMidi` はこのスレッドだけが触る (パイプに対する
/// I/O を 1 本に絞るため)。
pub struct MidiSender {
    stop: Arc<AtomicBool>,
    sched: Arc<MidiSched>,
    handle: Option<JoinHandle<()>>,
}

impl MidiSender {
    pub fn start(
        midi: UsbMidi,
        sched: Arc<MidiSched>,
        clock: Arc<dyn FrameSource>,
    ) -> std::result::Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (s, q, st) = (sched.clone(), sched.clone(), stop.clone());
        let handle = std::thread::Builder::new()
            .name("sd90-midi".into())
            .spawn(move || {
                let _ = q.waiter.set(std::thread::current());
                worker(&midi, &s, &st, clock);
            })
            .map_err(|e| format!("送出スレッドを起動できない: {e}"))?;
        Ok(Self {
            stop,
            sched,
            handle: Some(handle),
        })
    }
}

impl Drop for MidiSender {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        self.sched.wake();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn decode(rec: &[u8]) -> (u64, [u8; 4]) {
    let mut f = [0u8; 8];
    f.copy_from_slice(&rec[..8]);
    let mut p = [0u8; 4];
    p.copy_from_slice(&rec[8..]);
    (u64::from_le_bytes(f), p)
}

fn worker(midi: &UsbMidi, sched: &MidiSched, stop: &AtomicBool, src: Arc<dyn FrameSource>) {
    let _mmcss = crate::iso::join_pro_audio();
    let mut clock = Clock::new(src);
    let mut pending: VecDeque<(u64, [u8; 4])> = VecDeque::with_capacity(256);
    let mut buf = [0u8; MAX_XFER];

    loop {
        let stopping = stop.load(Relaxed);

        // キューに積まれたものを全部手元に移す。順序はフレームの昇順で入る。
        let mut rec = [0u8; RECORD_BYTES];
        while sched.q.consume(1, |b| rec.copy_from_slice(b)) {
            pending.push_back(decode(&rec));
        }

        let Some(&(due, _)) = pending.front() else {
            if stopping {
                return;
            }
            // 次に来るものは RT スレッドが起こしてくれる。取りこぼしても
            // 4ms で見に来るので詰まらない。
            std::thread::park_timeout(Duration::from_millis(4));
            continue;
        };

        // 停止中は時刻を無視して吐き切る (All Sound Off を落とさないため)。
        if !stopping {
            clock.wait_until(due, stop);
        }

        // due から 1ms 以内に並んでいるものは 1 転送にまとめる。
        let mut n = 0usize;
        while let Some(&(f, p)) = pending.front() {
            if !stopping && f > due + BATCH_FRAMES {
                break;
            }
            if n + 4 > buf.len() {
                break;
            }
            buf[n..n + 4].copy_from_slice(&p);
            n += 4;
            pending.pop_front();
        }
        if n > 0 && midi.write_in_place(&mut buf[..n]).is_err() {
            // デバイスが抜けた等。溜まっているものは捨てて畳む。
            return;
        }
    }
}
