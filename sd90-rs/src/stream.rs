//! iso IN を回し続けてリングに流し、iso OUT にリングから流し込むワーカ。
//!
//! WinUSB のハンドルは Send ではないので、デバイスはワーカスレッドの中で開き、
//! 入出力とも 1 本のスレッドで面倒を見る。起動時のエラーだけチャネルで返す。
//!
//! IN と OUT で構造が違うことに注意:
//!
//! * `WinUsb_ReadIsochPipeAsap` はパケット記述子を受け取るので、1 転送に
//!   複数フレームを詰められる (既定は 8ms 分をまとめて回収)。ただし
//!   **転送の粒度がそのまま時計の粒度**になるので、MIDI を出す側は
//!   `StreamCfg::capture_only` で 1ms に落とす。理由は `StreamCfg` の項。
//! * `WinUsb_WriteIsochPipeAsap` は**パケット記述子を取らない**。WinUSB が
//!   バッファを `wMaxPacketSize` (320B) 刻みで切ってしまうため、288B の
//!   パケットを作るには **1 転送 = 1 パケット = 1 フレーム** にするしかない。
//!   そのぶん転送本数は増えるが、パケット長を 1ms 単位で選べるので、
//!   ホストのフレームクロックとデバイスの水晶の ppm 差を 47/48/49 サンプルの
//!   出し分けで吸収できる。SD-90 が入力側でこちらにやっているのと同じこと。

use crate::device::{
    ALT_STREAMING, EP_CAPTURE_IN, EP_PLAYBACK_OUT, FRAME_BYTES, IF_CAPTURE, IF_PLAYBACK,
    PID_VENDOR, Sd90,
};
use crate::ring::Ring;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;
use windows::Win32::Devices::Usb::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::IO::{CancelIoEx, OVERLAPPED};
use windows::Win32::System::Threading::*;
use windows::core::{PCWSTR, Result, w};

/// IN: 1 転送あたりのパケット数 = ミリ秒数 (既定)。
const IN_PACKETS_PER_XFER: usize = 8;
/// IN: 同時に走らせる転送数 (既定)。8ms x 8 = 64ms 先まで積む。
const IN_SLOTS: usize = 8;

/// OUT: 1 転送 = 1 フレーム。8ms ごとにしか面倒を見られないので、
/// 取りこぼさないよう 24ms 分を常に積んでおく。
const OUT_SLOTS: usize = 24;
/// OUT: 公称のフレームあたりサンプル数。
const NOMINAL_FRAMES: usize = 48;
/// OUT: 目標水位からこれ以上ずれたら 47/49 サンプルで引き戻す。
const OUT_SERVO_MARGIN: f64 = 96.0;
/// OUT: 水位の平滑化係数 (1ms 周期で約 1 秒の時定数)。
///
/// リングの瞬時値は「ホストが bufferSize を一気に積む」「USB が 8ms ごとに
/// 384 フレームまとめて吸う」で常に大きく上下しており、その揺れ自体には
/// 情報が無い。生値でサーボすると 6 割のフレームで長さが変わる暴れ方をする。
/// 拾いたいのは ppm 単位のクロック差だけなので、平滑してから判断する。
const OUT_SERVO_ALPHA: f64 = 0.001;

/// ワーカの回し方。
///
/// `packets_per_xfer` は **`frames_captured` の更新粒度そのもの**なので、
/// MIDI の送出精度を直接決める。`latency` コマンドでの実測 (8 回 x 2 条件):
///
/// ```text
/// 8 パケット/転送 (8ms):  d 中央値 7.33ms  ばらつき 3.60〜8.77ms
/// 1 パケット/転送 (1ms):  d 中央値 3.29ms  ばらつき 2.50〜3.48ms
/// ```
///
/// 差の 4.04ms は 8ms 更新間隔の平均ラグ (半分 = 4.0ms) と一致する。
/// 時計は「最後に更新を見てからの経過」で補間するので、更新の粒度が
/// そのまま系統的な遅れとして乗り、粒度の中でランダムに揺れる。
///
/// 転送本数は 125/s から 1000/s に増えるが、OUT 側は元から 1000/s で
/// 回している (1 転送 = 1 パケットしか作れないため) ので負荷の桁は変わらない。
/// ASIO ドライバは実測値が README にある構成のままにしてあるので既定は 8。
#[derive(Clone, Copy)]
pub struct StreamCfg {
    /// iso OUT (IF0) を回すか。VST3 プラグインは音を送り込まないので false。
    /// 回さなければ IF0 の alt を上げずに済み、フルスピードの帯域も食わない。
    pub playback: bool,
    /// IN: 1 転送あたりのパケット数 = ミリ秒数。
    pub packets_per_xfer: usize,
    /// IN: 同時に走らせる転送数。`packets_per_xfer` との積が先読みのミリ秒。
    pub slots: usize,
}

impl Default for StreamCfg {
    fn default() -> Self {
        Self {
            playback: true,
            packets_per_xfer: IN_PACKETS_PER_XFER,
            slots: IN_SLOTS,
        }
    }
}

impl StreamCfg {
    /// VST3 プラグイン用。送出せず、時計を 1ms 粒度で刻む。先読みは 64ms のまま。
    pub fn capture_only() -> Self {
        Self {
            playback: false,
            packets_per_xfer: 1,
            slots: 64,
        }
    }
}

pub struct Shared {
    /// SD-90 -> ホスト
    pub ring: Ring,
    /// ホスト -> SD-90
    pub out_ring: Ring,
    /// 取り込んだ総フレーム数 = デバイスの水晶が刻んだ時間そのもの。
    ///
    /// リングが溢れて捨てた分も数える (時計であってデータ量ではない)。
    /// 更新は 1 転送に 1 回なので、粒度は `StreamCfg::packets_per_xfer` ミリ秒。
    /// MIDI の送出位置はこれを基準にする。
    pub frames_captured: AtomicU64,
    /// 到着通知。値は世代カウンタ (spurious wakeup 対策)。
    pub notify: (Mutex<u64>, Condvar),
    pub stop: AtomicBool,
    /// サーボが 47 / 49 サンプルに振った回数 (診断用)。
    pub out_short: AtomicUsize,
    pub out_long: AtomicUsize,
    /// OUT を流し始める前に溜めておくフレーム数。ASIO 側が bufferSize を入れる。
    ///
    /// ホストは bufferSize 単位のバースト (例: 512 フレームを 10.7ms ごと) で
    /// 供給するのに対し、USB は 48 フレーム/ms で一定に吸い出す。助走なしだと
    /// バーストの谷でリングが底を打ち、1ms 単位の無音が混ざる。
    pub out_prefill: AtomicUsize,
}

impl Shared {
    pub fn new(ring_frames: usize) -> Self {
        Self {
            ring: Ring::new(ring_frames, FRAME_BYTES),
            out_ring: Ring::new(ring_frames, FRAME_BYTES),
            frames_captured: AtomicU64::new(0),
            notify: (Mutex::new(0), Condvar::new()),
            stop: AtomicBool::new(false),
            out_short: AtomicUsize::new(0),
            out_long: AtomicUsize::new(0),
            out_prefill: AtomicUsize::new(512),
        }
    }

    pub fn wake(&self) {
        let (m, cv) = &self.notify;
        if let Ok(mut g) = m.lock() {
            *g = g.wrapping_add(1);
        }
        cv.notify_all();
    }
}

/// overlapped I/O の「まだ完了していない」を正常系として拾い直す。
fn ok_or_pending(r: Result<()>) -> Result<()> {
    match r {
        Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => Ok(()),
        other => other,
    }
}

fn is_incomplete(e: &windows::core::Error) -> bool {
    e.code() == ERROR_IO_INCOMPLETE.to_hresult() || e.code() == ERROR_IO_PENDING.to_hresult()
}

fn join_pro_audio() -> Option<HANDLE> {
    let mut idx = 0u32;
    unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut idx) }.ok()
}

fn max_packet_of(iface: WINUSB_INTERFACE_HANDLE, ep: u8) -> Option<usize> {
    for idx in 0..8u8 {
        let mut info = WINUSB_PIPE_INFORMATION::default();
        if unsafe { WinUsb_QueryPipe(iface, ALT_STREAMING, idx, &mut info) }.is_ok()
            && info.PipeId == ep
        {
            return Some(info.MaximumPacketSize as usize);
        }
    }
    None
}

pub struct InputStream {
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

impl InputStream {
    /// iso IN / OUT を両方回す (ASIO ドライバ用)。デバイスはワーカが自分で開く。
    pub fn start(shared: Arc<Shared>) -> std::result::Result<Self, String> {
        Self::start_cfg(shared, StreamCfg::default(), None)
    }

    /// `dev` を渡すと、そのハンドルを共有する。MIDI (IF2) を同じデバイスで
    /// 同時に叩きたい場合はこちらを使う。渡さなければワーカが自分で開く。
    pub fn start_cfg(
        shared: Arc<Shared>,
        cfg: StreamCfg,
        dev: Option<Arc<Sd90>>,
    ) -> std::result::Result<Self, String> {
        shared.stop.store(false, Ordering::Release);
        let (tx, rx) = mpsc::channel::<std::result::Result<(), String>>();
        let s = shared.clone();
        let handle = std::thread::Builder::new()
            .name("sd90-usb".into())
            .spawn(move || {
                if let Err(e) = worker(&s, &tx, cfg, dev) {
                    let _ = tx.send(Err(format!("{e}")));
                }
            })
            .map_err(|e| format!("スレッドを起動できない: {e}"))?;

        match rx.recv_timeout(std::time::Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self {
                shared,
                handle: Some(handle),
            }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                shared.stop.store(true, Ordering::Release);
                let _ = handle.join();
                Err("ストリームが起動しない (タイムアウト)".into())
            }
        }
    }
}

impl Drop for InputStream {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.wake();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// OUT 側の一式。capture-only のときは丸ごと作らない。
struct OutSide {
    iso: *mut c_void,
    buf: Vec<u8>,
    events: Vec<HANDLE>,
    ovs: Vec<OVERLAPPED>,
    /// wMaxPacketSize。1 スロットの確保幅でもある。
    max: usize,
    /// 次に面倒を見るスロット。
    slot: usize,
    /// 助走が済んだか。
    primed: bool,
    /// リング滞留量の平滑値。
    ema: f64,
}

/// 目標水位 `target` に対するサーボ。溜まりすぎなら 49、枯れかけなら 47 を
/// 送ってホストのフレームクロックとデバイスの水晶のずれを吸収する。
///
/// 閾値を絶対値で置くと、助走量より小さかった場合に常時 49 を送り続けて
/// 自分でリングを枯らす。必ず目標水位からの相対で見ること。
fn frames_for(level: f64, target: usize) -> usize {
    let t = target as f64;
    if level > t + OUT_SERVO_MARGIN {
        NOMINAL_FRAMES + 1
    } else if level + OUT_SERVO_MARGIN < t {
        NOMINAL_FRAMES - 1
    } else {
        NOMINAL_FRAMES
    }
}

impl OutSide {
    /// スロットを埋めて長さ (バイト) を返す。助走が済むまでは無音を送る。
    fn fill(&mut self, slot: usize, shared: &Arc<Shared>) -> usize {
        let avail = shared.out_ring.available();
        let target = shared.out_prefill.load(Ordering::Relaxed);
        self.ema += (avail as f64 - self.ema) * OUT_SERVO_ALPHA;
        let n = frames_for(self.ema, target);
        if n > NOMINAL_FRAMES {
            shared.out_long.fetch_add(1, Ordering::Relaxed);
        } else if n < NOMINAL_FRAMES {
            shared.out_short.fetch_add(1, Ordering::Relaxed);
        }
        let base = slot * self.max;
        let bytes = n * FRAME_BYTES;
        let dst = &mut self.buf[base..base + bytes];

        if !self.primed {
            // 規定量が溜まるまでは吸い出さずに無音を流す。
            if avail >= target.max(n) {
                self.primed = true;
            } else {
                dst.fill(0);
                return bytes;
            }
        }
        if avail >= n {
            let mut off = 0usize;
            shared.out_ring.consume(n, |part| {
                dst[off..off + part.len()].copy_from_slice(part);
                off += part.len();
            });
        } else {
            // 底を打った。無音で埋め、助走からやり直す。
            dst.fill(0);
            shared.out_ring.note_underrun();
            self.primed = false;
        }
        bytes
    }

    /// 1 転送 = 1 パケット。len は 47/48/49 サンプル分のいずれか。
    fn issue(&self, slot: usize, len: usize, cont: bool) -> Result<()> {
        unsafe {
            ResetEvent(self.ovs[slot].hEvent)?;
            ok_or_pending(WinUsb_WriteIsochPipeAsap(
                self.iso,
                (slot * self.max) as u32,
                len as u32,
                cont,
                Some(&self.ovs[slot]),
            ))
        }
    }
}

fn worker(
    shared: &Arc<Shared>,
    tx: &mpsc::Sender<std::result::Result<(), String>>,
    cfg: StreamCfg,
    dev: Option<Arc<Sd90>>,
) -> Result<()> {
    let dev = match dev {
        Some(d) => d,
        None => Arc::new(Sd90::open(PID_VENDOR)?),
    };
    let cap = dev.iface(IF_CAPTURE);
    let play = dev.iface(IF_PLAYBACK);

    // alt 0 はエンドポイント無し。alt 1 に上げて初めてストリームが立つ。
    // 送出しないなら IF0 は alt 0 のまま (帯域を予約させない)。
    unsafe {
        WinUsb_SetCurrentAlternateSetting(cap, ALT_STREAMING)?;
        if cfg.playback {
            WinUsb_SetCurrentAlternateSetting(play, ALT_STREAMING)?;
        }
    }

    let restore = || unsafe {
        let _ = WinUsb_SetCurrentAlternateSetting(cap, 0);
        if cfg.playback {
            let _ = WinUsb_SetCurrentAlternateSetting(play, 0);
        }
    };

    let Some(in_max) = max_packet_of(cap, EP_CAPTURE_IN) else {
        restore();
        return Err(windows::core::Error::new(
            ERROR_NOT_FOUND.to_hresult(),
            "iso エンドポイントが見つからない",
        ));
    };
    let out_max = if cfg.playback {
        match max_packet_of(play, EP_PLAYBACK_OUT) {
            Some(m) => m,
            None => {
                restore();
                return Err(windows::core::Error::new(
                    ERROR_NOT_FOUND.to_hresult(),
                    "iso エンドポイントが見つからない",
                ));
            }
        }
    } else {
        0
    };

    // ---- IN 側の準備 ----
    let in_slot_bytes = cfg.packets_per_xfer * in_max;
    let mut in_buf = vec![0u8; cfg.slots * in_slot_bytes];
    let mut in_iso: *mut c_void = std::ptr::null_mut();
    unsafe { WinUsb_RegisterIsochBuffer(cap, EP_CAPTURE_IN, &mut in_buf, &mut in_iso)? };

    let mut in_events: Vec<HANDLE> = Vec::with_capacity(cfg.slots);
    let mut in_ovs: Vec<OVERLAPPED> = Vec::with_capacity(cfg.slots);
    let mut in_descs: Vec<Vec<USBD_ISO_PACKET_DESCRIPTOR>> = Vec::with_capacity(cfg.slots);
    for _ in 0..cfg.slots {
        let ev = unsafe { CreateEventW(None, true, false, PCWSTR::null())? };
        in_events.push(ev);
        in_ovs.push(OVERLAPPED {
            hEvent: ev,
            ..Default::default()
        });
        in_descs.push(vec![
            USBD_ISO_PACKET_DESCRIPTOR::default();
            cfg.packets_per_xfer
        ]);
    }

    // ---- OUT 側の準備 (1 スロット = 1 フレーム) ----
    let mut out = if cfg.playback {
        let mut out_buf = vec![0u8; OUT_SLOTS * out_max];
        let mut out_iso: *mut c_void = std::ptr::null_mut();
        unsafe { WinUsb_RegisterIsochBuffer(play, EP_PLAYBACK_OUT, &mut out_buf, &mut out_iso)? };

        let mut out_events: Vec<HANDLE> = Vec::with_capacity(OUT_SLOTS);
        let mut out_ovs: Vec<OVERLAPPED> = Vec::with_capacity(OUT_SLOTS);
        for _ in 0..OUT_SLOTS {
            let ev = unsafe { CreateEventW(None, true, false, PCWSTR::null())? };
            out_events.push(ev);
            out_ovs.push(OVERLAPPED {
                hEvent: ev,
                ..Default::default()
            });
        }
        Some(OutSide {
            iso: out_iso,
            buf: out_buf,
            events: out_events,
            ovs: out_ovs,
            max: out_max,
            slot: 0,
            primed: false,
            ema: shared.out_prefill.load(Ordering::Relaxed) as f64,
        })
    } else {
        None
    };

    let _mmcss = join_pro_audio();

    let issue_in = |slot: usize,
                    ovs: &mut [OVERLAPPED],
                    descs: &mut [Vec<USBD_ISO_PACKET_DESCRIPTOR>],
                    cont: bool|
     -> Result<()> {
        unsafe {
            ResetEvent(ovs[slot].hEvent)?;
            ok_or_pending(WinUsb_ReadIsochPipeAsap(
                in_iso,
                (slot * in_slot_bytes) as u32,
                in_slot_bytes as u32,
                cont,
                &mut descs[slot],
                Some(&ovs[slot]),
            ))
        }
    };

    let mut result: Result<()> = Ok(());
    for slot in 0..cfg.slots {
        if let Err(e) = issue_in(slot, &mut in_ovs, &mut in_descs, slot != 0) {
            result = Err(e);
            break;
        }
    }
    // OUT は最初は無音で埋めて走らせる (ホストがまだ何も出していないため)。
    if result.is_ok()
        && let Some(out) = &mut out
    {
        for slot in 0..OUT_SLOTS {
            let len = out.fill(slot, shared);
            if let Err(e) = out.issue(slot, len, slot != 0) {
                result = Err(e);
                break;
            }
        }
    }

    let _ = tx.send(match &result {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("{e}")),
    });

    let mut in_slot = 0usize;

    while result.is_ok() && !shared.stop.load(Ordering::Acquire) {
        // ---- IN: 8ms 分をブロッキングで回収 ----
        let mut transferred = 0u32;
        if let Err(e) =
            unsafe { WinUsb_GetOverlappedResult(cap, &in_ovs[in_slot], &mut transferred, true) }
        {
            result = Err(e);
            break;
        }
        let mut got = 0usize;
        for d in &in_descs[in_slot] {
            // Offset は転送相対。スロットの基点を足すこと (README 参照)。
            let off = in_slot * in_slot_bytes + d.Offset as usize;
            let len = d.Length as usize;
            if d.Status != 0 || len == 0 || off + len > in_buf.len() {
                continue;
            }
            shared.ring.push(&in_buf[off..off + len]);
            got += len / FRAME_BYTES;
        }
        // 時計はリングの受け入れ可否と無関係に進める (溢れて捨てた分も時間は経つ)。
        shared
            .frames_captured
            .fetch_add(got as u64, Ordering::Release);
        shared.wake();
        if let Err(e) = issue_in(in_slot, &mut in_ovs, &mut in_descs, true) {
            result = Err(e);
            break;
        }
        in_slot = (in_slot + 1) % cfg.slots;

        // ---- OUT: 完了したぶんだけ詰め直して投げ返す (非ブロッキング) ----
        if let Some(out) = &mut out {
            for _ in 0..OUT_SLOTS {
                let slot = out.slot;
                let mut t = 0u32;
                match unsafe { WinUsb_GetOverlappedResult(play, &out.ovs[slot], &mut t, false) } {
                    Ok(()) => {}
                    Err(e) if is_incomplete(&e) => break, // まだ飛んでいない
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
                let len = out.fill(slot, shared);
                if let Err(e) = out.issue(slot, len, true) {
                    result = Err(e);
                    break;
                }
                out.slot = (slot + 1) % OUT_SLOTS;
            }
        }
    }

    let (out_ovs, out_events, out_iso) = match out {
        Some(o) => (o.ovs, o.events, Some(o.iso)),
        None => (Vec::new(), Vec::new(), None),
    };
    unsafe {
        let _ = CancelIoEx(dev.file(), None);
        for ov in in_ovs.iter().chain(out_ovs.iter()) {
            let mut t = 0u32;
            let _ = WinUsb_GetOverlappedResult(cap, ov, &mut t, true);
        }
        let _ = WinUsb_UnregisterIsochBuffer(in_iso);
        if let Some(iso) = out_iso {
            let _ = WinUsb_UnregisterIsochBuffer(iso);
        }
        restore();
        for ev in in_events.iter().chain(out_events.iter()) {
            let _ = CloseHandle(*ev);
        }
    }
    result
}
