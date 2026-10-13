//! SMF を SD-90 に流しながら、その音を USB で録る。
//!
//! 肝は時計をどれにするか。ホストの `Instant` を基準にすると、SD-90 の水晶と
//! ppm 単位でずれているぶんが 3 分の曲では数十ミリ秒たまる。録音側のフレーム
//! 数はデバイスの水晶そのものなので、**MIDI の送出位置を「録音済みフレーム数」
//! で決める**。こうすると「録音の N サンプル目でこのノートを送った」が確定し、
//! あとで ZUN 版 WAV と突き合わせるときの基準になる。
//!
//! ただしフレーム数の更新は 1 転送 = 8ms に 1 回しか来ない。その間は
//! 「最後に更新を見た時刻からの経過 x 48kHz」で補間する。デバイスクロックで
//! 位相を合わせ、その間をホストの時計で埋める形になる。
//!
//! MIDI と音は同じ 1 本の USB に乗る。MIDI は IF2 の bulk、音は IF1 の iso。

use crate::smf;
use sd90_asio::device::{PID_VENDOR, SAMPLE_RATE, Sd90};
use sd90_asio::iso::{self, CaptureCtl};
use sd90_asio::usbmidi::UsbMidi;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

/// 同時刻とみなしてまとめて送る幅。1ms。
const BATCH_FRAMES: u64 = SAMPLE_RATE as u64 / 1000;

pub struct Opts {
    /// USB-MIDI のケーブル番号。0 = PART A, 1 = PART B, 2/3 = 背面の MIDI OUT。
    pub cable: u8,
    /// 1 音目の前に空ける無音 (ms)。音源側が SysEx を消化する時間でもある。
    pub preroll_ms: u64,
    /// 最後のイベントのあと録り続ける長さ (ms)。残響を切らないため。
    pub tail_ms: u64,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            cable: 0,
            preroll_ms: 1500,
            tail_ms: 4000,
        }
    }
}

/// 録音済みフレーム数を、8ms の更新間隔の内側まで補間して読む時計。
struct Clock {
    ctl: Arc<CaptureCtl>,
    last: u64,
    at: Instant,
}

impl Clock {
    fn new(ctl: Arc<CaptureCtl>) -> Self {
        Self {
            last: ctl.frames.load(Relaxed),
            at: Instant::now(),
            ctl,
        }
    }

    fn now(&mut self) -> u64 {
        let f = self.ctl.frames.load(Relaxed);
        if f != self.last {
            self.last = f;
            self.at = Instant::now();
        }
        self.last + (self.at.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as u64
    }

    /// `target` フレームまで待つ。遠いうちは寝て、近づいたら譲るだけにする。
    fn wait_until(&mut self, target: u64) {
        loop {
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

pub fn run(mid_path: &str, out_wav: &str, opts: &Opts) -> Result<(), String> {
    let song = smf::load(mid_path)?;
    println!(
        "{mid_path}: {} イベント / {:.2}s / division={}{}",
        song.events.len(),
        song.duration,
        song.division,
        if song.name.is_empty() {
            String::new()
        } else {
            format!(" / {}", song.name)
        }
    );
    if let Some((_, bpm)) = song.tempos.first() {
        println!(
            "  テンポ {:.1} BPM から (変更 {} 回)",
            bpm,
            song.tempos.len().saturating_sub(1)
        );
    }

    run_song(&song, out_wav, opts)?;
    Ok(())
}

/// 曲を 1 本流して録る。`run` はファイルから読んだものを、`miditest` はその場で
/// 組んだものを渡す。戻り値は演奏開始位置 (録音バッファ上のフレーム)。
pub fn run_song(song: &smf::Song, out_wav: &str, opts: &Opts) -> Result<u64, String> {
    if song.events.is_empty() {
        return Err("イベントが 1 つも無い".into());
    }
    // MIDI (IF2 bulk) と録音 (IF1 iso) で同じデバイスを共有する。
    let dev = Arc::new(Sd90::open(PID_VENDOR).map_err(|e| format!("SD-90 を開けない: {e}"))?);
    let midi = UsbMidi::open_cable(dev.clone(), opts.cable).map_err(|e| e.to_string())?;
    println!("MIDI 出力: SD-90 IF2 cable {}", opts.cable);

    let ctl = Arc::new(CaptureCtl::default());
    let total_ms = ((song.duration * 1000.0) as u64) + opts.preroll_ms + opts.tail_ms + 2000;

    let (cap_dev, cap_ctl, out) = (dev.clone(), ctl.clone(), out_wav.to_string());
    let cap = std::thread::spawn(move || -> Result<iso::CaptureStats, String> {
        iso::capture_with(&cap_dev, total_ms, &out, Some(&cap_ctl)).map_err(|e| e.to_string())
    });

    // 取り込みが実際に回り始める (フレームが増え始める) まで待つ。
    let wait_start = Instant::now();
    while ctl.frames.load(Relaxed) == 0 {
        if cap.is_finished() {
            return match cap.join() {
                Ok(Err(e)) => Err(e),
                _ => Err("取り込みスレッドが即座に終了した".into()),
            };
        }
        if wait_start.elapsed() > Duration::from_secs(5) {
            ctl.stop.store(true, Relaxed);
            let _ = cap.join();
            return Err("5 秒待っても iso IN が回り始めない".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // このスレッドも Pro Audio にしておく (送出が 15ms 単位でずれるのを防ぐ)。
    let _mmcss = iso::join_pro_audio();

    let mut clock = Clock::new(ctl.clone());
    let t0 = clock.now() + opts.preroll_ms * SAMPLE_RATE as u64 / 1000;
    let frame_of = |sec: f64| t0 + (sec * SAMPLE_RATE as f64) as u64;

    // 送出位置の記録。frame は「録音バッファの何フレーム目で送ったか」。
    let mut map: Vec<(u64, f64, &smf::Msg)> = Vec::with_capacity(song.events.len());
    let mut late_max = 0i64;
    let mut late_sum = 0i64;
    let mut batches = 0u64;
    let mut next_report = Instant::now() + Duration::from_secs(5);

    let mut i = 0usize;
    while i < song.events.len() {
        let target = frame_of(song.events[i].sec);
        clock.wait_until(target);

        // 1ms 以内に並んでいるぶんは 1 転送にまとめる。和音の頭では数十イベントが
        // ほぼ同じ tick に来るので、まとめないと USB の転送回数だけで遅れる。
        let mut j = i;
        let mut batch: Vec<&[u8]> = Vec::new();
        while j < song.events.len() && frame_of(song.events[j].sec) <= target + BATCH_FRAMES {
            batch.push(song.events[j].msg.bytes());
            j += 1;
        }

        let sent = clock.now();
        midi.send(&batch).map_err(|e| e.to_string())?;
        batches += 1;
        let late = (sent as i64 - target as i64).abs();
        late_sum += late * batch.len() as i64;
        late_max = late_max.max(late);
        for e in &song.events[i..j] {
            map.push((sent, e.sec, &e.msg));
        }
        i = j;

        if Instant::now() > next_report {
            next_report += Duration::from_secs(5);
            print!("\r  {:6.1}s / {:.1}s", song.events[i - 1].sec, song.duration);
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
    }
    println!("\r  {:6.1}s / {:.1}s  送出完了", song.duration, song.duration);

    // 残響ぶん録ってから止める。
    clock.wait_until(frame_of(song.duration) + opts.tail_ms * SAMPLE_RATE as u64 / 1000);
    midi.silence();
    ctl.stop.store(true, Relaxed);

    let stats = match cap.join() {
        Ok(r) => r?,
        Err(_) => return Err("取り込みスレッドが落ちた".into()),
    };
    iso::report(&stats, out_wav);

    let n = map.len().max(1) as f64;
    println!(
        "MIDI 送出のずれ: 平均 {:.2}ms / 最大 {:.2}ms ({} イベントを {} 転送)",
        late_sum as f64 / n / SAMPLE_RATE as f64 * 1000.0,
        late_max as f64 / SAMPLE_RATE as f64 * 1000.0,
        map.len(),
        batches
    );
    println!(
        "演奏開始位置: 録音の {t0} フレーム目 ({:.3}s)",
        t0 as f64 / SAMPLE_RATE as f64
    );

    write_map(out_wav, t0, &map)?;
    Ok(t0)
}

/// 各イベントを録音のどのフレームで送ったかを書き出す。
/// 解析側 (Python) はこれを読めば、録った WAV のサンプル位置と楽譜が直結する。
fn write_map(out_wav: &str, t0: u64, map: &[(u64, f64, &smf::Msg)]) -> Result<(), String> {
    use std::io::Write;
    let path = format!("{out_wav}.map");
    let mut f =
        std::io::BufWriter::new(std::fs::File::create(&path).map_err(|e| format!("{path}: {e}"))?);
    writeln!(f, "# sd90-rs render event map").map_err(|e| e.to_string())?;
    writeln!(f, "# sample_rate\t{SAMPLE_RATE}").map_err(|e| e.to_string())?;
    writeln!(f, "# t0_frame\t{t0}").map_err(|e| e.to_string())?;
    writeln!(f, "# frame\tsec\tbytes").map_err(|e| e.to_string())?;
    for (frame, sec, msg) in map {
        let hex: String = msg.bytes().iter().map(|b| format!("{b:02x}")).collect();
        writeln!(f, "{frame}\t{sec:.6}\t{hex}").map_err(|e| e.to_string())?;
    }
    println!("イベント位置: {path}");
    Ok(())
}
