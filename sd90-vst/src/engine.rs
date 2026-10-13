//! SD-90 を 1 つ持ち、DAW のブロックとデバイスのフレームを対応づける層。
//!
//! 録音バスは物理的に 1 系統しかないので、**音を出せるのはプロセス内で 1 つの
//! インスタンスだけ**。2 個目以降は掴めずに無音を返す。ここでシングルトンに
//! しておけば、上のプラグイン層はそれを報告するだけで済む。
//!
//! # タイムライン
//!
//! プラグインは「イベントを先に受け取る」ことができない。できるのは実際の
//! 遅れを正しく申告することだけで、それさえ合っていればホストが他トラックを
//! ずらして辻褄を合わせる (PDC)。
//!
//! * `B` … リングの目標滞留量。出力はライブの取り込みから常に B 遅れる。
//! * `d` … 音源の MIDI 受信から発音までの応答。`sd90-rs latency` の実測値。
//! * 申告レイテンシ **`L = B + d`**。
//!
//! このとき、ブロック先頭の読み出し位置を `R` として、
//! **sampleOffset `k` のイベントはデバイスフレーム `R + B + k` に送ればよい**。
//!
//! 導出: 出力位置 `P` の音は取り込みフレーム `R + (P - P_now)` のもの。ホストは
//! 我々の出力を `L` 遅れとみなしているので、位置 `P` のイベントは出力位置
//! `P + L` で聞こえなければならない。それは取り込みフレーム
//! `R + (P - P_now) + L` にあたり、そこで鳴るには `d` だけ手前に送る必要がある
//! から `R + (P - P_now) + B`。`k = P - P_now` で上の式になる。
//!
//! 大事なのは、ここにホストの時計が一切出てこないこと。`R` はリングの読み出し
//! 位置から厳密に決まり、`B` は定数。送出スレッドだけがデバイスクロックを見る。

use crate::resample::Reader;
use sd90_rs::device::{PID_VENDOR, SAMPLE_RATE, Sd90};
use sd90_rs::latency::MODULE_RESPONSE_FRAMES;
use sd90_rs::midisched::{MidiSched, MidiSender};
use sd90_rs::stream::{InputStream, Shared, StreamCfg};
use sd90_rs::usbmidi::UsbMidi;
use std::sync::atomic::Ordering::{Acquire, Relaxed};
use std::sync::{Arc, Mutex, Weak};

/// リングの容量。0.34 秒。目標水位よりずっと大きく取っておく。
const RING_FRAMES: usize = 16384;
/// 目標滞留量 `B` の余裕分。2ms。
///
/// アンダーランは「ホストが早く来た」ではなく「USB スレッドの起床が遅れた」
/// ときに起きる。1ms 粒度で 48 フレームずつ積まれるので、2ms ぶん持っていれば
/// 数回の遅れは飲める。
const SAFETY_MARGIN_FRAMES: usize = SAMPLE_RATE as usize / 500;

/// ブロック長 (**デバイスフレーム**に換算したもの) に対する目標滞留量 `B`。
///
/// **ブロック長を下回ってはいけない。** ホストは 1 ブロックぶんを一度に
/// 吸っていくので、その瞬間にリングがブロック長を持っていなければ底を打つ。
/// B を 4ms 固定にして 256 フレーム (5.33ms) のブロックを回すと、水位の平均は
/// 足りていてもアンダーランが出る。VST3 では `setupProcessing` が
/// `maxSamplesPerBlock` をくれるので、それを入れること。
pub fn safety_for(max_block: usize) -> usize {
    max_block + SAFETY_MARGIN_FRAMES
}
/// PART A / PART B / 背面 MIDI OUT 1 / 2。
pub const CABLES: [u8; 4] = [0, 1, 2, 3];

static ENGINE: Mutex<Weak<Engine>> = Mutex::new(Weak::new());

pub struct Engine {
    shared: Arc<Shared>,
    sched: Arc<MidiSched>,
    /// 目標滞留量 `B`。
    safety: usize,
    // 落とす順番が意味を持つ。sender (残りを吐き切る) -> stream -> dev。
    _sender: MidiSender,
    _stream: InputStream,
    _dev: Arc<Sd90>,
}

impl Engine {
    /// 既に誰かが持っていればそれを返す。無ければ開く。
    ///
    /// 掴めるのは 1 プロセスに 1 つ。ASIO ドライバが同じ機を使っている間は
    /// IF1 の alt を取り合うので、ここが失敗する。
    pub fn acquire(safety: usize) -> Result<Arc<Engine>, String> {
        let mut slot = ENGINE.lock().map_err(|_| "エンジンのロックが壊れている")?;
        if let Some(e) = slot.upgrade() {
            return Ok(e);
        }
        let e = Arc::new(Engine::open(safety)?);
        *slot = Arc::downgrade(&e);
        Ok(e)
    }

    fn open(safety: usize) -> Result<Engine, String> {
        let dev = Arc::new(Sd90::open(PID_VENDOR).map_err(|e| {
            format!("SD-90 を開けない ({e})。Vendor モードと Zadig の適用、ASIO ドライバとの併用を確認すること")
        })?);
        // MIDI はストリームより先に開くこと。`open_cable` は IF2 の alt を設定
        // し直す (SET_INTERFACE) が、WinUSB は IF1 に iso 転送が出ている間の
        // alt 変更を ERROR_BUSY (0x800700AA) で断る。逆順だと必ずここで落ちる。
        let midi = UsbMidi::open_cable(dev.clone(), 0).map_err(|e| e.to_string())?;
        let shared = Arc::new(Shared::new(RING_FRAMES));
        // 送出はしない。時計は 1ms 粒度 (理由は StreamCfg の項)。
        let stream =
            InputStream::start_cfg(shared.clone(), StreamCfg::capture_only(), Some(dev.clone()))?;
        let sched = Arc::new(MidiSched::new(4096));
        let sender = MidiSender::start(midi, sched.clone(), shared.clone())?;
        Ok(Engine {
            shared,
            sched,
            safety,
            _sender: sender,
            _stream: stream,
            _dev: dev,
        })
    }

    pub fn safety(&self) -> usize {
        self.safety
    }

    /// ホストに申告するレイテンシ `L = B + d`。
    pub fn latency_frames(&self) -> usize {
        self.safety + MODULE_RESPONSE_FRAMES as usize
    }

    /// リングの読み出し位置に対応するデバイスフレーム `R`。
    ///
    /// USB スレッドは「押し込む → `frames_captured` を進める」の順で動くので、
    /// 2 つを別々に読むと最大 1 転送ぶんずれる。前後で同じ値を読めるまで
    /// 回して整合を取る (待つ側は RT スレッドなので、ロックは使えない)。
    pub fn read_frame(&self) -> u64 {
        loop {
            let a = self.shared.frames_captured.load(Acquire);
            let avail = self.shared.ring.available() as u64;
            if self.shared.frames_captured.load(Relaxed) == a {
                return a.saturating_sub(avail);
            }
        }
    }

    /// このブロックの sampleOffset 0 に対応する送出フレーム。
    /// イベントは `dispatch_base() + sample_offset` に積む。
    pub fn dispatch_base(&self) -> u64 {
        self.read_frame() + self.safety as u64
    }

    /// RT スレッド専用。確保もロックもしない。
    pub fn schedule(&self, frame: u64, msg: &[u8], cable: u8) {
        self.sched.push_msg(frame, msg, cable);
    }

    /// 積み終わったら呼ぶ。送出スレッドを起こす。
    pub fn flush(&self) {
        self.sched.wake();
    }

    /// 停止時に必ず通す。全 cable の全 ch に All Sound Off を送る。
    pub fn panic(&self) {
        self.sched.panic_now(&CABLES);
    }

    pub fn ring(&self) -> &sd90_rs::ring::Ring {
        &self.shared.ring
    }

    /// 診断用。(取り込み総フレーム, リング溢れ, 送出キュー溢れ)。
    pub fn diag(&self) -> (u64, usize, usize) {
        (
            self.shared.frames_captured.load(Relaxed),
            self.shared.ring.overruns(),
            self.sched.dropped(),
        )
    }

    /// このエンジン用の読み出し器 (ホストが 48kHz のとき)。ブロックごとに `pull` する。
    pub fn reader(&self, max_block: usize) -> Reader {
        Reader::new(self.safety, max_block, 1.0)
    }
}
