//! 音源の応答 `d` — 「MIDI をこのデバイスフレームに送る」と決めてから、
//! 実際にその音が取り込み側に現れるまでの遅れ — を実測する。
//!
//! VST3 の申告レイテンシは `L = B + d` (B = リングの目標滞留量) で決まる。
//! `L` がずれていると DAW の PDC が丸ごとずれるので、`d` は勘で置かず測る。
//!
//! 測り方は sd90-seq の render と同じ発想で、送出位置も観測位置も**デバイスの
//! フレーム番号**で表す。ホストの時計は一切基準にしない。ついでにこの経路は
//! VST3 プラグインがそのまま使う経路 (capture-only ストリーム + midisched) なので、
//! ここが通れば Phase 0 の配線は通っている。
//!
//! 送出スレッドの誤差 (実測 0.1〜1.4ms) は `d` に混ざるが、VST3 プラグインでも同じ
//! スレッドが同じ仕事をする以上、混ざったままの値のほうが申告値として正しい。

use crate::device::{BYTES_PER_SAMPLE, CHANNELS, FRAME_BYTES, PID_VENDOR, SAMPLE_RATE, Sd90};
use crate::midisched::{Clock, MidiSched, MidiSender};
use crate::stream::{InputStream, Shared, StreamCfg};
use crate::usbmidi::UsbMidi;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// 実測した `d`。`StreamCfg::capture_only` (1ms 粒度) での 16 回の中央値。
///
/// ```text
/// cable 0 / ch 10 / note 38 x 16
/// d = 162 フレーム (3.38ms)   最小 3.33ms / 最大 3.46ms / 検出 16/16
/// ```
///
/// VST3 の申告レイテンシはこれに B (リングの目標滞留量) を足したもの。
/// 個体差や音色で変わるので、疑わしければ `sd90-rs latency` で測り直すこと。
pub const MODULE_RESPONSE_FRAMES: u64 = 162;

const RING_FRAMES: usize = 16384;
/// 1 発目までの助走。ストリームが定常になるのを待つ。
const SETTLE_MS: u64 = 800;
/// 送出を予約する先の距離。送出スレッドが確実に間に合う程度に取る。
const LEAD_MS: u64 = 200;
/// 打鍵の間隔。残響が次の窓に残らない程度。
const SPACING_MS: u64 = 1200;
/// ノートオンからオフまで。
const GATE_MS: u64 = 250;
/// オンセットを探す窓。
const SEARCH_MS: u64 = 400;
/// 送出前のノイズフロアを測る窓。
const FLOOR_MS: u64 = 150;

fn ms_to_frames(ms: u64) -> u64 {
    ms * SAMPLE_RATE as u64 / 1000
}

fn frames_to_ms(f: i64) -> f64 {
    f as f64 * 1000.0 / SAMPLE_RATE as f64
}

fn sample_i24_le(b: &[u8]) -> i32 {
    let v = (b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16);
    (v << 8) >> 8
}

/// フレーム `i` の 2ch のうち大きいほうの絶対値。
fn level(pcm: &[u8], i: usize) -> i32 {
    let base = i * FRAME_BYTES;
    let mut m = 0;
    for ch in 0..CHANNELS {
        let s = sample_i24_le(&pcm[base + ch * BYTES_PER_SAMPLE..]).abs();
        if s > m {
            m = s;
        }
    }
    m
}

/// `send` フレームに送ったノートのオンセットを探す。
/// 戻り値は (オンセットのフレーム, 使った閾値, 窓内のピーク, ノイズフロア)。
fn find_onset(pcm: &[u8], send: u64) -> Option<(u64, i32, i32, i32)> {
    let total = pcm.len() / FRAME_BYTES;
    let send = send as usize;
    let floor_from = send.saturating_sub(ms_to_frames(FLOOR_MS) as usize);
    let to = (send + ms_to_frames(SEARCH_MS) as usize).min(total);
    if send >= total || floor_from >= send {
        return None;
    }

    let floor = (floor_from..send).map(|i| level(pcm, i)).max().unwrap_or(0);
    let peak = (send..to).map(|i| level(pcm, i)).max().unwrap_or(0);
    // ノイズフロアの 4 倍か、ピークの -26dB の高いほう。前者だけだと無音時の
    // DC オフセットで、後者だけだと残響の裾で誤検出する。
    let th = (floor.saturating_mul(4)).max(peak / 20).max(64);
    if peak < th {
        return None;
    }
    (send..to)
        .find(|&i| level(pcm, i) >= th)
        .map(|i| (i as u64, th, peak, floor))
}

pub struct Opts {
    pub count: usize,
    pub cable: u8,
    /// 1 始まり。既定の 10 は GS のドラムパート。
    pub channel: u8,
    /// 既定の 38 は Acoustic Snare。立ち上がりが速いものを選ぶこと。
    pub note: u8,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            count: 8,
            cable: 0,
            channel: 10,
            note: 38,
        }
    }
}

pub fn measure(opts: &Opts) -> Result<(), String> {
    let dev = Arc::new(Sd90::open(PID_VENDOR).map_err(|e| format!("SD-90 を開けない: {e}"))?);
    let shared = Arc::new(Shared::new(RING_FRAMES));
    // VST3 プラグインと同じ構成。iso OUT は回さない。
    let _stream =
        InputStream::start_cfg(shared.clone(), StreamCfg::capture_only(), Some(dev.clone()))?;
    let midi = UsbMidi::open_cable(dev.clone(), opts.cable).map_err(|e| e.to_string())?;
    let sched = Arc::new(MidiSched::new(1024));
    let _sender = MidiSender::start(midi, sched.clone(), shared.clone())?;

    // 取り込みをすべて手元に落とす。リングは 0.34s 分しかないので溜めない。
    let drain_stop = Arc::new(AtomicBool::new(false));
    let drain = {
        let (shared, stop) = (shared.clone(), drain_stop.clone());
        std::thread::spawn(move || {
            let mut pcm: Vec<u8> = Vec::with_capacity(RING_FRAMES * FRAME_BYTES * 16);
            loop {
                let avail = shared.ring.available();
                if avail > 0 {
                    shared
                        .ring
                        .consume(avail, |part| pcm.extend_from_slice(part));
                } else if stop.load(Relaxed) {
                    return pcm;
                } else {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        })
    };

    // ストリームが回り始めるまで待つ。
    let t0 = Instant::now();
    while shared.frames_captured.load(Relaxed) == 0 {
        if t0.elapsed() > Duration::from_secs(3) {
            drain_stop.store(true, Relaxed);
            let _ = drain.join();
            return Err("取り込みが始まらない".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut clock = Clock::new(shared.clone());
    let never = AtomicBool::new(false);
    let settled = clock.now() + ms_to_frames(SETTLE_MS);
    clock.wait_until(settled, &never);

    println!(
        "cable {} / ch {} / note {} を {} 回",
        opts.cable, opts.channel, opts.note, opts.count
    );

    let st = 0x90 | (opts.channel.saturating_sub(1) & 0x0F);
    let mut sends: Vec<u64> = Vec::with_capacity(opts.count);
    let base = clock.now() + ms_to_frames(LEAD_MS);
    for i in 0..opts.count {
        let at = base + ms_to_frames(SPACING_MS) * i as u64;
        sched.push_msg(at, &[st, opts.note, 127], opts.cable);
        sched.push_msg(at + ms_to_frames(GATE_MS), &[st, opts.note, 0], opts.cable);
        sched.wake();
        sends.push(at);
    }

    // 最後の打鍵の窓が埋まるまで待ってから畳む。
    let end = base + ms_to_frames(SPACING_MS) * opts.count as u64 + ms_to_frames(SEARCH_MS);
    clock.wait_until(end, &never);
    sched.panic_now(&[opts.cable]);
    std::thread::sleep(Duration::from_millis(100));

    drain_stop.store(true, Relaxed);
    let pcm = drain.join().map_err(|_| "取り込みスレッドが落ちた")?;

    let overruns = shared.ring.overruns();
    let dropped = sched.dropped();
    if overruns > 0 {
        return Err(format!(
            "リングが {overruns} 回溢れた。フレーム番号と録音位置がずれるので測定にならない"
        ));
    }
    if dropped > 0 {
        eprintln!("警告: 送出キューが {dropped} 回溢れた");
    }

    println!(
        "\n取り込み {} フレーム ({:.1}s)\n",
        pcm.len() / FRAME_BYTES,
        pcm.len() as f64 / FRAME_BYTES as f64 / SAMPLE_RATE as f64
    );
    println!("  #  送出フレーム  オンセット      d        閾値/ピーク/床");
    let mut ds: Vec<i64> = Vec::with_capacity(opts.count);
    for (i, &s) in sends.iter().enumerate() {
        match find_onset(&pcm, s) {
            Some((on, th, peak, floor)) => {
                let d = on as i64 - s as i64;
                ds.push(d);
                println!(
                    "{:>3}  {:>12}  {:>9}  {:>5} ({:>5.2}ms)  {th}/{peak}/{floor}",
                    i + 1,
                    s,
                    on,
                    d,
                    frames_to_ms(d)
                );
            }
            None => println!("{:>3}  {:>12}  検出できず", i + 1, s),
        }
    }

    if ds.is_empty() {
        return Err(
            "1 発も検出できなかった。音源が鳴っているか、録音バスに乗っているかを確認すること"
                .into(),
        );
    }
    ds.sort_unstable();
    let med = ds[ds.len() / 2];
    let (lo, hi) = (ds[0], ds[ds.len() - 1]);
    println!(
        "\n中央値 d = {med} フレーム ({:.2}ms)   最小 {:.2}ms / 最大 {:.2}ms / 検出 {}/{}",
        frames_to_ms(med),
        frames_to_ms(lo),
        frames_to_ms(hi),
        ds.len(),
        opts.count
    );
    if frames_to_ms(hi - lo) > 3.0 {
        eprintln!(
            "ばらつきが 3ms を超えている。送出スレッドの優先度か、音源側の\n\
             アタックの遅い音色を疑うこと (打楽器系のノートを指定する)"
        );
    }
    Ok(())
}
