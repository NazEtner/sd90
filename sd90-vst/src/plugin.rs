//! プラグイン本体。`IComponent` / `IAudioProcessor` / `IEditController` /
//! `IMidiMapping` を 1 つのオブジェクトに同居させる (single component effect)。
//!
//! C++ の多重継承と同じレイアウトを手で作る。vtable ポインタを構造体の先頭に
//! 並べ、各メソッドは受け取った `this` から自分のスロットのオフセットを引いて
//! オブジェクト先頭に戻る。x64 には呼び出し規約が 1 つしかないので
//! `extern "system"` で一致する。
//!
//! `getControllerClassId` は `kNotImplemented` を返す。ホストはそれを見て
//! コンポーネント自身に `IEditController` を問い合わせに来る (SDK の
//! `SingleComponentEffect` と同じ経路)。

use crate::control::{Control, Pacer};
use crate::engine::{CABLES, Engine, safety_for};
use crate::resample::Reader;
use crate::sd90::{Model, Part};
use crate::vst3::*;
use sd90_rs::device::{CHANNELS, SAMPLE_RATE};
use sd90_rs::latency::MODULE_RESPONSE_FRAMES;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, TryLockError};

/// このプラグインの CLSID。登録後は絶対に変えないこと。
pub const CID_SD90: Tuid = uid(0x5D903A17, 0x4C9E4B6E, 0x9F2A1C55, 0x0E7B3D42);
pub const PLUGIN_NAME: &str = "SD-90";
pub const VENDOR: &str = "sd90-rs";
pub const VERSION: &str = "0.1.0";

/// イベント入力バス。index がそのまま USB-MIDI の cable になる。
const EVENT_BUSES: [&str; 4] = ["PART A", "PART B", "MIDI OUT 1", "MIDI OUT 2"];
/// 1 バスあたりの MIDI チャンネル数。
const MIDI_CHANNELS: i32 = 16;
/// CC 0..127 に加えてアフタータッチ (128) とピッチベンド (129)。
const CTRL_COUNT: i32 = K_COUNT_CTRL_NUMBER;

/// ホストが `setupProcessing` を呼ぶ前に聞いてくるレイテンシの既定値。
const DEFAULT_BLOCK: usize = 512;

// ---- パラメータ ID ---------------------------------------------------------
//
// CC / ピッチベンド / アフタータッチはイベントでは来ない。VST3 はこれらを
// パラメータ変更に変換して渡すので、(バス, チャンネル, コントローラ番号) の
// 組それぞれに ID を割り当てて `IMidiMapping` で教える必要がある。
// ここを省くと音色すら切り替わらない。

fn param_id(bus: i32, channel: i32, ctrl: i32) -> u32 {
    ((bus as u32) << 12) | ((channel as u32) << 8) | (ctrl as u32)
}

fn param_decode(id: u32) -> Option<(usize, u8, i32)> {
    let bus = (id >> 12) as usize;
    let ch = ((id >> 8) & 0xF) as u8;
    let ctrl = (id & 0xFF) as i32;
    if bus >= EVENT_BUSES.len() || ctrl >= CTRL_COUNT {
        return None;
    }
    Some((bus, ch, ctrl))
}

fn param_count() -> i32 {
    EVENT_BUSES.len() as i32 * MIDI_CHANNELS * CTRL_COUNT
}

// ---- 状態 -----------------------------------------------------------------

struct State {
    engine: Option<Arc<Engine>>,
    reader: Option<Reader>,
    /// `setupProcessing` で受け取った最大ブロック長 (ホストのフレーム)。
    max_block: usize,
    /// ホストのサンプリングレート。
    rate: f64,
    /// 目標滞留量 B (デバイスフレーム)。
    safety: usize,
    /// リアルタイム処理か。オフラインでは音を返せない。
    realtime: bool,
    handler: *mut IComponentHandler,
    active: bool,
    /// 直近の失敗理由 (ログ用)。
    error: Option<String>,
    /// エディタからの送信の枠。
    pacer: Pacer,
}

impl State {
    fn new() -> Self {
        Self {
            engine: None,
            reader: None,
            max_block: DEFAULT_BLOCK,
            rate: SAMPLE_RATE as f64,
            safety: safety_for(DEFAULT_BLOCK),
            realtime: true,
            handler: std::ptr::null_mut(),
            active: false,
            error: None,
            pacer: Pacer::default(),
        }
    }

    /// ホスト 1 フレームあたりのデバイスフレーム数。
    fn nominal(&self) -> f64 {
        SAMPLE_RATE as f64 / self.rate
    }

    /// 申告レイテンシ `L = B + d` をホストのフレームで。B と d はデバイスフレーム。
    fn latency(&self) -> u32 {
        ((self.safety + MODULE_RESPONSE_FRAMES as usize) as f64 / self.nominal()).ceil() as u32
    }

    /// ホストのレートとブロック長から B を決め直す。
    fn update_safety(&mut self) {
        let dev_block = (self.max_block as f64 * self.nominal()).ceil() as usize;
        self.safety = safety_for(dev_block);
    }

    fn new_reader(&self) -> Reader {
        Reader::new(self.safety, self.max_block, self.nominal())
    }
}

#[repr(C)]
pub struct Plugin {
    component_vtbl: *const IComponentVtbl,
    processor_vtbl: *const IAudioProcessorVtbl,
    controller_vtbl: *const IEditControllerVtbl,
    midimap_vtbl: *const IMidiMappingVtbl,
    refs: AtomicU32,
    state: Mutex<State>,
    /// エディタと共有する本体の設定と送信待ち。
    ctrl: Arc<Control>,
}

/// `this` (= vtable ポインタが置かれているアドレス) からオブジェクト先頭に戻る。
macro_rules! obj {
    ($this:expr, $field:ident) => {
        ($this as *mut u8).sub(std::mem::offset_of!(Plugin, $field)) as *mut Plugin
    };
}

impl Plugin {
    pub fn new() -> *mut Plugin {
        let p = Box::new(Plugin {
            component_vtbl: &COMPONENT_VTBL,
            processor_vtbl: &PROCESSOR_VTBL,
            controller_vtbl: &CONTROLLER_VTBL,
            midimap_vtbl: &MIDIMAP_VTBL,
            refs: AtomicU32::new(1),
            state: Mutex::new(State::new()),
            ctrl: Arc::new(Control::default()),
        });
        Box::into_raw(p)
    }

    unsafe fn query(this: *mut Plugin, iid: *const Tuid, out: *mut *mut c_void) -> Tresult {
        if out.is_null() || iid.is_null() {
            return K_INVALID_ARGUMENT;
        }
        unsafe {
            *out = std::ptr::null_mut();
            // FUnknown / IPluginBase / IComponent はすべて先頭の vtable で足りる。
            let p = if tuid_eq(iid, &IID_FUNKNOWN)
                || tuid_eq(iid, &IID_IPLUGIN_BASE)
                || tuid_eq(iid, &IID_ICOMPONENT)
                || tuid_eq(iid, &CID_SD90)
            {
                std::ptr::addr_of!((*this).component_vtbl) as *mut c_void
            } else if tuid_eq(iid, &IID_IAUDIO_PROCESSOR) {
                std::ptr::addr_of!((*this).processor_vtbl) as *mut c_void
            } else if tuid_eq(iid, &IID_IEDIT_CONTROLLER) {
                std::ptr::addr_of!((*this).controller_vtbl) as *mut c_void
            } else if tuid_eq(iid, &IID_IMIDI_MAPPING) {
                std::ptr::addr_of!((*this).midimap_vtbl) as *mut c_void
            } else {
                return K_NO_INTERFACE;
            };
            Self::add_ref(this);
            *out = p;
            K_RESULT_OK
        }
    }

    unsafe fn add_ref(this: *mut Plugin) -> u32 {
        unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
    }

    unsafe fn release(this: *mut Plugin) -> u32 {
        let n = unsafe { (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1 };
        if n == 0 {
            unsafe { drop(Box::from_raw(this)) };
        }
        n
    }
}

// ---- IComponent -----------------------------------------------------------

unsafe extern "system" fn c_query(
    this: *mut c_void,
    iid: *const Tuid,
    out: *mut *mut c_void,
) -> Tresult {
    unsafe { Plugin::query(obj!(this, component_vtbl), iid, out) }
}
unsafe extern "system" fn c_add_ref(this: *mut c_void) -> u32 {
    unsafe { Plugin::add_ref(obj!(this, component_vtbl)) }
}
unsafe extern "system" fn c_release(this: *mut c_void) -> u32 {
    unsafe { Plugin::release(obj!(this, component_vtbl)) }
}

unsafe extern "system" fn c_initialize(_this: *mut c_void, _ctx: *mut c_void) -> Tresult {
    K_RESULT_OK
}

unsafe extern "system" fn c_terminate(this: *mut c_void) -> Tresult {
    let p = unsafe { obj!(this, component_vtbl) };
    if let Ok(mut st) = unsafe { (*p).state.lock() } {
        st.engine = None;
        st.reader = None;
        st.handler = std::ptr::null_mut();
    }
    K_RESULT_OK
}

unsafe extern "system" fn c_get_controller_class_id(
    _this: *mut c_void,
    _cid: *mut Tuid,
) -> Tresult {
    // 分離しない。ホストはこれを見てコンポーネント自身に IEditController を聞く。
    K_NOT_IMPLEMENTED
}

unsafe extern "system" fn c_set_io_mode(_this: *mut c_void, _mode: i32) -> Tresult {
    K_NOT_IMPLEMENTED
}

unsafe extern "system" fn c_get_bus_count(_this: *mut c_void, media: i32, dir: i32) -> i32 {
    match (media, dir) {
        // 録音バスは物理的に 1 系統しかないのでステレオ 1 本。
        (K_AUDIO, K_OUTPUT) => 1,
        (K_EVENT, K_INPUT) => EVENT_BUSES.len() as i32,
        _ => 0,
    }
}

unsafe extern "system" fn c_get_bus_info(
    _this: *mut c_void,
    media: i32,
    dir: i32,
    index: i32,
    info: *mut BusInfo,
) -> Tresult {
    if info.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let info = unsafe { &mut *info };
    info.media_type = media;
    info.direction = dir;
    info.bus_type = K_MAIN;
    match (media, dir) {
        (K_AUDIO, K_OUTPUT) if index == 0 => {
            info.channel_count = CHANNELS as i32;
            set_str16(&mut info.name, "SD-90");
            info.flags = K_DEFAULT_ACTIVE;
            K_RESULT_OK
        }
        (K_EVENT, K_INPUT) if (index as usize) < EVENT_BUSES.len() => {
            info.channel_count = MIDI_CHANNELS;
            set_str16(&mut info.name, EVENT_BUSES[index as usize]);
            // 背面 MIDI OUT (cable 2/3) は要る人だけ有効にする。
            info.flags = if index < 2 { K_DEFAULT_ACTIVE } else { 0 };
            K_RESULT_OK
        }
        _ => K_INVALID_ARGUMENT,
    }
}

unsafe extern "system" fn c_get_routing_info(
    _this: *mut c_void,
    _in: *mut RoutingInfo,
    _out: *mut RoutingInfo,
) -> Tresult {
    K_NOT_IMPLEMENTED
}

unsafe extern "system" fn c_activate_bus(
    _this: *mut c_void,
    _media: i32,
    _dir: i32,
    _index: i32,
    _state: u8,
) -> Tresult {
    K_RESULT_OK
}

unsafe extern "system" fn c_set_active(this: *mut c_void, state: u8) -> Tresult {
    let p = unsafe { obj!(this, component_vtbl) };
    let Ok(mut st) = (unsafe { (*p).state.lock() }) else {
        return K_INTERNAL_ERROR;
    };
    if state != 0 {
        st.active = true;
        let ctrl = unsafe { &(*p).ctrl };
        match Engine::acquire(st.safety) {
            Ok(e) => {
                st.reader = Some(st.new_reader());
                if let Ok(mut w) = ctrl.engine.lock() {
                    *w = Arc::downgrade(&e);
                }
                st.engine = Some(e);
                st.error = None;
                ctrl.online.store(true, Ordering::Relaxed);
                if let Ok(mut s) = ctrl.status.lock() {
                    s.clear();
                }
                // 掴むたびに本体は別の誰かに触られているかもしれない。
                // エディタで作った設定があるなら送り直す。
                if ctrl.model.lock().map(|m| m.recall).unwrap_or(false) {
                    ctrl.send_all();
                }
            }
            Err(e) => {
                // ここで失敗を返すとホストによってはプラグインを無効化して
                // しまう。デバイスが無くてもプロジェクトは開けるべきなので、
                // 無音を返す状態で生かしておく。
                ctrl.online.store(false, Ordering::Relaxed);
                if let Ok(mut s) = ctrl.status.lock() {
                    *s = e.clone();
                }
                st.error = Some(e);
                st.engine = None;
                st.reader = None;
            }
        }
        ctrl.latency.store(st.latency(), Ordering::Relaxed);
    } else {
        st.active = false;
        if let Some(e) = &st.engine {
            e.panic();
        }
        st.engine = None;
        st.reader = None;
        let ctrl = unsafe { &(*p).ctrl };
        ctrl.online.store(false, Ordering::Relaxed);
    }
    K_RESULT_OK
}

/// 保存するのはエディタの `Model` だけ (`sd90.rs` の形式)。
unsafe extern "system" fn c_set_state(this: *mut c_void, s: *mut c_void) -> Tresult {
    if s.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let p = unsafe { obj!(this, component_vtbl) };
    let bytes = unsafe { IBStream::read_to_end(s as *mut IBStream, 1 << 20) };
    let Some(model) = Model::from_bytes(&bytes) else {
        // 空 (旧版で保存したプロジェクト) や壊れたものは無視して既定のまま。
        return K_RESULT_OK;
    };
    let ctrl = unsafe { &(*p).ctrl };
    let recall = model.recall;
    if let Ok(mut m) = ctrl.model.lock() {
        *m = model;
    }
    ctrl.model_rev.fetch_add(1, Ordering::Relaxed);
    if recall && ctrl.online.load(Ordering::Relaxed) {
        ctrl.send_all();
    }
    K_RESULT_OK
}

unsafe extern "system" fn c_get_state(this: *mut c_void, s: *mut c_void) -> Tresult {
    if s.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let p = unsafe { obj!(this, component_vtbl) };
    let ctrl = unsafe { &(*p).ctrl };
    let bytes = match ctrl.model.lock() {
        Ok(m) => m.to_bytes(),
        Err(_) => return K_INTERNAL_ERROR,
    };
    if unsafe { IBStream::write_all(s as *mut IBStream, &bytes) } {
        K_RESULT_OK
    } else {
        K_INTERNAL_ERROR
    }
}

static COMPONENT_VTBL: IComponentVtbl = IComponentVtbl {
    query_interface: c_query,
    add_ref: c_add_ref,
    release: c_release,
    initialize: c_initialize,
    terminate: c_terminate,
    get_controller_class_id: c_get_controller_class_id,
    set_io_mode: c_set_io_mode,
    get_bus_count: c_get_bus_count,
    get_bus_info: c_get_bus_info,
    get_routing_info: c_get_routing_info,
    activate_bus: c_activate_bus,
    set_active: c_set_active,
    set_state: c_set_state,
    get_state: c_get_state,
};

// ---- IAudioProcessor ------------------------------------------------------

unsafe extern "system" fn p_query(
    this: *mut c_void,
    iid: *const Tuid,
    out: *mut *mut c_void,
) -> Tresult {
    unsafe { Plugin::query(obj!(this, processor_vtbl), iid, out) }
}
unsafe extern "system" fn p_add_ref(this: *mut c_void) -> u32 {
    unsafe { Plugin::add_ref(obj!(this, processor_vtbl)) }
}
unsafe extern "system" fn p_release(this: *mut c_void) -> u32 {
    unsafe { Plugin::release(obj!(this, processor_vtbl)) }
}

unsafe extern "system" fn p_set_bus_arrangements(
    _this: *mut c_void,
    _inputs: *mut SpeakerArrangement,
    num_in: i32,
    outputs: *mut SpeakerArrangement,
    num_out: i32,
) -> Tresult {
    // ステレオ 1 本以外は受けない。
    if num_in == 0 && num_out == 1 && !outputs.is_null() && unsafe { *outputs } == K_SPEAKER_STEREO
    {
        K_RESULT_TRUE
    } else {
        K_RESULT_FALSE
    }
}

unsafe extern "system" fn p_get_bus_arrangement(
    _this: *mut c_void,
    dir: i32,
    index: i32,
    arr: *mut SpeakerArrangement,
) -> Tresult {
    if arr.is_null() || dir != K_OUTPUT || index != 0 {
        return K_INVALID_ARGUMENT;
    }
    unsafe { *arr = K_SPEAKER_STEREO };
    K_RESULT_OK
}

unsafe extern "system" fn p_can_process_sample_size(_this: *mut c_void, size: i32) -> Tresult {
    if size == K_SAMPLE32 {
        K_RESULT_OK
    } else {
        K_RESULT_FALSE
    }
}

unsafe extern "system" fn p_get_latency_samples(this: *mut c_void) -> u32 {
    let p = unsafe { obj!(this, processor_vtbl) };
    match unsafe { (*p).state.lock() } {
        Ok(st) => st.latency(),
        Err(_) => 0,
    }
}

unsafe extern "system" fn p_setup_processing(
    this: *mut c_void,
    setup: *mut ProcessSetup,
) -> Tresult {
    if setup.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let s = unsafe { &*setup };
    // SD-90 の水晶は 48kHz 固定。他のレートはリサンプラで公称比ごと変換する。
    if !(8_000.0..=768_000.0).contains(&s.sample_rate) || s.symbolic_sample_size != K_SAMPLE32 {
        return K_RESULT_FALSE;
    }
    let p = unsafe { obj!(this, processor_vtbl) };
    let Ok(mut st) = (unsafe { (*p).state.lock() }) else {
        return K_INTERNAL_ERROR;
    };
    st.max_block = s.max_samples_per_block.max(1) as usize;
    st.rate = s.sample_rate;
    st.update_safety();
    st.realtime = s.process_mode == K_REALTIME;
    if st.engine.is_some() {
        st.reader = Some(st.new_reader());
    }
    let ctrl = unsafe { &(*p).ctrl };
    ctrl.latency.store(st.latency(), Ordering::Relaxed);
    ctrl.host_rate.store(st.rate as u32, Ordering::Relaxed);
    K_RESULT_OK
}

unsafe extern "system" fn p_set_processing(this: *mut c_void, state: u8) -> Tresult {
    let p = unsafe { obj!(this, processor_vtbl) };
    let Ok(mut st) = (unsafe { (*p).state.lock() }) else {
        return K_INTERNAL_ERROR;
    };
    if state == 0 {
        if let Some(e) = &st.engine {
            e.panic();
        }
    } else if let Some(r) = st.reader.as_mut() {
        // 止まっていた間も USB 側は積み続けている。溜まった分を持ち越すと
        // 実際の遅れが申告値より大きくなるので、助走からやり直す。
        r.reset();
    }
    K_RESULT_OK
}

unsafe extern "system" fn p_get_tail_samples(_this: *mut c_void) -> u32 {
    // 残響は音源の中。こちらは持っていない。
    0
}

/// 出力バッファを無音で埋める。
unsafe fn silence_outputs(data: &ProcessData) {
    if data.outputs.is_null() || data.num_outputs < 1 {
        return;
    }
    let bus = unsafe { &*data.outputs };
    if bus.channel_buffers.is_null() {
        return;
    }
    for ch in 0..bus.num_channels as isize {
        let buf = unsafe { *bus.channel_buffers.offset(ch) };
        if !buf.is_null() {
            unsafe { std::slice::from_raw_parts_mut(buf, data.num_samples as usize) }.fill(0.0);
        }
    }
}

/// f32 の 0..1 を MIDI の 0..127 にする。
fn to7(v: f32) -> u8 {
    (v * 127.0 + 0.5).clamp(0.0, 127.0) as u8
}

unsafe extern "system" fn p_process(this: *mut c_void, data: *mut ProcessData) -> Tresult {
    if data.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let data = unsafe { &mut *data };
    let p = unsafe { obj!(this, processor_vtbl) };

    // RT スレッドなのでロックは待たない。取れなければ無音を返す。
    // (競合するのはホストが setState 等を並行して呼んだときだけ)
    let mut st = match unsafe { (*p).state.try_lock() } {
        Ok(st) => st,
        Err(TryLockError::WouldBlock) | Err(TryLockError::Poisoned(_)) => {
            unsafe { silence_outputs(data) };
            return K_RESULT_OK;
        }
    };

    // オフラインレンダはこの構成では成立しない (実時間でしか音が返らない)。
    // バウンスはリアルタイムで行うこと。
    if data.symbolic_sample_size != K_SAMPLE32 || data.process_mode == K_OFFLINE {
        unsafe { silence_outputs(data) };
        return K_RESULT_OK;
    }

    let Some(engine) = st.engine.clone() else {
        unsafe { silence_outputs(data) };
        return K_RESULT_OK;
    };

    // このブロックの sampleOffset 0 に対応する送出フレーム。B はエンジンを
    // 開いたときの値ではなく、今のレート / ブロック長で決めたものを使う。
    let base = engine.read_frame() + st.safety as u64;
    // sampleOffset (ホストのフレーム) をデバイスフレームに直す。
    let nominal = st.nominal();
    let dev = |k: i32| (k.max(0) as f64 * nominal).round() as u64;
    let ctrl = unsafe { &(*p).ctrl };

    // ---- エディタからの送信 ----
    // DAW のイベントより先に、ブロックの先頭フレームで積む。送出スレッドは
    // フレームの昇順に積まれる前提なので、未来のフレームには積まない。
    let dev_samples = dev(data.num_samples) as usize;
    let mut sent = ctrl.drain(&mut st.pacer, dev_samples, |cable, msg| {
        engine.schedule(base, msg, cable)
    });
    // DAW から来た CC のうち、エディタに映すもの。
    let mut seen: [(u8, u8, u8, u8); 16] = [(0, 0, 0, 0); 16];
    let mut n_seen = 0usize;

    // ---- ノート等のイベント ----
    if !data.input_events.is_null() {
        let n = unsafe { IEventList::count(data.input_events) };
        for i in 0..n {
            let mut e = std::mem::MaybeUninit::<Event>::zeroed();
            if unsafe { IEventList::get(data.input_events, i, &mut *e.as_mut_ptr()) } != K_RESULT_OK
            {
                continue;
            }
            let e = unsafe { e.assume_init() };
            let bus = e.bus_index as usize;
            if bus >= CABLES.len() {
                continue;
            }
            let cable = CABLES[bus];
            let at = base + dev(e.sample_offset);
            unsafe {
                match e.event_type {
                    K_NOTE_ON_EVENT => {
                        let on = e.ev.note_on;
                        let vel = to7(on.velocity);
                        let ch = (on.channel & 0x0F) as u8;
                        // velocity 0 のノートオンはノートオフとして扱う。
                        if vel == 0 {
                            engine.schedule(at, &[0x80 | ch, on.pitch as u8 & 0x7F, 0], cable);
                        } else {
                            engine.schedule(at, &[0x90 | ch, on.pitch as u8 & 0x7F, vel], cable);
                        }
                        sent = true;
                    }
                    K_NOTE_OFF_EVENT => {
                        let off = e.ev.note_off;
                        let ch = (off.channel & 0x0F) as u8;
                        engine.schedule(
                            at,
                            &[0x80 | ch, off.pitch as u8 & 0x7F, to7(off.velocity)],
                            cable,
                        );
                        sent = true;
                    }
                    K_POLY_PRESSURE_EVENT => {
                        let pp = e.ev.poly_pressure;
                        let ch = (pp.channel & 0x0F) as u8;
                        engine.schedule(
                            at,
                            &[0xA0 | ch, pp.pitch as u8 & 0x7F, to7(pp.pressure)],
                            cable,
                        );
                        sent = true;
                    }
                    K_DATA_EVENT => {
                        let d = e.ev.data;
                        // F0..F7 がそのまま入っている。生バイトを EP に直接
                        // 書くのではなく 4 バイトパケットに分解して積むこと
                        // (sd90-rs README の「CIN 誤解釈」の罠)。
                        if d.data_type == K_MIDI_SYS_EX && !d.bytes.is_null() && d.size > 0 {
                            let msg = std::slice::from_raw_parts(d.bytes, d.size as usize);
                            engine.schedule(at, msg, cable);
                            sent = true;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // ---- CC / ピッチベンド / アフタータッチ ----
    if !data.input_parameter_changes.is_null() {
        let pc = data.input_parameter_changes;
        let vt = unsafe { (*pc).vtbl };
        let count = unsafe { ((*vt).get_parameter_count)(pc as *mut c_void) };
        for qi in 0..count {
            let q = unsafe { ((*vt).get_parameter_data)(pc as *mut c_void, qi) };
            if q.is_null() {
                continue;
            }
            let qvt = unsafe { (*q).vtbl };
            let id = unsafe { ((*qvt).get_parameter_id)(q as *mut c_void) };
            let Some((bus, ch, ctrl)) = param_decode(id) else {
                continue;
            };
            let cable = CABLES[bus];
            let points = unsafe { ((*qvt).get_point_count)(q as *mut c_void) };
            for pi in 0..points {
                let (mut off, mut val) = (0i32, 0f64);
                if unsafe { ((*qvt).get_point)(q as *mut c_void, pi, &mut off, &mut val) }
                    != K_RESULT_OK
                {
                    continue;
                }
                let at = base + dev(off);
                match ctrl {
                    K_PITCH_BEND => {
                        // 14bit。0.5 が中央。
                        let v = (val * 16383.0 + 0.5).clamp(0.0, 16383.0) as u16;
                        engine.schedule(at, &[0xE0 | ch, (v & 0x7F) as u8, (v >> 7) as u8], cable);
                    }
                    K_AFTER_TOUCH => {
                        engine.schedule(at, &[0xD0 | ch, to7(val as f32)], cable);
                    }
                    cc if (0..128).contains(&cc) => {
                        let v = to7(val as f32);
                        engine.schedule(at, &[0xB0 | ch, cc as u8, v], cable);
                        if matches!(cc, 1 | 7 | 10 | 91 | 93) && n_seen < seen.len() {
                            seen[n_seen] = (bus as u8, ch, cc as u8, v);
                            n_seen += 1;
                        }
                    }
                    _ => {}
                }
                sent = true;
            }
        }
    }

    if sent {
        engine.flush();
    }
    if n_seen > 0 {
        mirror_ccs(ctrl, &seen[..n_seen]);
    }

    // ---- 音声出力 ----
    let n = data.num_samples.max(0) as usize;
    if n == 0 || data.num_outputs < 1 || data.outputs.is_null() {
        return K_RESULT_OK;
    }
    let bus = unsafe { &mut *data.outputs };
    if bus.channel_buffers.is_null() || bus.num_channels < 1 {
        return K_RESULT_OK;
    }
    let Some(reader) = st.reader.as_mut() else {
        unsafe { silence_outputs(data) };
        return K_RESULT_OK;
    };

    let wrote = unsafe {
        let lp = *bus.channel_buffers;
        if lp.is_null() {
            return K_RESULT_OK;
        }
        let left = std::slice::from_raw_parts_mut(lp, n);
        if bus.num_channels >= 2 {
            let rp = *bus.channel_buffers.offset(1);
            if rp.is_null() {
                return K_RESULT_OK;
            }
            let right = std::slice::from_raw_parts_mut(rp, n);
            let mut bufs: [&mut [f32]; 2] = [left, right];
            reader.pull(engine.ring(), &mut bufs)
        } else {
            // モノラルで来ることは無い想定だが、来たら左だけ書く。
            let mut bufs: [&mut [f32]; 1] = [left];
            reader.pull(engine.ring(), &mut bufs)
        }
    };
    let rs = reader.stats();
    ctrl.reader_underruns.store(rs.underruns as u32, Ordering::Relaxed);
    ctrl.reader_overfulls.store(rs.overfulls as u32, Ordering::Relaxed);
    ctrl.reader_ratio.store((rs.ratio as f32).to_bits(), Ordering::Relaxed);
    ctrl.reader_level.store(rs.level as u32, Ordering::Relaxed);
    ctrl.host_frames.fetch_add(n as u64, Ordering::Relaxed);
    if wrote {
        for ch in 0..(bus.num_channels.min(2) as isize) {
            let b = unsafe { *bus.channel_buffers.offset(ch) };
            if !b.is_null() {
                let s = unsafe { std::slice::from_raw_parts(b, n) };
                let peak = s.iter().fold(0f32, |a, x| a.max(x.abs()));
                ctrl.note_peak(ch as usize, peak);
            }
        }
    }
    // 無音を返した場合は silence フラグを立てておくとホストが後段を省ける。
    bus.silence_flags = if wrote {
        0
    } else {
        (1 << bus.num_channels) - 1
    };
    K_RESULT_OK
}

/// DAW が送った CC をエディタの表示に映す。RT スレッドなのでロックは
/// 待たない。取れなければ映さないだけ (本体には届いている)。
fn mirror_ccs(ctrl: &Control, seen: &[(u8, u8, u8, u8)]) {
    let Ok(mut m) = ctrl.model.try_lock() else {
        return;
    };
    let mut hit = false;
    for &(bus, ch, cc, v) in seen {
        for (i, part) in m.parts.iter_mut().enumerate() {
            if Part::cable(i) != bus || !part.rx_sw || part.rx_ch != ch {
                continue;
            }
            let field = match cc {
                1 => &mut part.modulation,
                7 => &mut part.level,
                10 => &mut part.pan,
                91 => &mut part.reverb,
                93 => &mut part.chorus,
                _ => continue,
            };
            if *field != v {
                *field = v;
                hit = true;
            }
        }
    }
    drop(m);
    if hit {
        ctrl.model_rev.fetch_add(1, Ordering::Relaxed);
    }
}

static PROCESSOR_VTBL: IAudioProcessorVtbl = IAudioProcessorVtbl {
    query_interface: p_query,
    add_ref: p_add_ref,
    release: p_release,
    set_bus_arrangements: p_set_bus_arrangements,
    get_bus_arrangement: p_get_bus_arrangement,
    can_process_sample_size: p_can_process_sample_size,
    get_latency_samples: p_get_latency_samples,
    setup_processing: p_setup_processing,
    set_processing: p_set_processing,
    process: p_process,
    get_tail_samples: p_get_tail_samples,
};

// ---- IEditController ------------------------------------------------------

unsafe extern "system" fn e_query(
    this: *mut c_void,
    iid: *const Tuid,
    out: *mut *mut c_void,
) -> Tresult {
    unsafe { Plugin::query(obj!(this, controller_vtbl), iid, out) }
}
unsafe extern "system" fn e_add_ref(this: *mut c_void) -> u32 {
    unsafe { Plugin::add_ref(obj!(this, controller_vtbl)) }
}
unsafe extern "system" fn e_release(this: *mut c_void) -> u32 {
    unsafe { Plugin::release(obj!(this, controller_vtbl)) }
}

unsafe extern "system" fn e_initialize(_this: *mut c_void, _ctx: *mut c_void) -> Tresult {
    K_RESULT_OK
}
unsafe extern "system" fn e_terminate(_this: *mut c_void) -> Tresult {
    K_RESULT_OK
}
unsafe extern "system" fn e_set_component_state(_this: *mut c_void, _s: *mut c_void) -> Tresult {
    K_RESULT_OK
}
unsafe extern "system" fn e_set_state(_this: *mut c_void, _s: *mut c_void) -> Tresult {
    K_RESULT_OK
}
unsafe extern "system" fn e_get_state(_this: *mut c_void, _s: *mut c_void) -> Tresult {
    K_RESULT_OK
}

unsafe extern "system" fn e_get_parameter_count(_this: *mut c_void) -> i32 {
    param_count()
}

unsafe extern "system" fn e_get_parameter_info(
    _this: *mut c_void,
    index: i32,
    info: *mut ParameterInfo,
) -> Tresult {
    if info.is_null() || index < 0 || index >= param_count() {
        return K_INVALID_ARGUMENT;
    }
    let per_bus = MIDI_CHANNELS * CTRL_COUNT;
    let bus = index / per_bus;
    let rem = index % per_bus;
    let ch = rem / CTRL_COUNT;
    let ctrl = rem % CTRL_COUNT;

    let info = unsafe { &mut *info };
    info.id = param_id(bus, ch, ctrl);
    let what = match ctrl {
        K_AFTER_TOUCH => "AT".to_string(),
        K_PITCH_BEND => "PB".to_string(),
        cc => format!("CC{cc}"),
    };
    set_str16(
        &mut info.title,
        &format!("{} ch{} {}", EVENT_BUSES[bus as usize], ch + 1, what),
    );
    set_str16(&mut info.short_title, &what);
    set_str16(&mut info.units, "");
    info.step_count = 0;
    // ピッチベンドだけ中央が既定。
    info.default_normalized_value = if ctrl == K_PITCH_BEND { 0.5 } else { 0.0 };
    info.unit_id = 0;
    // kIsHidden を付ければホストの自動化一覧から消えるが、値を配らなくなる
    // ホストがあると CC が黙って効かなくなる。見えていて困る程度で済ませる。
    info.flags = K_CAN_AUTOMATE;
    K_RESULT_OK
}

unsafe extern "system" fn e_get_param_string_by_value(
    _this: *mut c_void,
    id: u32,
    value: f64,
    out: *mut u16,
) -> Tresult {
    if out.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let Some((_, _, ctrl)) = param_decode(id) else {
        return K_INVALID_ARGUMENT;
    };
    let s = if ctrl == K_PITCH_BEND {
        format!("{}", (value * 16383.0).round() as i32 - 8192)
    } else {
        format!("{}", (value * 127.0).round() as i32)
    };
    let dst = unsafe { std::slice::from_raw_parts_mut(out, 128) };
    set_str16(dst, &s);
    K_RESULT_OK
}

unsafe extern "system" fn e_get_param_value_by_string(
    _this: *mut c_void,
    _id: u32,
    _s: *const u16,
    _out: *mut f64,
) -> Tresult {
    K_NOT_IMPLEMENTED
}

unsafe extern "system" fn e_normalized_to_plain(_this: *mut c_void, id: u32, v: f64) -> f64 {
    match param_decode(id) {
        Some((_, _, K_PITCH_BEND)) => v * 16383.0 - 8192.0,
        _ => v * 127.0,
    }
}

unsafe extern "system" fn e_plain_to_normalized(_this: *mut c_void, id: u32, v: f64) -> f64 {
    match param_decode(id) {
        Some((_, _, K_PITCH_BEND)) => ((v + 8192.0) / 16383.0).clamp(0.0, 1.0),
        _ => (v / 127.0).clamp(0.0, 1.0),
    }
}

unsafe extern "system" fn e_get_param_normalized(_this: *mut c_void, id: u32) -> f64 {
    match param_decode(id) {
        Some((_, _, K_PITCH_BEND)) => 0.5,
        _ => 0.0,
    }
}

unsafe extern "system" fn e_set_param_normalized(_this: *mut c_void, _id: u32, _v: f64) -> Tresult {
    // 値は process の inputParameterChanges 側で拾う。こちらは保持しない。
    K_RESULT_OK
}

unsafe extern "system" fn e_set_component_handler(
    this: *mut c_void,
    handler: *mut c_void,
) -> Tresult {
    let p = unsafe { obj!(this, controller_vtbl) };
    if let Ok(mut st) = unsafe { (*p).state.lock() } {
        st.handler = handler as *mut IComponentHandler;
    }
    K_RESULT_OK
}

unsafe extern "system" fn e_create_view(this: *mut c_void, name: *const u8) -> *mut c_void {
    if name.is_null() {
        return std::ptr::null_mut();
    }
    let name = unsafe { std::ffi::CStr::from_ptr(name as *const std::ffi::c_char) };
    if name != VIEW_TYPE_EDITOR {
        return std::ptr::null_mut();
    }
    let p = unsafe { obj!(this, controller_vtbl) };
    crate::gui::View::create(unsafe { (*p).ctrl.clone() })
}

static CONTROLLER_VTBL: IEditControllerVtbl = IEditControllerVtbl {
    query_interface: e_query,
    add_ref: e_add_ref,
    release: e_release,
    initialize: e_initialize,
    terminate: e_terminate,
    set_component_state: e_set_component_state,
    set_state: e_set_state,
    get_state: e_get_state,
    get_parameter_count: e_get_parameter_count,
    get_parameter_info: e_get_parameter_info,
    get_param_string_by_value: e_get_param_string_by_value,
    get_param_value_by_string: e_get_param_value_by_string,
    normalized_param_to_plain: e_normalized_to_plain,
    plain_param_to_normalized: e_plain_to_normalized,
    get_param_normalized: e_get_param_normalized,
    set_param_normalized: e_set_param_normalized,
    set_component_handler: e_set_component_handler,
    create_view: e_create_view,
};

// ---- IMidiMapping ---------------------------------------------------------

unsafe extern "system" fn m_query(
    this: *mut c_void,
    iid: *const Tuid,
    out: *mut *mut c_void,
) -> Tresult {
    unsafe { Plugin::query(obj!(this, midimap_vtbl), iid, out) }
}
unsafe extern "system" fn m_add_ref(this: *mut c_void) -> u32 {
    unsafe { Plugin::add_ref(obj!(this, midimap_vtbl)) }
}
unsafe extern "system" fn m_release(this: *mut c_void) -> u32 {
    unsafe { Plugin::release(obj!(this, midimap_vtbl)) }
}

unsafe extern "system" fn m_get_assignment(
    _this: *mut c_void,
    bus: i32,
    channel: i16,
    ctrl: i32,
    id: *mut u32,
) -> Tresult {
    if id.is_null()
        || bus < 0
        || bus as usize >= EVENT_BUSES.len()
        || !(0..MIDI_CHANNELS as i16).contains(&channel)
        || !(0..CTRL_COUNT).contains(&ctrl)
    {
        return K_RESULT_FALSE;
    }
    unsafe { *id = param_id(bus, channel as i32, ctrl) };
    K_RESULT_OK
}

static MIDIMAP_VTBL: IMidiMappingVtbl = IMidiMappingVtbl {
    query_interface: m_query,
    add_ref: m_add_ref,
    release: m_release,
    get_midi_controller_assignment: m_get_assignment,
};
