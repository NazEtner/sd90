//! IF1 / EP 0x81 からアイソクロナス IN を吸い出す。
//!
//! この機の iso エンドポイントは sync type = None なので、フィードバック
//! エンドポイントの実装は要らない。IN 方向はデバイスがクロックを刻むので、
//! こちらは「取りこぼさないよう先読みで積んでおく」ことだけ考えればよい。
//!
//! 踏んだ罠: `USBD_ISO_PACKET_DESCRIPTOR.Offset` は登録バッファ先頭からの
//! 絶対値ではなく、その転送に渡した offset からの相対値。どのスロットでも
//! 0,320,640,... が返るので、スロットの基点を足し忘れると常にスロット 0 の
//! 領域を読み続ける。しかも WinUSB は何も文句を言わず、パケット記述子の
//! Status は 0、平均サンプル数も 48.000 のままなので、波形の不連続として
//! しか現れない。転送境界の 25〜30% が飛ぶという形で発覚した。
//!
//! なお回収ループの中では「届いたバイトを連結する」以外の仕事をしない。
//! ファイル書き込みやサンプル単位の演算を挟むとキューが枯れうるため。

use crate::device::{
    ALT_STREAMING, BYTES_PER_SAMPLE, CHANNELS, EP_CAPTURE_IN, EXPECTED_PACKET_BYTES, FRAME_BYTES,
    IF_CAPTURE, SAMPLE_RATE, Sd90,
};
use std::ffi::c_void;
use std::io::Write;
use windows::Win32::Devices::Usb::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::IO::{CancelIoEx, OVERLAPPED};
use windows::Win32::System::Threading::*;
use windows::core::{PCWSTR, Result, w};

/// 1 転送あたりのパケット数 = ミリ秒数。
const PACKETS_PER_XFER: usize = 8;
/// 同時に走らせる転送数。8ms x 8 = 64ms 先まで積む。
const SLOTS: usize = 8;

/// 取り込みループの外から時計を読み、外から止めるための口。
///
/// `frames` は「これまでに連結したフレーム数」= デバイスの水晶が刻んだ時間そのもの。
/// ホストの `Instant` と違って SD-90 のクロックに対して原理的にずれないので、
/// MIDI の送出タイミングはこれを基準にする。更新は 1 転送 (8ms) に 1 回。
pub struct CaptureCtl {
    pub frames: std::sync::atomic::AtomicU64,
    pub stop: std::sync::atomic::AtomicBool,
}

impl Default for CaptureCtl {
    fn default() -> Self {
        Self {
            frames: std::sync::atomic::AtomicU64::new(0),
            stop: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

pub struct CaptureStats {
    pub packets: u64,
    pub bytes: u64,
    pub short_packets: u64,
    pub zero_packets: u64,
    pub errors: u64,
    pub len_histogram: std::collections::BTreeMap<u32, u64>,
    pub peak: [i32; CHANNELS],
    pub first_packet: Vec<u8>,
    pub elapsed_ms: u64,
    /// 転送境界の総数と、そのうち波形が不連続だった数。
    pub boundaries: usize,
    pub boundary_glitches: usize,
}

/// overlapped I/O は「まだ完了していない」を FALSE + ERROR_IO_PENDING で返す。
/// windows クレートはこれも Err に畳んでしまうので、正常系として拾い直す。
fn ok_or_pending(r: Result<()>) -> Result<()> {
    match r {
        Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => Ok(()),
        other => other,
    }
}

/// MMCSS で "Pro Audio" 特性を取る。失敗しても致命的ではないので無視する。
/// 取り込みスレッド以外 (送出側など) からも使うので公開しておく。
pub fn join_pro_audio() -> Option<HANDLE> {
    let mut idx = 0u32;
    unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut idx) }.ok()
}

/// 24bit リトルエンディアン想定で 1 サンプルを取り出す。
fn sample_i24_le(b: &[u8]) -> i32 {
    let v = (b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16);
    (v << 8) >> 8 // 符号拡張
}

fn frame_sample(buf: &[u8], frame: usize, ch: usize) -> i32 {
    sample_i24_le(&buf[frame * FRAME_BYTES + ch * BYTES_PER_SAMPLE..])
}

/// 出力先が .wav なら WAV ヘッダを被せる。パケット長は 282/288/294 と揺れるが
/// すべて 6 の倍数なので、そのまま連結してもフレーム境界は崩れない。
fn wav_header(data_len: u32) -> [u8; 44] {
    let block_align = FRAME_BYTES as u16;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_len).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    h[22..24].copy_from_slice(&(CHANNELS as u16).to_le_bytes());
    h[24..28].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
    h[28..32].copy_from_slice(&(SAMPLE_RATE * block_align as u32).to_le_bytes());
    h[32..34].copy_from_slice(&block_align.to_le_bytes());
    h[34..36].copy_from_slice(&((BYTES_PER_SAMPLE * 8) as u16).to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_len.to_le_bytes());
    h
}

pub fn capture(dev: &Sd90, duration_ms: u64, out_path: &str) -> Result<CaptureStats> {
    capture_with(dev, duration_ms, out_path, None)
}

/// `ctl` を渡すと、取り込み済みフレーム数を外に見せ、外から早期終了できる。
/// `duration_ms` はその場合でも上限として効く (止め忘れの保険)。
pub fn capture_with(
    dev: &Sd90,
    duration_ms: u64,
    out_path: &str,
    ctl: Option<&CaptureCtl>,
) -> Result<CaptureStats> {
    use std::sync::atomic::Ordering::Relaxed;
    let iface = dev.iface(IF_CAPTURE);

    // alt 0 はエンドポイント無し(帯域ゼロ)。alt 1 に上げて初めてストリームが立つ。
    unsafe { WinUsb_SetCurrentAlternateSetting(iface, ALT_STREAMING)? };

    let mut pipe = WINUSB_PIPE_INFORMATION::default();
    let mut found = false;
    for idx in 0..8u8 {
        let mut info = WINUSB_PIPE_INFORMATION::default();
        if unsafe { WinUsb_QueryPipe(iface, ALT_STREAMING, idx, &mut info) }.is_ok()
            && info.PipeId == EP_CAPTURE_IN
        {
            pipe = info;
            found = true;
            break;
        }
    }
    if !found {
        let _ = unsafe { WinUsb_SetCurrentAlternateSetting(iface, 0) };
        return Err(windows::core::Error::new(
            ERROR_NOT_FOUND.to_hresult(),
            "EP 0x81 が見つからない",
        ));
    }
    let max_packet = pipe.MaximumPacketSize as usize;
    println!(
        "pipe 0x{:02X}  type={}  wMaxPacketSize={}  interval={}  ({} スロット x {} パケット = {}ms 先読み)",
        pipe.PipeId,
        pipe.PipeType.0,
        max_packet,
        pipe.Interval,
        SLOTS,
        PACKETS_PER_XFER,
        SLOTS * PACKETS_PER_XFER
    );

    let slot_bytes = PACKETS_PER_XFER * max_packet;
    let mut buf = vec![0u8; SLOTS * slot_bytes];

    let mut iso_handle: *mut c_void = std::ptr::null_mut();
    unsafe { WinUsb_RegisterIsochBuffer(iface, EP_CAPTURE_IN, &mut buf, &mut iso_handle)? };

    let _mmcss = join_pro_audio();

    let mut events: Vec<HANDLE> = Vec::with_capacity(SLOTS);
    let mut ovs: Vec<OVERLAPPED> = Vec::with_capacity(SLOTS);
    let mut descs: Vec<Vec<USBD_ISO_PACKET_DESCRIPTOR>> = Vec::with_capacity(SLOTS);
    for _ in 0..SLOTS {
        let ev = unsafe { CreateEventW(None, true, false, PCWSTR::null())? };
        events.push(ev);
        ovs.push(OVERLAPPED {
            hEvent: ev,
            ..Default::default()
        });
        descs.push(vec![
            USBD_ISO_PACKET_DESCRIPTOR::default();
            PACKETS_PER_XFER
        ]);
    }

    // 回収ループで確保が走らないよう、全部先に取っておく。
    let cap_packets = duration_ms as usize + PACKETS_PER_XFER * SLOTS + 64;
    let mut captured: Vec<u8> = Vec::with_capacity(cap_packets * max_packet);
    let mut lens: Vec<u32> = Vec::with_capacity(cap_packets);
    let mut boundary_frames: Vec<usize> = Vec::with_capacity(cap_packets / PACKETS_PER_XFER + 8);
    let mut zero_packets = 0u64;
    let mut errors = 0u64;
    // 最初の数転送分のパケット記述子を控えておく (Offset が登録バッファ基準か
    // 転送基準か、パケットが詰めて置かれるか max_packet 刻みかを見るため)。
    let debug_desc = std::env::var("SD90_DEBUG_DESC").is_ok();
    let mut desc_log: Vec<(usize, u32, Vec<(u32, u32, i32)>)> = Vec::with_capacity(8);

    let issue = |slot: usize,
                 ovs: &mut [OVERLAPPED],
                 descs: &mut [Vec<USBD_ISO_PACKET_DESCRIPTOR>],
                 cont: bool|
     -> Result<()> {
        unsafe {
            ResetEvent(ovs[slot].hEvent)?;
            ok_or_pending(WinUsb_ReadIsochPipeAsap(
                iso_handle,
                (slot * slot_bytes) as u32,
                slot_bytes as u32,
                cont,
                &mut descs[slot],
                Some(&ovs[slot]),
            ))
        }
    };

    let mut result: Result<()> = Ok(());
    for slot in 0..SLOTS {
        if let Err(e) = issue(slot, &mut ovs, &mut descs, slot != 0) {
            result = Err(e);
            break;
        }
    }

    let start = std::time::Instant::now();
    let mut slot = 0usize;

    // ---- ここから先はリアルタイム経路。連結以外の仕事をしないこと。 ----
    while result.is_ok()
        && start.elapsed().as_millis() < duration_ms as u128
        && !ctl.is_some_and(|c| c.stop.load(Relaxed))
    {
        let mut transferred = 0u32;
        if let Err(e) =
            unsafe { WinUsb_GetOverlappedResult(iface, &ovs[slot], &mut transferred, true) }
        {
            result = Err(e);
            break;
        }

        boundary_frames.push(captured.len() / FRAME_BYTES);
        if debug_desc && desc_log.len() < 6 {
            desc_log.push((
                slot,
                transferred,
                descs[slot]
                    .iter()
                    .map(|d| (d.Offset, d.Length, d.Status))
                    .collect(),
            ));
        }
        for d in &descs[slot] {
            // USBD_ISO_PACKET_DESCRIPTOR.Offset は「その転送に渡した offset からの
            // 相対値」で、登録バッファ先頭からの絶対値ではない。全スロットが
            // 0,320,640,... を返すので、スロットの基点を足さないと常にスロット 0 の
            // 領域を読むことになる。
            let off = slot * slot_bytes + d.Offset as usize;
            let len = d.Length as usize;
            lens.push(d.Length);
            if d.Status != 0 || off + len > buf.len() {
                errors += 1;
                continue;
            }
            if len == 0 {
                zero_packets += 1;
                continue;
            }
            captured.extend_from_slice(&buf[off..off + len]);
        }

        if let Some(c) = ctl {
            // 連結済みバイト数をフレーム数に直して公開するだけ。RT 経路なので
            // これ以上の仕事はここに書かないこと。
            c.frames
                .store((captured.len() / FRAME_BYTES) as u64, Relaxed);
        }

        if let Err(e) = issue(slot, &mut ovs, &mut descs, true) {
            result = Err(e);
            break;
        }
        slot = (slot + 1) % SLOTS;
    }
    let elapsed_ms = start.elapsed().as_millis() as u64;
    // ---- リアルタイム経路ここまで ----

    unsafe {
        let _ = CancelIoEx(dev.file(), None);
        for ov in &ovs {
            let mut t = 0u32;
            let _ = WinUsb_GetOverlappedResult(iface, ov, &mut t, true);
        }
        let _ = WinUsb_UnregisterIsochBuffer(iso_handle);
        let _ = WinUsb_SetCurrentAlternateSetting(iface, 0);
        for ev in &events {
            let _ = CloseHandle(*ev);
        }
    }
    result?;

    if debug_desc {
        println!("--- パケット記述子 (slot_bytes={slot_bytes}, max_packet={max_packet}) ---");
        for (s, t, ds) in &desc_log {
            println!("slot {s} transferred={t}");
            for (i, (off, len, st)) in ds.iter().enumerate() {
                println!("  [{i}] Offset={off:6} Length={len:4} Status=0x{st:08X}");
            }
        }
    }

    // ---- ここから後処理 ----
    let mut stats = CaptureStats {
        packets: lens.len() as u64,
        bytes: captured.len() as u64,
        short_packets: lens
            .iter()
            .filter(|&&l| l != 0 && (l as usize) < EXPECTED_PACKET_BYTES)
            .count() as u64,
        zero_packets,
        errors,
        len_histogram: Default::default(),
        peak: [0; CHANNELS],
        first_packet: captured[..captured.len().min(48)].to_vec(),
        elapsed_ms,
        boundaries: 0,
        boundary_glitches: 0,
    };
    for l in &lens {
        *stats.len_histogram.entry(*l).or_insert(0) += 1;
    }

    let frames = captured.len() / FRAME_BYTES;
    for f in 0..frames {
        for c in 0..CHANNELS {
            let s = frame_sample(&captured, f, c).abs();
            if s > stats.peak[c] {
                stats.peak[c] = s;
            }
        }
    }

    // 転送境界で波形が飛んでいないか。WinUSB は継ぎ目の取りこぼしを報告しない
    // ので、これが唯一の検出手段になる。
    if frames > 2 {
        let mut sum = 0f64;
        for f in 1..frames {
            sum += (frame_sample(&captured, f, 0) - frame_sample(&captured, f - 1, 0)).abs() as f64;
        }
        let mean = sum / (frames - 1) as f64;
        let thr = (mean * 8.0).max(4096.0);
        for &b in &boundary_frames {
            if b == 0 || b >= frames {
                continue;
            }
            stats.boundaries += 1;
            let step =
                (frame_sample(&captured, b, 0) - frame_sample(&captured, b - 1, 0)).abs() as f64;
            if step > thr {
                stats.boundary_glitches += 1;
            }
        }
    }

    let is_wav = out_path.to_ascii_lowercase().ends_with(".wav");
    let mut f = std::fs::File::create(out_path)
        .map_err(|e| windows::core::Error::new(E_FAIL, e.to_string()))?;
    if is_wav {
        f.write_all(&wav_header(captured.len() as u32))
            .map_err(|e| windows::core::Error::new(E_FAIL, e.to_string()))?;
    }
    f.write_all(&captured)
        .map_err(|e| windows::core::Error::new(E_FAIL, e.to_string()))?;

    Ok(stats)
}

pub fn report(stats: &CaptureStats, out_path: &str) {
    println!();
    println!("=== 取り込み結果 ===");
    println!("パケット数     : {}", stats.packets);
    println!("バイト数       : {} -> {}", stats.bytes, out_path);
    println!("長さ0のパケット: {}", stats.zero_packets);
    println!("短いパケット   : {}", stats.short_packets);
    println!("エラー         : {}", stats.errors);
    println!("パケット長の分布 (期待値 {EXPECTED_PACKET_BYTES}):");
    for (len, n) in &stats.len_histogram {
        println!("  {len:5} bytes  x {n}");
    }

    if stats.boundaries > 0 {
        let pct = 100.0 * stats.boundary_glitches as f64 / stats.boundaries as f64;
        // 判定は音楽的な過渡音と区別がつかないので、率で見る。
        // 継ぎ目が落ちていると 10% 台に跳ね上がるため、閾値は 1% で十分効く。
        let verdict = if stats.boundary_glitches == 0 {
            "OK (継ぎ目で落ちていない)"
        } else if pct < 1.0 {
            "OK (過渡音との区別がつかない水準)"
        } else {
            "NG (継ぎ目でフレームが落ちている)"
        };
        println!(
            "転送境界の不連続: {}/{} ({pct:.1}%)  {verdict}",
            stats.boundary_glitches, stats.boundaries
        );
    }

    if stats.packets > 0 {
        let frames = stats.bytes as f64 / FRAME_BYTES as f64;
        let per_packet = frames / stats.packets as f64;
        println!(
            "平均 {per_packet:.3} サンプル/パケット -> 実効 {:.0} Hz (期待値 {SAMPLE_RATE})",
            per_packet * 1000.0
        );
        println!(
            "  ({} パケット / 壁時計 {}ms)",
            stats.packets, stats.elapsed_ms
        );
    }

    if !stats.first_packet.is_empty() {
        println!("先頭 {} バイト:", stats.first_packet.len());
        for (i, c) in stats.first_packet.chunks(12).enumerate() {
            let hex: Vec<String> = c.iter().map(|b| format!("{b:02X}")).collect();
            println!("  {:04X}: {}", i * 12, hex.join(" "));
        }
    }

    let full = (1i32 << 23) as f64;
    println!("ピーク (24bit LE / 2ch):");
    for (c, p) in stats.peak.iter().enumerate() {
        let db = if *p > 0 {
            format!("{:.1} dBFS", 20.0 * (*p as f64 / full).log10())
        } else {
            "無音".to_string()
        };
        println!("  ch{c}: {p} ({db})");
    }
}
