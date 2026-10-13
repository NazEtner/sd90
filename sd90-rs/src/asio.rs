//! ASIO ドライバ本体 (入力 2ch 専用)。
//!
//! ASIO は COM 風の in-proc サーバだが、標準的な COM ではないところが 2 つある。
//!   * ホストは `CoCreateInstance(clsid, .., clsid, ..)` と **IID に CLSID を渡す**。
//!     したがって QueryInterface は CLSID も受け付ける必要がある。
//!   * vtable は IUnknown の 3 メソッドに続いて ASIO の 20 メソッドが並ぶだけ。
//!     x64 には呼び出し規約が 1 つしかないので、`extern "system"` で一致する
//!     (x86 の __thiscall 問題は考えなくてよい)。
//!
//! 出力は持たない (`getChannels` で outputs=0)。SD-90 を鳴らすのは MIDI 側の
//! 役目で、こちらは録音専用という割り切り。

use crate::device::{CHANNELS, FRAME_BYTES, PID_VENDOR, Sd90};
use crate::stream::{InputStream, Shared};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{E_NOINTERFACE, S_OK};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{AvSetMmThreadCharacteristicsW, INFINITE};
use windows::core::{GUID, HRESULT, w};

pub const CLSID_SD90_ASIO: GUID = GUID::from_u128(0x7e1a5c42_9b3d_4f86_a1c7_5d2e0b4f93a8);
pub const DRIVER_NAME: &str = "SD-90 (WinUSB)";

const ASIO_FALSE: i32 = 0;
const ASIO_TRUE: i32 = 1;

const ASE_OK: i32 = 0;
const ASE_NOT_PRESENT: i32 = -1000;
const ASE_INVALID_PARAMETER: i32 = -998;
const ASE_INVALID_MODE: i32 = -997;
const ASE_NO_CLOCK: i32 = -995;
const ASE_NO_MEMORY: i32 = -994;

/// 32bit 整数 LSB。ネイティブは 24bit だが、Int24LSB より対応ホストが広い。
/// どのみち deinterleave で全サンプルに触るので、8bit 左シフトの追加コストは無い。
const ASIOST_INT32LSB: i32 = 18;

const SAMPLE_RATE: f64 = 48_000.0;
const MIN_BUFFER: i32 = 128;
const MAX_BUFFER: i32 = 8192;
const PREFERRED_BUFFER: i32 = 512;
/// USB 転送 1 本ぶん (8ms) を固定の入力遅延として申告する。
const USB_LATENCY_FRAMES: i32 = 384;
/// OUT 側は 1 フレーム単位の転送を 24 本積むので 24ms ぶん。
const OUT_QUEUE_FRAMES: i32 = 24 * 48;

/// OUT リングの目標水位。ホストのバースト (bufferSize) と USB 側の
/// まとめ吸い出し (8ms = 384 フレーム) の両方を飲み込める深さにする。
fn out_prefill_frames(buffer_size: usize) -> usize {
    (buffer_size * 2).max(1024)
}
/// リングの容量 (フレーム)。約 341ms。
const RING_FRAMES: usize = 16384;

// ---- ASIO の構造体定義 ----------------------------------------------------

/// Windows の ASIO SDK は NATIVE_INT64=0 なので、64bit 値は hi/lo の 2 ワード。
/// u64 をそのまま書くと上下が逆になるので注意。
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct AsioSamples {
    pub hi: u32,
    pub lo: u32,
}

impl AsioSamples {
    fn from_u64(v: u64) -> Self {
        Self {
            hi: (v >> 32) as u32,
            lo: v as u32,
        }
    }
}

#[repr(C)]
pub struct AsioChannelInfo {
    pub channel: i32,
    pub is_input: i32,
    pub is_active: i32,
    pub channel_group: i32,
    pub sample_type: i32,
    pub name: [u8; 32],
}

#[repr(C)]
pub struct AsioBufferInfo {
    pub is_input: i32,
    pub channel_num: i32,
    pub buffers: [*mut c_void; 2],
}

#[repr(C)]
pub struct AsioClockSource {
    pub index: i32,
    pub associated_channel: i32,
    pub associated_group: i32,
    pub is_current_source: i32,
    pub name: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AsioCallbacks {
    pub buffer_switch: Option<unsafe extern "system" fn(i32, i32)>,
    pub sample_rate_did_change: Option<unsafe extern "system" fn(f64)>,
    pub asio_message: Option<unsafe extern "system" fn(i32, i32, *mut c_void, *mut f64) -> i32>,
    pub buffer_switch_time_info:
        Option<unsafe extern "system" fn(*mut c_void, i32, i32) -> *mut c_void>,
}

// ---- vtable ---------------------------------------------------------------

#[repr(C)]
pub struct IAsioVtbl {
    query_interface:
        unsafe extern "system" fn(*mut AsioDriver, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut AsioDriver) -> u32,
    release: unsafe extern "system" fn(*mut AsioDriver) -> u32,

    init: unsafe extern "system" fn(*mut AsioDriver, *mut c_void) -> i32,
    get_driver_name: unsafe extern "system" fn(*mut AsioDriver, *mut u8),
    get_driver_version: unsafe extern "system" fn(*mut AsioDriver) -> i32,
    get_error_message: unsafe extern "system" fn(*mut AsioDriver, *mut u8),
    start: unsafe extern "system" fn(*mut AsioDriver) -> i32,
    stop: unsafe extern "system" fn(*mut AsioDriver) -> i32,
    get_channels: unsafe extern "system" fn(*mut AsioDriver, *mut i32, *mut i32) -> i32,
    get_latencies: unsafe extern "system" fn(*mut AsioDriver, *mut i32, *mut i32) -> i32,
    get_buffer_size:
        unsafe extern "system" fn(*mut AsioDriver, *mut i32, *mut i32, *mut i32, *mut i32) -> i32,
    can_sample_rate: unsafe extern "system" fn(*mut AsioDriver, f64) -> i32,
    get_sample_rate: unsafe extern "system" fn(*mut AsioDriver, *mut f64) -> i32,
    set_sample_rate: unsafe extern "system" fn(*mut AsioDriver, f64) -> i32,
    get_clock_sources:
        unsafe extern "system" fn(*mut AsioDriver, *mut AsioClockSource, *mut i32) -> i32,
    set_clock_source: unsafe extern "system" fn(*mut AsioDriver, i32) -> i32,
    get_sample_position:
        unsafe extern "system" fn(*mut AsioDriver, *mut AsioSamples, *mut AsioSamples) -> i32,
    get_channel_info: unsafe extern "system" fn(*mut AsioDriver, *mut AsioChannelInfo) -> i32,
    create_buffers: unsafe extern "system" fn(
        *mut AsioDriver,
        *mut AsioBufferInfo,
        i32,
        i32,
        *const AsioCallbacks,
    ) -> i32,
    dispose_buffers: unsafe extern "system" fn(*mut AsioDriver) -> i32,
    control_panel: unsafe extern "system" fn(*mut AsioDriver) -> i32,
    future: unsafe extern "system" fn(*mut AsioDriver, i32, *mut c_void) -> i32,
    output_ready: unsafe extern "system" fn(*mut AsioDriver) -> i32,
}

static VTBL: IAsioVtbl = IAsioVtbl {
    query_interface,
    add_ref,
    release,
    init,
    get_driver_name,
    get_driver_version,
    get_error_message,
    start,
    stop,
    get_channels,
    get_latencies,
    get_buffer_size,
    can_sample_rate,
    get_sample_rate,
    set_sample_rate,
    get_clock_sources,
    set_clock_source,
    get_sample_position,
    get_channel_info,
    create_buffers,
    dispose_buffers,
    control_panel,
    future,
    output_ready,
};

// ---- ドライバ状態 ---------------------------------------------------------

/// createBuffers で確保した 1 チャンネル分のダブルバッファ。
struct ChanBuf {
    /// SD-90 側のチャンネル番号 (0=L, 1=R)。
    hw: usize,
    is_input: bool,
    bufs: [Box<[i32]>; 2],
}

struct Inner {
    initialized: bool,
    buffer_size: usize,
    chans: Vec<ChanBuf>,
    callbacks: Option<AsioCallbacks>,
    shared: Option<Arc<Shared>>,
    input: Option<InputStream>,
    rt_stop: Option<Arc<AtomicBool>>,
    rt_thread: Option<std::thread::JoinHandle<()>>,
    position: Arc<AtomicU64>,
    timestamp_ns: Arc<AtomicU64>,
    last_error: String,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            initialized: false,
            buffer_size: PREFERRED_BUFFER as usize,
            chans: Vec::new(),
            callbacks: None,
            shared: None,
            input: None,
            rt_stop: None,
            rt_thread: None,
            position: Arc::new(AtomicU64::new(0)),
            timestamp_ns: Arc::new(AtomicU64::new(0)),
            last_error: String::new(),
        }
    }
}

#[repr(C)]
pub struct AsioDriver {
    vtbl: *const IAsioVtbl,
    refs: AtomicU32,
    inner: Mutex<Inner>,
}

/// クラスファクトリから使う薄いラッパ。vtable を transmute で覗く必要をなくす。
///
/// # Safety
/// `obj` は `AsioDriver::new()` が返したポインタであること。
pub unsafe fn driver_query_interface(
    obj: *mut AsioDriver,
    riid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe { query_interface(obj, riid, out) }
}

/// # Safety
/// `obj` は `AsioDriver::new()` が返したポインタであること。
pub unsafe fn driver_release(obj: *mut AsioDriver) -> u32 {
    unsafe { release(obj) }
}

impl AsioDriver {
    pub fn new() -> *mut AsioDriver {
        let b = Box::new(AsioDriver {
            vtbl: &VTBL,
            refs: AtomicU32::new(1),
            inner: Mutex::new(Inner::default()),
        });
        crate::comserver::lock_module();
        Box::into_raw(b)
    }
}

fn write_cstr(dst: *mut u8, s: &str, cap: usize) {
    if dst.is_null() {
        return;
    }
    let bytes = s.as_bytes();
    let n = bytes.len().min(cap.saturating_sub(1));
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, n);
        *dst.add(n) = 0;
    }
}

fn fill_name(dst: &mut [u8; 32], s: &str) {
    dst.fill(0);
    let b = s.as_bytes();
    let n = b.len().min(31);
    dst[..n].copy_from_slice(&b[..n]);
}

fn now_ns() -> u64 {
    unsafe {
        let mut f = 0i64;
        let mut c = 0i64;
        let _ = QueryPerformanceFrequency(&mut f);
        let _ = QueryPerformanceCounter(&mut c);
        if f <= 0 {
            return 0;
        }
        (c as u128 * 1_000_000_000u128 / f as u128) as u64
    }
}

// ---- IUnknown -------------------------------------------------------------

unsafe extern "system" fn query_interface(
    this: *mut AsioDriver,
    riid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if !riid.is_null() {
        crate::log::log(&format!("query_interface riid={:?}", unsafe { *riid }));
    }
    if out.is_null() {
        return E_NOINTERFACE;
    }
    unsafe {
        *out = std::ptr::null_mut();
        if riid.is_null() {
            return E_NOINTERFACE;
        }
        // ASIO ホストは IID に CLSID を渡してくる。IUnknown も受ける。
        let iid = *riid;
        if iid == CLSID_SD90_ASIO || iid == GUID::from_u128(0x00000000_0000_0000_c000_000000000046)
        {
            add_ref(this);
            *out = this as *mut c_void;
            return S_OK;
        }
    }
    E_NOINTERFACE
}

unsafe extern "system" fn add_ref(this: *mut AsioDriver) -> u32 {
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn release(this: *mut AsioDriver) -> u32 {
    let n = unsafe { (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1 };
    if n == 0 {
        unsafe {
            let _ = stop(this);
            let _ = dispose_buffers(this);
            drop(Box::from_raw(this));
        }
        crate::comserver::unlock_module();
    }
    n
}

// ---- ASIO -----------------------------------------------------------------

unsafe extern "system" fn init(this: *mut AsioDriver, _sys_handle: *mut c_void) -> i32 {
    crate::log::log("init");
    let d = unsafe { &*this };
    let Ok(mut inner) = d.inner.lock() else {
        return ASIO_FALSE;
    };
    // 実際にデバイスが開けるかだけ確かめて、すぐ閉じる。
    // ストリームを立てるのは start() の仕事。
    match Sd90::open(PID_VENDOR) {
        Ok(dev) => {
            drop(dev);
            inner.initialized = true;
            inner.last_error.clear();
            ASIO_TRUE
        }
        Err(e) => {
            inner.last_error = format!(
                "SD-90 を開けない: {e}. \
                 本体の USB モードを Vendor にし、Zadig で WinUSB を割り当てること。"
            );
            ASIO_FALSE
        }
    }
}

unsafe extern "system" fn get_driver_name(_this: *mut AsioDriver, name: *mut u8) {
    crate::log::log("get_driver_name");
    write_cstr(name, DRIVER_NAME, 32);
}

unsafe extern "system" fn get_driver_version(_this: *mut AsioDriver) -> i32 {
    crate::log::log("get_driver_version");
    1
}

unsafe extern "system" fn get_error_message(this: *mut AsioDriver, s: *mut u8) {
    crate::log::log("get_error_message");
    let d = unsafe { &*this };
    let msg = d
        .inner
        .lock()
        .map(|i| i.last_error.clone())
        .unwrap_or_default();
    write_cstr(s, &msg, 124);
}

unsafe extern "system" fn get_channels(
    _this: *mut AsioDriver,
    inputs: *mut i32,
    outputs: *mut i32,
) -> i32 {
    crate::log::log("get_channels -> in=2 out=2");
    unsafe {
        if !inputs.is_null() {
            *inputs = CHANNELS as i32;
        }
        if !outputs.is_null() {
            // 0 を返すと SONAR (Cakewalk) は getChannels の直後に撤収する。
            // 出力を持たないドライバを受け付けない設計なので、実際に鳴らす。
            *outputs = CHANNELS as i32;
        }
    }
    ASE_OK
}

unsafe extern "system" fn get_latencies(
    this: *mut AsioDriver,
    input: *mut i32,
    output: *mut i32,
) -> i32 {
    crate::log::log("get_latencies");
    let d = unsafe { &*this };
    let bs = d
        .inner
        .lock()
        .map(|i| i.buffer_size as i32)
        .unwrap_or(PREFERRED_BUFFER);
    unsafe {
        if !input.is_null() {
            *input = bs + USB_LATENCY_FRAMES;
        }
        if !output.is_null() {
            // ホストバッファ + リングの助走 + USB キューの合計。
            *output = bs + out_prefill_frames(bs as usize) as i32 + OUT_QUEUE_FRAMES;
        }
    }
    ASE_OK
}

unsafe extern "system" fn get_buffer_size(
    _this: *mut AsioDriver,
    min: *mut i32,
    max: *mut i32,
    preferred: *mut i32,
    granularity: *mut i32,
) -> i32 {
    crate::log::log("get_buffer_size");
    unsafe {
        if !min.is_null() {
            *min = MIN_BUFFER;
        }
        if !max.is_null() {
            *max = MAX_BUFFER;
        }
        if !preferred.is_null() {
            *preferred = PREFERRED_BUFFER;
        }
        if !granularity.is_null() {
            *granularity = -1; // 2 の冪のみ
        }
    }
    ASE_OK
}

unsafe extern "system" fn can_sample_rate(_this: *mut AsioDriver, rate: f64) -> i32 {
    crate::log::log("can_sample_rate");
    // 記述子上、離散 48000Hz の 1 つだけ。
    if (rate - SAMPLE_RATE).abs() < 1.0 {
        ASE_OK
    } else {
        ASE_NO_CLOCK
    }
}

unsafe extern "system" fn get_sample_rate(_this: *mut AsioDriver, rate: *mut f64) -> i32 {
    crate::log::log("get_sample_rate");
    unsafe {
        if rate.is_null() {
            return ASE_INVALID_PARAMETER;
        }
        *rate = SAMPLE_RATE;
    }
    ASE_OK
}

unsafe extern "system" fn set_sample_rate(_this: *mut AsioDriver, rate: f64) -> i32 {
    crate::log::log("set_sample_rate");
    if (rate - SAMPLE_RATE).abs() < 1.0 || rate == 0.0 {
        ASE_OK
    } else {
        ASE_NO_CLOCK
    }
}

unsafe extern "system" fn get_clock_sources(
    _this: *mut AsioDriver,
    clocks: *mut AsioClockSource,
    num: *mut i32,
) -> i32 {
    crate::log::log("get_clock_sources");
    unsafe {
        if num.is_null() {
            return ASE_INVALID_PARAMETER;
        }
        if !clocks.is_null() && *num >= 1 {
            let c = &mut *clocks;
            c.index = 0;
            c.associated_channel = -1;
            c.associated_group = -1;
            c.is_current_source = ASIO_TRUE;
            fill_name(&mut c.name, "Internal");
        }
        *num = 1;
    }
    ASE_OK
}

unsafe extern "system" fn set_clock_source(_this: *mut AsioDriver, index: i32) -> i32 {
    crate::log::log("set_clock_source");
    if index == 0 { ASE_OK } else { ASE_NOT_PRESENT }
}

unsafe extern "system" fn get_sample_position(
    this: *mut AsioDriver,
    pos: *mut AsioSamples,
    stamp: *mut AsioSamples,
) -> i32 {
    crate::log::log("get_sample_position");
    let d = unsafe { &*this };
    let Ok(inner) = d.inner.lock() else {
        return ASE_NOT_PRESENT;
    };
    unsafe {
        if !pos.is_null() {
            *pos = AsioSamples::from_u64(inner.position.load(Ordering::Acquire));
        }
        if !stamp.is_null() {
            *stamp = AsioSamples::from_u64(inner.timestamp_ns.load(Ordering::Acquire));
        }
    }
    ASE_OK
}

unsafe extern "system" fn get_channel_info(
    this: *mut AsioDriver,
    info: *mut AsioChannelInfo,
) -> i32 {
    crate::log::log("get_channel_info");
    if info.is_null() {
        return ASE_INVALID_PARAMETER;
    }
    let d = unsafe { &*this };
    let info = unsafe { &mut *info };
    let is_input = info.is_input != ASIO_FALSE;
    let ch = info.channel;
    if ch < 0 || ch >= CHANNELS as i32 {
        return ASE_INVALID_PARAMETER;
    }
    info.channel_group = 0;
    info.sample_type = ASIOST_INT32LSB;
    info.is_active = d
        .inner
        .lock()
        .map(|i| {
            if i.chans
                .iter()
                .any(|c| c.hw == ch as usize && c.is_input == is_input)
            {
                ASIO_TRUE
            } else {
                ASIO_FALSE
            }
        })
        .unwrap_or(ASIO_FALSE);
    fill_name(
        &mut info.name,
        match (is_input, ch) {
            (true, 0) => "SD-90 In L",
            (true, _) => "SD-90 In R",
            (false, 0) => "SD-90 Out L",
            (false, _) => "SD-90 Out R",
        },
    );
    ASE_OK
}

unsafe extern "system" fn create_buffers(
    this: *mut AsioDriver,
    infos: *mut AsioBufferInfo,
    num_channels: i32,
    buffer_size: i32,
    callbacks: *const AsioCallbacks,
) -> i32 {
    crate::log::log("create_buffers");
    if infos.is_null() || callbacks.is_null() || num_channels <= 0 {
        return ASE_INVALID_PARAMETER;
    }
    if buffer_size < MIN_BUFFER
        || buffer_size > MAX_BUFFER
        || !(buffer_size as u32).is_power_of_two()
    {
        return ASE_INVALID_MODE;
    }
    let d = unsafe { &*this };
    let Ok(mut inner) = d.inner.lock() else {
        return ASE_NOT_PRESENT;
    };
    if inner.rt_thread.is_some() {
        return ASE_INVALID_MODE; // 走行中は張り替えさせない
    }
    inner.chans.clear();

    let bs = buffer_size as usize;
    let list = unsafe { std::slice::from_raw_parts_mut(infos, num_channels as usize) };
    for bi in list.iter_mut() {
        let hw = bi.channel_num;
        if hw < 0 || hw >= CHANNELS as i32 {
            inner.chans.clear();
            return ASE_INVALID_PARAMETER;
        }
        let mut a = vec![0i32; bs].into_boxed_slice();
        let mut b = vec![0i32; bs].into_boxed_slice();
        bi.buffers[0] = a.as_mut_ptr() as *mut c_void;
        bi.buffers[1] = b.as_mut_ptr() as *mut c_void;
        inner.chans.push(ChanBuf {
            hw: hw as usize,
            is_input: bi.is_input != ASIO_FALSE,
            bufs: [a, b],
        });
    }
    if inner.chans.is_empty() {
        return ASE_NO_MEMORY;
    }
    inner.buffer_size = bs;
    inner.callbacks = Some(unsafe { *callbacks });
    ASE_OK
}

unsafe extern "system" fn dispose_buffers(this: *mut AsioDriver) -> i32 {
    crate::log::log("dispose_buffers");
    let d = unsafe { &*this };
    let Ok(mut inner) = d.inner.lock() else {
        return ASE_NOT_PRESENT;
    };
    if inner.rt_thread.is_some() {
        return ASE_INVALID_MODE;
    }
    inner.chans.clear();
    inner.callbacks = None;
    ASE_OK
}

unsafe extern "system" fn start(this: *mut AsioDriver) -> i32 {
    crate::log::log("start");
    let d = unsafe { &*this };
    let Ok(mut inner) = d.inner.lock() else {
        return ASE_NOT_PRESENT;
    };
    if !inner.initialized || inner.chans.is_empty() || inner.callbacks.is_none() {
        return ASE_INVALID_MODE;
    }
    if inner.rt_thread.is_some() {
        return ASE_OK;
    }

    let shared = Arc::new(Shared::new(RING_FRAMES));
    // 助走量。リングは「ホストが bufferSize を一気に積む」のと
    // 「USB スレッドが 8ms ごとに 384 フレームまとめて吸う」のが噛み合って
    // 大きく上下する。谷が 0 を割らないよう、両方のバースト幅を足した程度は要る。
    shared.out_prefill.store(
        out_prefill_frames(inner.buffer_size),
        std::sync::atomic::Ordering::Relaxed,
    );
    let input = match InputStream::start(shared.clone()) {
        Ok(s) => s,
        Err(e) => {
            inner.last_error = e;
            return ASE_NOT_PRESENT;
        }
    };

    // RT スレッドに渡す生ポインタ。dispose_buffers は走行中を弾くので、
    // スレッドが動いている間はバッファが解放されないことが保証される。
    let mut in_plan: Vec<(usize, [*mut i32; 2])> = Vec::new();
    let mut out_plan: Vec<(usize, [*mut i32; 2])> = Vec::new();
    for c in inner.chans.iter_mut() {
        let p0 = c.bufs[0].as_mut_ptr();
        let p1 = c.bufs[1].as_mut_ptr();
        if c.is_input {
            in_plan.push((c.hw, [p0, p1]));
        } else {
            out_plan.push((c.hw, [p0, p1]));
        }
    }

    let rt = RtPlan {
        shared: shared.clone(),
        buffer_size: inner.buffer_size,
        in_chans: in_plan,
        out_chans: out_plan,
        callbacks: inner.callbacks.unwrap(),
        position: inner.position.clone(),
        timestamp_ns: inner.timestamp_ns.clone(),
        stop: Arc::new(AtomicBool::new(false)),
    };
    let stop = rt.stop.clone();
    inner.position.store(0, Ordering::Release);

    let handle = match std::thread::Builder::new()
        .name("sd90-asio-rt".into())
        .spawn(move || rt_loop(rt))
    {
        Ok(h) => h,
        Err(e) => {
            inner.last_error = format!("ASIO スレッドを起動できない: {e}");
            return ASE_NOT_PRESENT;
        }
    };

    inner.shared = Some(shared);
    inner.input = Some(input);
    inner.rt_stop = Some(stop);
    inner.rt_thread = Some(handle);
    ASE_OK
}

unsafe extern "system" fn stop(this: *mut AsioDriver) -> i32 {
    crate::log::log("stop");
    let d = unsafe { &*this };
    let Ok(mut inner) = d.inner.lock() else {
        return ASE_NOT_PRESENT;
    };
    if let Some(s) = inner.rt_stop.take() {
        s.store(true, Ordering::Release);
    }
    if let Some(sh) = &inner.shared {
        crate::log::log(&format!(
            "ring stats: in(overrun={}) out(overrun={} underrun={} servo47={} servo49={})",
            sh.ring.overruns(),
            sh.out_ring.overruns(),
            sh.out_ring.underruns(),
            sh.out_short.load(std::sync::atomic::Ordering::Relaxed),
            sh.out_long.load(std::sync::atomic::Ordering::Relaxed)
        ));
        sh.wake();
    }
    if let Some(h) = inner.rt_thread.take() {
        let _ = h.join();
    }
    // InputStream の Drop が USB スレッドを畳む。
    inner.input = None;
    inner.shared = None;
    ASE_OK
}

unsafe extern "system" fn control_panel(_this: *mut AsioDriver) -> i32 {
    crate::log::log("control_panel");
    // 設定項目が無い (レートもチャンネル数も固定)。
    ASE_NOT_PRESENT
}

unsafe extern "system" fn future(_this: *mut AsioDriver, _selector: i32, _opt: *mut c_void) -> i32 {
    crate::log::log("future");
    ASE_NOT_PRESENT
}

unsafe extern "system" fn output_ready(_this: *mut AsioDriver) -> i32 {
    crate::log::log("output_ready");
    ASE_NOT_PRESENT // 出力を持たない
}

// ---- RT スレッド ----------------------------------------------------------

struct RtPlan {
    shared: Arc<Shared>,
    buffer_size: usize,
    in_chans: Vec<(usize, [*mut i32; 2])>,
    out_chans: Vec<(usize, [*mut i32; 2])>,
    callbacks: AsioCallbacks,
    position: Arc<AtomicU64>,
    timestamp_ns: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
}

// 生ポインタの実体は createBuffers が確保したバッファで、
// RT スレッドの生存期間中は解放されない。
unsafe impl Send for RtPlan {}

fn rt_loop(rt: RtPlan) {
    let mut task = 0u32;
    let _mmcss = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task) };

    let bs = rt.buffer_size;
    // リングの折り返しをまたぐので、いったん連続領域に取り出してから配る。
    let mut scratch = vec![0u8; bs * FRAME_BYTES];
    // ホストが埋めた出力バッファを 24bit インタリーブに畳む先。
    let mut out_scratch = vec![0u8; bs * FRAME_BYTES];
    let mut index = 0usize;
    let mut pos: u64 = 0;

    let Some(buffer_switch) = rt.callbacks.buffer_switch else {
        return;
    };

    while !rt.stop.load(Ordering::Acquire) {
        if rt.shared.ring.available() < bs {
            let (m, cv) = &rt.shared.notify;
            let Ok(g) = m.lock() else { break };
            // 取りこぼしても次の到着で回るので、タイムアウト付きで待つ。
            let _ = cv.wait_timeout(g, std::time::Duration::from_millis(100));
            continue;
        }

        let mut off = 0usize;
        rt.shared.ring.consume(bs, |part| {
            scratch[off..off + part.len()].copy_from_slice(part);
            off += part.len();
        });

        // 24bit LE パック -> Int32LSB へ展開しつつ deinterleave。
        for (hw, bufs) in &rt.in_chans {
            let dst = unsafe { std::slice::from_raw_parts_mut(bufs[index], bs) };
            let base = hw * 3;
            for (f, d) in dst.iter_mut().enumerate() {
                let o = f * FRAME_BYTES + base;
                let v = (scratch[o] as u32)
                    | ((scratch[o + 1] as u32) << 8)
                    | ((scratch[o + 2] as u32) << 16);
                *d = (v << 8) as i32;
            }
        }

        pos += bs as u64;
        rt.timestamp_ns.store(now_ns(), Ordering::Release);
        rt.position.store(pos, Ordering::Release);

        // bufferSwitch の中でホストが出力バッファ[index] を埋める規約なので、
        // 戻ってきた直後に回収する。入力と同じフレーム数だけ出すことで、
        // 出力レートは自動的にデバイスのクロックに追従する。
        unsafe { buffer_switch(index as i32, ASIO_TRUE) };

        if !rt.out_chans.is_empty() {
            if rt.out_chans.len() != CHANNELS {
                out_scratch.fill(0); // 要求されなかった側は無音
            }
            for (hw, bufs) in &rt.out_chans {
                let src = unsafe { std::slice::from_raw_parts(bufs[index], bs) };
                let base = hw * 3;
                for (f, v) in src.iter().enumerate() {
                    // Int32LSB -> 24bit パック
                    let s = (*v >> 8) as u32;
                    let o = f * FRAME_BYTES + base;
                    out_scratch[o] = s as u8;
                    out_scratch[o + 1] = (s >> 8) as u8;
                    out_scratch[o + 2] = (s >> 16) as u8;
                }
            }
            rt.shared.out_ring.push(&out_scratch);
        }

        index ^= 1;
    }

    let _ = INFINITE; // (未使用定数の握り潰し回避用)
}
