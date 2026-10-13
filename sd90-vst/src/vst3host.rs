//! `vst3test` 用の最小 VST3 ホスト (bin 側のみ)。
//!
//! DAW も Steinberg の validator も無しに、DLL を `LoadLibrary` して
//! `GetPluginFactory` → クラスファクトリ → `IComponent` / `IAudioProcessor` /
//! `IEditController` / `IMidiMapping` の全経路を叩く。ASIO ドライバに対する
//! `sd90-rs asiotest` と同じ役割。
//!
//! vtable の宣言はプラグイン側と同じ `vst3` モジュールを使うので、
//! 「宣言が間違っていても両方同じ間違いをするので通ってしまう」問題は残る。
//! そこは `vst3/mod.rs` の `const _: () = assert!(...)` (ヘッダから clang で
//! 実測したサイズ) で押さえてある。ここで見たいのは呼び出しが成立するか、
//! バスと ID の辻褄が合うか、実際に音が返るか。

use sd90_rs::device::{CHANNELS, SAMPLE_RATE};
use sd90_vst::plugin::CID_SD90;
use sd90_vst::vst3::*;
use std::ffi::c_void;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{PCSTR, PCWSTR};

type GetFactoryFn = unsafe extern "system" fn() -> *mut c_void;
type InitDllFn = unsafe extern "system" fn() -> bool;

fn u16str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_u16(b: &[u16]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..n])
}

fn from_u8(b: &[u8]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..n]).into_owned()
}

// ---- ホストが渡すイベントリスト --------------------------------------------

#[repr(C)]
struct HostEvents {
    vtbl: *const IEventListVtbl,
    events: Vec<Event>,
}

unsafe extern "system" fn ev_query(
    _this: *mut c_void,
    _iid: *const Tuid,
    _out: *mut *mut c_void,
) -> Tresult {
    K_NO_INTERFACE
}
unsafe extern "system" fn ev_ref(_this: *mut c_void) -> u32 {
    1
}
unsafe extern "system" fn ev_count(this: *mut c_void) -> i32 {
    unsafe { (*(this as *mut HostEvents)).events.len() as i32 }
}
unsafe extern "system" fn ev_get(this: *mut c_void, index: i32, out: *mut Event) -> Tresult {
    unsafe {
        let h = &*(this as *mut HostEvents);
        match h.events.get(index as usize) {
            Some(e) if !out.is_null() => {
                *out = *e;
                K_RESULT_OK
            }
            _ => K_INVALID_ARGUMENT,
        }
    }
}
unsafe extern "system" fn ev_add(_this: *mut c_void, _e: *mut Event) -> Tresult {
    K_NOT_IMPLEMENTED
}

static EVENTS_VTBL: IEventListVtbl = IEventListVtbl {
    query_interface: ev_query,
    add_ref: ev_ref,
    release: ev_ref,
    get_event_count: ev_count,
    get_event: ev_get,
    add_event: ev_add,
};

fn note_event(kind: u16, bus: i32, offset: i32, ch: i16, pitch: i16, vel: f32) -> Event {
    let ev = if kind == K_NOTE_ON_EVENT {
        EventUnion {
            note_on: NoteOnEvent {
                channel: ch,
                pitch,
                tuning: 0.0,
                velocity: vel,
                length: 0,
                note_id: -1,
            },
        }
    } else {
        EventUnion {
            note_off: NoteOffEvent {
                channel: ch,
                pitch,
                velocity: vel,
                note_id: -1,
                tuning: 0.0,
            },
        }
    };
    Event {
        bus_index: bus,
        sample_offset: offset,
        ppq_position: 0.0,
        flags: 0,
        event_type: kind,
        ev,
    }
}

fn sysex_event(bus: i32, offset: i32, bytes: &[u8]) -> Event {
    Event {
        bus_index: bus,
        sample_offset: offset,
        ppq_position: 0.0,
        flags: 0,
        event_type: K_DATA_EVENT,
        ev: EventUnion {
            data: DataEvent {
                size: bytes.len() as u32,
                data_type: K_MIDI_SYS_EX,
                bytes: bytes.as_ptr(),
            },
        },
    }
}

// ---- 本体 -----------------------------------------------------------------

struct Module(HMODULE);

impl Drop for Module {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}

/// 呼び出しが失敗したらそこで止める。
fn check(what: &str, hr: Tresult) -> Result<(), String> {
    if hr == K_RESULT_OK || hr == K_RESULT_TRUE {
        println!("  {what}: ok");
        Ok(())
    } else {
        Err(format!("{what} が {hr:#010x} を返した"))
    }
}

pub fn run(dll: &str, ms: u64, block: usize) -> Result<(), String> {
    let module = unsafe { LoadLibraryW(PCWSTR(u16str(dll).as_ptr())) }
        .map_err(|e| format!("{dll} を読み込めない: {e}"))?;
    let module = Module(module);
    println!("{dll} を読み込んだ");

    let get_factory: GetFactoryFn = unsafe {
        let p = GetProcAddress(module.0, PCSTR(c"GetPluginFactory".as_ptr() as *const u8))
            .ok_or("GetPluginFactory が無い")?;
        std::mem::transmute(p)
    };
    if let Some(p) = unsafe { GetProcAddress(module.0, PCSTR(c"InitDll".as_ptr() as *const u8)) } {
        let init: InitDllFn = unsafe { std::mem::transmute(p) };
        if !unsafe { init() } {
            return Err("InitDll が false を返した".into());
        }
        println!("  InitDll: ok");
    }

    let factory = unsafe { get_factory() };
    if factory.is_null() {
        return Err("GetPluginFactory が null を返した".into());
    }
    let fvt = unsafe { *(factory as *const *const IPluginFactory3Vtbl) };

    // ---- ファクトリ ----
    let mut finfo: PFactoryInfo = unsafe { std::mem::zeroed() };
    check("getFactoryInfo", unsafe {
        ((*fvt).get_factory_info)(factory, &mut finfo)
    })?;
    println!("  ベンダ: {}", from_u8(&finfo.vendor));

    let n = unsafe { ((*fvt).count_classes)(factory) };
    println!("  クラス数: {n}");
    if n < 1 {
        return Err("クラスが 1 つも無い".into());
    }

    let mut ci: PClassInfo2 = unsafe { std::mem::zeroed() };
    check("getClassInfo2", unsafe {
        ((*fvt).get_class_info2)(factory, 0, &mut ci)
    })?;
    println!(
        "  {} / {} / {} / SDK {}",
        from_u8(&ci.name),
        from_u8(&ci.category),
        from_u8(&ci.sub_categories),
        from_u8(&ci.sdk_version)
    );
    if ci.cid != CID_SD90 {
        return Err("CID が一致しない".into());
    }

    let mut ciw: PClassInfoW = unsafe { std::mem::zeroed() };
    check("getClassInfoUnicode", unsafe {
        ((*fvt).get_class_info_unicode)(factory, 0, &mut ciw)
    })?;
    println!("  (unicode) {}", from_u16(&ciw.name));

    // ---- 生成 ----
    let mut comp: *mut c_void = std::ptr::null_mut();
    check("createInstance(IComponent)", unsafe {
        ((*fvt).create_instance)(
            factory,
            CID_SD90.as_ptr(),
            IID_ICOMPONENT.as_ptr(),
            &mut comp,
        )
    })?;
    if comp.is_null() {
        return Err("createInstance が null を返した".into());
    }
    let cvt = unsafe { *(comp as *const *const IComponentVtbl) };
    check("IComponent::initialize", unsafe {
        ((*cvt).initialize)(comp, std::ptr::null_mut())
    })?;

    // ---- 他のインタフェースを引く ----
    let mut proc_: *mut c_void = std::ptr::null_mut();
    check("QI(IAudioProcessor)", unsafe {
        ((*cvt).query_interface)(comp, &IID_IAUDIO_PROCESSOR, &mut proc_)
    })?;
    let pvt = unsafe { *(proc_ as *const *const IAudioProcessorVtbl) };

    let mut ctrl: *mut c_void = std::ptr::null_mut();
    check("QI(IEditController)", unsafe {
        ((*cvt).query_interface)(comp, &IID_IEDIT_CONTROLLER, &mut ctrl)
    })?;
    let evt = unsafe { *(ctrl as *const *const IEditControllerVtbl) };

    let mut mm: *mut c_void = std::ptr::null_mut();
    check("QI(IMidiMapping)", unsafe {
        ((*cvt).query_interface)(comp, &IID_IMIDI_MAPPING, &mut mm)
    })?;
    let mvt = unsafe { *(mm as *const *const IMidiMappingVtbl) };

    // ---- バス ----
    for (media, dir, label) in [
        (K_AUDIO, K_OUTPUT, "音声出力"),
        (K_AUDIO, K_INPUT, "音声入力"),
        (K_EVENT, K_INPUT, "イベント入力"),
        (K_EVENT, K_OUTPUT, "イベント出力"),
    ] {
        let count = unsafe { ((*cvt).get_bus_count)(comp, media, dir) };
        print!("  {label}: {count} 本");
        for i in 0..count {
            let mut bi: BusInfo = unsafe { std::mem::zeroed() };
            if unsafe { ((*cvt).get_bus_info)(comp, media, dir, i, &mut bi) } == K_RESULT_OK {
                print!(
                    "  [{}] {} ({}ch{})",
                    i,
                    from_u16(&bi.name),
                    bi.channel_count,
                    if bi.flags & K_DEFAULT_ACTIVE != 0 {
                        ", 既定で有効"
                    } else {
                        ""
                    }
                );
            }
        }
        println!();
    }

    // ---- MIDI マッピング ----
    let params = unsafe { ((*evt).get_parameter_count)(ctrl) };
    println!("  パラメータ数: {params}");
    let mut id = 0u32;
    check("getMidiControllerAssignment(bus0, ch0, CC7)", unsafe {
        ((*mvt).get_midi_controller_assignment)(mm, 0, 0, 7, &mut id)
    })?;
    let mut pi: ParameterInfo = unsafe { std::mem::zeroed() };
    // 返ってきた ID が本当にパラメータ一覧に居るか (CC7 は index 7)。
    let ok = unsafe { ((*evt).get_parameter_info)(ctrl, 7, &mut pi) };
    if ok != K_RESULT_OK || pi.id != id {
        return Err(format!(
            "CC7 の ID {id} がパラメータ一覧と食い違う (一覧側 {})",
            pi.id
        ));
    }
    println!("  CC7 -> id {} \"{}\"", id, from_u16(&pi.title));

    // ---- セットアップ ----
    check("canProcessSampleSize(32)", unsafe {
        ((*pvt).can_process_sample_size)(proc_, K_SAMPLE32)
    })?;
    let mut out_arr: SpeakerArrangement = K_SPEAKER_STEREO;
    check("setBusArrangements", unsafe {
        ((*pvt).set_bus_arrangements)(proc_, std::ptr::null_mut(), 0, &mut out_arr, 1)
    })?;
    let mut setup = ProcessSetup {
        process_mode: K_REALTIME,
        symbolic_sample_size: K_SAMPLE32,
        max_samples_per_block: block as i32,
        sample_rate: SAMPLE_RATE as f64,
    };
    check("setupProcessing", unsafe {
        ((*pvt).setup_processing)(proc_, &mut setup)
    })?;
    // 48kHz 以外は断ること自体が仕様。ここで通ってしまうほうが問題。
    let mut bad = ProcessSetup {
        sample_rate: 44100.0,
        ..ProcessSetup {
            process_mode: K_REALTIME,
            symbolic_sample_size: K_SAMPLE32,
            max_samples_per_block: block as i32,
            sample_rate: 44100.0,
        }
    };
    if unsafe { ((*pvt).setup_processing)(proc_, &mut bad) } == K_RESULT_OK {
        return Err("44.1kHz を受けてしまった (48kHz 固定のはず)".into());
    }
    println!("  setupProcessing(44.1kHz): 正しく断った");
    unsafe { ((*pvt).setup_processing)(proc_, &mut setup) };

    check("setActive(true)", unsafe { ((*cvt).set_active)(comp, 1) })?;
    check("setProcessing(true)", unsafe {
        ((*pvt).set_processing)(proc_, 1)
    })?;

    let latency = unsafe { ((*pvt).get_latency_samples)(proc_) };
    println!(
        "  申告レイテンシ: {latency} フレーム ({:.2}ms)",
        latency as f64 * 1000.0 / SAMPLE_RATE as f64
    );

    // ---- process を回す ----
    let blocks = (ms * SAMPLE_RATE as u64 / 1000) as usize / block;
    let mut chans: Vec<Vec<f32>> = (0..CHANNELS).map(|_| vec![0f32; block]).collect();
    let mut rec: Vec<f32> = Vec::with_capacity(blocks * block * CHANNELS);
    let gm_on: Vec<u8> = vec![0xF0, 0x7E, 0x7F, 0x09, 0x01, 0xF7];

    let period = Duration::from_secs_f64(block as f64 / SAMPLE_RATE as f64);
    let t0 = Instant::now();
    for i in 0..blocks {
        let due = t0 + period.mul_f64(i as f64);
        while let Some(w) = due.checked_duration_since(Instant::now()) {
            if w > Duration::from_millis(2) {
                std::thread::sleep(w - Duration::from_millis(2));
            } else {
                std::thread::yield_now();
            }
        }

        let mut events = HostEvents {
            vtbl: &EVENTS_VTBL,
            events: Vec::new(),
        };
        if i == 0 {
            events.events.push(sysex_event(0, 0, &gm_on));
        } else if i == 4 {
            for (k, p) in [60i16, 64, 67].iter().enumerate() {
                events
                    .events
                    .push(note_event(K_NOTE_ON_EVENT, 0, k as i32, 0, *p, 0.8));
            }
        } else if i + blocks / 8 == blocks {
            for p in [60i16, 64, 67] {
                events
                    .events
                    .push(note_event(K_NOTE_OFF_EVENT, 0, 0, 0, p, 0.0));
            }
        }

        let mut ptrs: Vec<*mut f32> = chans.iter_mut().map(|c| c.as_mut_ptr()).collect();
        let mut bus = AudioBusBuffers {
            num_channels: CHANNELS as i32,
            silence_flags: 0,
            channel_buffers: ptrs.as_mut_ptr(),
        };
        let mut data = ProcessData {
            process_mode: K_REALTIME,
            symbolic_sample_size: K_SAMPLE32,
            num_samples: block as i32,
            num_inputs: 0,
            num_outputs: 1,
            inputs: std::ptr::null_mut(),
            outputs: &mut bus,
            input_parameter_changes: std::ptr::null_mut(),
            output_parameter_changes: std::ptr::null_mut(),
            input_events: &mut events as *mut HostEvents as *mut IEventList,
            output_events: std::ptr::null_mut(),
            process_context: std::ptr::null_mut(),
        };
        let hr = unsafe { ((*pvt).process)(proc_, &mut data) };
        if hr != K_RESULT_OK {
            return Err(format!("process が {hr:#010x} を返した (ブロック {i})"));
        }
        for k in 0..block {
            for c in &chans {
                rec.push(c[k]);
            }
        }
    }

    // ---- 畳む ----
    check("setProcessing(false)", unsafe {
        ((*pvt).set_processing)(proc_, 0)
    })?;
    check("setActive(false)", unsafe { ((*cvt).set_active)(comp, 0) })?;
    check("terminate", unsafe { ((*cvt).terminate)(comp) })?;
    unsafe {
        ((*mvt).release)(mm);
        ((*evt).release)(ctrl);
        ((*pvt).release)(proc_);
        let last = ((*cvt).release)(comp);
        println!("  最後の release 後の参照数: {last} (0 なら破棄済み)");
    }

    let left: Vec<f32> = rec.iter().step_by(CHANNELS).copied().collect();
    let peak = left.iter().fold(0f32, |m, s| m.max(s.abs()));
    println!(
        "\n{} フレーム / 左chピーク {:.4} ({:.1} dBFS)",
        left.len(),
        peak,
        20.0 * peak.log10()
    );
    if peak < 0.001 {
        return Err("音が返ってこない".into());
    }
    Ok(())
}
