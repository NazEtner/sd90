//! ASIO ドライバの自己検証用の、ごく小さなホスト実装。
//!
//! レジストリ登録 (= 管理者権限) 無しで、DllGetClassObject → クラスファクトリ →
//! IASIO の全経路を実際に叩く。vtable はホスト側が自前で宣言するものなので、
//! ここでもドライバ側の定義を使わずに書き下ろす。ずれていれば即座に壊れる。

use sd90_asio::asio::{AsioBufferInfo, AsioCallbacks, AsioChannelInfo, AsioSamples};
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicUsize, Ordering};
use windows::Win32::Foundation::S_OK;
use windows::core::{GUID, HRESULT};

const IID_ICLASSFACTORY: GUID = GUID::from_u128(0x00000001_0000_0000_c000_000000000046);
const CLSID: GUID = GUID::from_u128(0x7e1a5c42_9b3d_4f86_a1c7_5d2e0b4f93a8);

#[repr(C)]
struct FactoryVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    create_instance: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const GUID,
        *mut *mut c_void,
    ) -> HRESULT,
    lock_server: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
}

#[repr(C)]
struct AsioVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    init: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
    get_driver_name: unsafe extern "system" fn(*mut c_void, *mut u8),
    get_driver_version: unsafe extern "system" fn(*mut c_void) -> i32,
    get_error_message: unsafe extern "system" fn(*mut c_void, *mut u8),
    start: unsafe extern "system" fn(*mut c_void) -> i32,
    stop: unsafe extern "system" fn(*mut c_void) -> i32,
    get_channels: unsafe extern "system" fn(*mut c_void, *mut i32, *mut i32) -> i32,
    get_latencies: unsafe extern "system" fn(*mut c_void, *mut i32, *mut i32) -> i32,
    get_buffer_size:
        unsafe extern "system" fn(*mut c_void, *mut i32, *mut i32, *mut i32, *mut i32) -> i32,
    can_sample_rate: unsafe extern "system" fn(*mut c_void, f64) -> i32,
    get_sample_rate: unsafe extern "system" fn(*mut c_void, *mut f64) -> i32,
    set_sample_rate: unsafe extern "system" fn(*mut c_void, f64) -> i32,
    get_clock_sources: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut i32) -> i32,
    set_clock_source: unsafe extern "system" fn(*mut c_void, i32) -> i32,
    get_sample_position:
        unsafe extern "system" fn(*mut c_void, *mut AsioSamples, *mut AsioSamples) -> i32,
    get_channel_info: unsafe extern "system" fn(*mut c_void, *mut AsioChannelInfo) -> i32,
    create_buffers: unsafe extern "system" fn(
        *mut c_void,
        *mut AsioBufferInfo,
        i32,
        i32,
        *const AsioCallbacks,
    ) -> i32,
    dispose_buffers: unsafe extern "system" fn(*mut c_void) -> i32,
    control_panel: unsafe extern "system" fn(*mut c_void) -> i32,
    future: unsafe extern "system" fn(*mut c_void, i32, *mut c_void) -> i32,
    output_ready: unsafe extern "system" fn(*mut c_void) -> i32,
}

// bufferSwitch はコンテキストを取れない (ASIO の仕様) ので静的に持つ。
static SWITCHES: AtomicI64 = AtomicI64::new(0);
static PEAK_L: AtomicI32 = AtomicI32::new(0);
static PEAK_R: AtomicI32 = AtomicI32::new(0);
static BUF_L: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
static BUF_R: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
static OUT_L: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
static OUT_R: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
static BUF_SIZE: AtomicI32 = AtomicI32::new(0);
/// 出力の検証用に 440Hz を流す。位相は bufferSwitch 内でしか触らない。
static PHASE: AtomicI64 = AtomicI64::new(0);
static TONE: AtomicI32 = AtomicI32::new(0);
/// 戻ってきた入力を検証するための記録先 (i32 LE インタリーブ)。
static REC: Mutex<Vec<i32>> = Mutex::new(Vec::new());
static RECORDING: AtomicI32 = AtomicI32::new(0);

unsafe extern "system" fn on_buffer_switch(index: i32, _direct: i32) {
    SWITCHES.fetch_add(1, Ordering::Relaxed);
    let n = BUF_SIZE.load(Ordering::Relaxed) as usize;
    let i = index.clamp(0, 1) as usize;
    for (slot, peak) in [(&BUF_L[i], &PEAK_L), (&BUF_R[i], &PEAK_R)] {
        let p = slot.load(Ordering::Relaxed) as *const i32;
        if p.is_null() || n == 0 {
            continue;
        }
        let s = unsafe { std::slice::from_raw_parts(p, n) };
        let mut m = peak.load(Ordering::Relaxed);
        for v in s {
            let a = v.abs();
            if a > m {
                m = a;
            }
        }
        peak.store(m, Ordering::Relaxed);
    }

    // 入力を記録 (検証用。RT 的には行儀が悪いがテスト専用)。
    if RECORDING.load(Ordering::Relaxed) != 0 && n > 0 {
        let l = BUF_L[i].load(Ordering::Relaxed) as *const i32;
        let r = BUF_R[i].load(Ordering::Relaxed) as *const i32;
        if !l.is_null()
            && !r.is_null()
            && let Ok(mut rec) = REC.lock()
        {
            let ls = unsafe { std::slice::from_raw_parts(l, n) };
            let rs = unsafe { std::slice::from_raw_parts(r, n) };
            for k in 0..n {
                rec.push(ls[k]);
                rec.push(rs[k]);
            }
        }
    }

    // 出力バッファに 440Hz の正弦波を書き込む (-12 dBFS)。
    if TONE.load(Ordering::Relaxed) != 0 {
        let mut ph = PHASE.load(Ordering::Relaxed);
        for slot in [&OUT_L[i], &OUT_R[i]] {
            let p = slot.load(Ordering::Relaxed) as *mut i32;
            if p.is_null() || n == 0 {
                continue;
            }
            let d = unsafe { std::slice::from_raw_parts_mut(p, n) };
            let mut k = ph;
            for v in d.iter_mut() {
                let t = k as f64 * 440.0 * std::f64::consts::TAU / 48000.0;
                *v = (t.sin() * 0.25 * 2147483647.0) as i32;
                k += 1;
            }
        }
        ph += n as i64;
        PHASE.store(ph, Ordering::Relaxed);
    }
}

unsafe extern "system" fn on_rate_change(_r: f64) {}
unsafe extern "system" fn on_message(
    selector: i32,
    _value: i32,
    _msg: *mut c_void,
    _opt: *mut f64,
) -> i32 {
    // kAsioSelectorSupported(1) / kAsioEngineVersion(3) 以外は 0 を返す。
    match selector {
        1 => 0,
        3 => 2,
        _ => 0,
    }
}

fn cstr(b: &[u8]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..n]).to_string()
}

pub fn run(duration_ms: u64) -> Result<(), String> {
    unsafe {
        let mut factory: *mut c_void = std::ptr::null_mut();
        let hr = sd90_asio::comserver::DllGetClassObject(&CLSID, &IID_ICLASSFACTORY, &mut factory);
        if hr != S_OK || factory.is_null() {
            return Err(format!("DllGetClassObject 失敗 hr=0x{:08X}", hr.0));
        }
        let fv = *(factory as *const *const FactoryVtbl);

        let mut drv: *mut c_void = std::ptr::null_mut();
        // ASIO ホストの流儀どおり、IID に CLSID を渡す。
        let hr = ((*fv).create_instance)(factory, std::ptr::null_mut(), &CLSID, &mut drv);
        ((*fv).release)(factory);
        if hr != S_OK || drv.is_null() {
            return Err(format!("CreateInstance 失敗 hr=0x{:08X}", hr.0));
        }
        let v = *(drv as *const *const AsioVtbl);

        if ((*v).init)(drv, std::ptr::null_mut()) == 0 {
            let mut msg = [0u8; 128];
            ((*v).get_error_message)(drv, msg.as_mut_ptr());
            ((*v).release)(drv);
            return Err(format!("init 失敗: {}", cstr(&msg)));
        }

        let mut name = [0u8; 32];
        ((*v).get_driver_name)(drv, name.as_mut_ptr());
        println!("ドライバ名   : {}", cstr(&name));
        println!("バージョン   : {}", ((*v).get_driver_version)(drv));

        let (mut ins, mut outs) = (0i32, 0i32);
        ((*v).get_channels)(drv, &mut ins, &mut outs);
        println!("チャンネル   : in={ins} out={outs}");

        let (mut mn, mut mx, mut pref, mut gran) = (0i32, 0i32, 0i32, 0i32);
        ((*v).get_buffer_size)(drv, &mut mn, &mut mx, &mut pref, &mut gran);
        println!("バッファ     : min={mn} max={mx} preferred={pref} granularity={gran}");

        let mut rate = 0f64;
        ((*v).get_sample_rate)(drv, &mut rate);
        println!(
            "サンプルレート: {rate}  (48000可={}, 44100可={})",
            ((*v).can_sample_rate)(drv, 48000.0) == 0,
            ((*v).can_sample_rate)(drv, 44100.0) == 0
        );

        for ch in 0..ins {
            let mut info: AsioChannelInfo = std::mem::zeroed();
            info.channel = ch;
            info.is_input = 1;
            if ((*v).get_channel_info)(drv, &mut info) == 0 {
                println!(
                    "  ch{ch}: name={:?} type={} group={}",
                    cstr(&info.name),
                    info.sample_type,
                    info.channel_group
                );
            }
        }

        let bs = pref;
        BUF_SIZE.store(bs, Ordering::Relaxed);
        let tone = std::env::var("SD90_TONE").is_ok();
        let rec_only = std::env::var("SD90_REC").is_ok();
        TONE.store(if tone { 1 } else { 0 }, Ordering::Relaxed);
        let mut infos: Vec<AsioBufferInfo> = (0..ins)
            .map(|c| AsioBufferInfo {
                is_input: 1,
                channel_num: c,
                buffers: [std::ptr::null_mut(); 2],
            })
            .chain((0..outs).map(|c| AsioBufferInfo {
                is_input: 0,
                channel_num: c,
                buffers: [std::ptr::null_mut(); 2],
            }))
            .collect();
        let cbs = AsioCallbacks {
            buffer_switch: Some(on_buffer_switch),
            sample_rate_did_change: Some(on_rate_change),
            asio_message: Some(on_message),
            buffer_switch_time_info: None,
        };
        let total = infos.len() as i32;
        let e = ((*v).create_buffers)(drv, infos.as_mut_ptr(), total, bs, &cbs);
        if e != 0 {
            ((*v).release)(drv);
            return Err(format!("createBuffers 失敗 ASIOError={e}"));
        }
        BUF_L[0].store(infos[0].buffers[0] as usize, Ordering::Relaxed);
        BUF_L[1].store(infos[0].buffers[1] as usize, Ordering::Relaxed);
        if ins > 1 {
            BUF_R[0].store(infos[1].buffers[0] as usize, Ordering::Relaxed);
            BUF_R[1].store(infos[1].buffers[1] as usize, Ordering::Relaxed);
        }
        let base = ins as usize;
        if outs > 0 {
            OUT_L[0].store(infos[base].buffers[0] as usize, Ordering::Relaxed);
            OUT_L[1].store(infos[base].buffers[1] as usize, Ordering::Relaxed);
        }
        if outs > 1 {
            OUT_R[0].store(infos[base + 1].buffers[0] as usize, Ordering::Relaxed);
            OUT_R[1].store(infos[base + 1].buffers[1] as usize, Ordering::Relaxed);
        }
        if tone || rec_only {
            println!("(入力を asiotest_in.i32 に記録する)");
            if let Ok(mut r) = REC.lock() {
                r.reserve(48000 * 2 * 10);
            }
            RECORDING.store(1, Ordering::Relaxed);
        }

        let (mut il, mut ol) = (0i32, 0i32);
        ((*v).get_latencies)(drv, &mut il, &mut ol);
        println!(
            "レイテンシ   : in={il} samples ({:.1}ms) out={ol} samples ({:.1}ms)",
            il as f64 / 48.0,
            ol as f64 / 48.0
        );

        println!("\n{duration_ms}ms 走らせる (bufferSize={bs})…");
        let e = ((*v).start)(drv);
        if e != 0 {
            let mut msg = [0u8; 128];
            ((*v).get_error_message)(drv, msg.as_mut_ptr());
            ((*v).release)(drv);
            return Err(format!("start 失敗 ASIOError={e}: {}", cstr(&msg)));
        }
        let t0 = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(duration_ms));

        let mut pos = AsioSamples::default();
        let mut ts = AsioSamples::default();
        ((*v).get_sample_position)(drv, &mut pos, &mut ts);
        let position = ((pos.hi as u64) << 32) | pos.lo as u64;

        ((*v).stop)(drv);
        let elapsed = t0.elapsed().as_secs_f64();

        let switches = SWITCHES.load(Ordering::Relaxed);
        let expected = (elapsed * 48000.0 / bs as f64).round() as i64;
        println!("\n=== 結果 ===");
        println!("bufferSwitch : {switches} 回 (期待値 約{expected} 回)");
        println!(
            "サンプル位置 : {position} (経過 {elapsed:.3}s -> {:.0} Hz)",
            position as f64 / elapsed
        );
        println!(
            "ピーク       : L={} ({:.1} dBFS)  R={} ({:.1} dBFS)",
            PEAK_L.load(Ordering::Relaxed),
            20.0 * (PEAK_L.load(Ordering::Relaxed).max(1) as f64 / 2147483648.0).log10(),
            PEAK_R.load(Ordering::Relaxed),
            20.0 * (PEAK_R.load(Ordering::Relaxed).max(1) as f64 / 2147483648.0).log10(),
        );

        RECORDING.store(0, Ordering::Relaxed);
        if (tone || rec_only)
            && let Ok(rec) = REC.lock()
        {
            let mut bytes = Vec::with_capacity(rec.len() * 4);
            for v in rec.iter() {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            let _ = std::fs::write("asiotest_in.i32", &bytes);
            println!(
                "入力記録     : {} フレーム -> asiotest_in.i32",
                rec.len() / 2
            );
        }

        ((*v).dispose_buffers)(drv);
        let refs = ((*v).release)(drv);
        println!("解放後の参照 : {refs} (0 なら漏れなし)");
        Ok(())
    }
}
