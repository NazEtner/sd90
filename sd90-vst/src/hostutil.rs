//! 自前のホスト (`editor` プローブと単体版 `sd90-editor`) が共有する部品。
//! bin 側からだけ `#[path]` で取り込む。

use sd90_vst::vst3::*;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

// ---- メモリ上の IBStream ----------------------------------------------------

#[repr(C)]
pub struct MemStream {
    vtbl: *const IBStreamVtbl,
    refs: AtomicU32,
    pub data: Vec<u8>,
    pos: usize,
}

unsafe extern "system" fn ms_query(this: *mut c_void, iid: *const Tuid, out: *mut *mut c_void) -> Tresult {
    unsafe {
        if tuid_eq(iid, &IID_FUNKNOWN) || tuid_eq(iid, &IID_IBSTREAM) {
            *out = this;
            ms_add_ref(this);
            K_RESULT_OK
        } else {
            *out = std::ptr::null_mut();
            K_NO_INTERFACE
        }
    }
}
unsafe extern "system" fn ms_add_ref(this: *mut c_void) -> u32 {
    unsafe { (*(this as *mut MemStream)).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}
unsafe extern "system" fn ms_release(this: *mut c_void) -> u32 {
    // 持ち主はスタック側。数えるだけ。
    unsafe { (*(this as *mut MemStream)).refs.fetch_sub(1, Ordering::Relaxed) - 1 }
}
unsafe extern "system" fn ms_read(this: *mut c_void, buf: *mut c_void, n: i32, got: *mut i32) -> Tresult {
    let s = unsafe { &mut *(this as *mut MemStream) };
    let k = (n.max(0) as usize).min(s.data.len() - s.pos);
    unsafe {
        std::ptr::copy_nonoverlapping(s.data.as_ptr().add(s.pos), buf as *mut u8, k);
        if !got.is_null() {
            *got = k as i32;
        }
    }
    s.pos += k;
    K_RESULT_OK
}
unsafe extern "system" fn ms_write(this: *mut c_void, buf: *const c_void, n: i32, put: *mut i32) -> Tresult {
    let s = unsafe { &mut *(this as *mut MemStream) };
    let b = unsafe { std::slice::from_raw_parts(buf as *const u8, n.max(0) as usize) };
    s.data.truncate(s.pos);
    s.data.extend_from_slice(b);
    s.pos += b.len();
    if !put.is_null() {
        unsafe { *put = n };
    }
    K_RESULT_OK
}
unsafe extern "system" fn ms_seek(this: *mut c_void, pos: i64, mode: i32, out: *mut i64) -> Tresult {
    let s = unsafe { &mut *(this as *mut MemStream) };
    let base = match mode {
        0 => 0i64,
        1 => s.pos as i64,
        _ => s.data.len() as i64,
    };
    s.pos = (base + pos).clamp(0, s.data.len() as i64) as usize;
    if !out.is_null() {
        unsafe { *out = s.pos as i64 };
    }
    K_RESULT_OK
}
unsafe extern "system" fn ms_tell(this: *mut c_void, out: *mut i64) -> Tresult {
    let s = unsafe { &*(this as *mut MemStream) };
    if !out.is_null() {
        unsafe { *out = s.pos as i64 };
    }
    K_RESULT_OK
}

static MS_VTBL: IBStreamVtbl = IBStreamVtbl {
    query_interface: ms_query,
    add_ref: ms_add_ref,
    release: ms_release,
    read: ms_read,
    write: ms_write,
    seek: ms_seek,
    tell: ms_tell,
};

impl MemStream {
    pub fn new(data: Vec<u8>) -> MemStream {
        MemStream {
            vtbl: &MS_VTBL,
            refs: AtomicU32::new(1),
            data,
            pos: 0,
        }
    }
}


/// スレッドを跨いで渡すための入れ物。
pub struct SendPtr(pub *mut c_void, pub *const IAudioProcessorVtbl);
unsafe impl Send for SendPtr {}

/// ホストのオーディオスレッドのふり。止まるまで `process` を回し、出力の
/// 最大値を `peak` に記録する。
pub fn process_thread(p: SendPtr, stop: Arc<AtomicBool>, peak: Arc<AtomicU32>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let p = p;
        let (proc_, pvt) = (p.0, p.1);
        const BLOCK: usize = 256;
        let mut chans = [[0f32; BLOCK]; 2];
        let period = Duration::from_secs_f64(BLOCK as f64 / 48000.0);
        let t0 = Instant::now();
        let mut i = 0u64;
        while !stop.load(Ordering::Relaxed) {
            let due = t0 + period.mul_f64(i as f64);
            while let Some(w) = due.checked_duration_since(Instant::now()) {
                if w > Duration::from_millis(2) {
                    std::thread::sleep(w - Duration::from_millis(2));
                } else {
                    std::thread::yield_now();
                }
            }
            let mut ptrs = [chans[0].as_mut_ptr(), chans[1].as_mut_ptr()];
            let mut bus = AudioBusBuffers {
                num_channels: 2,
                silence_flags: 0,
                channel_buffers: ptrs.as_mut_ptr(),
            };
            let mut data = ProcessData {
                process_mode: K_REALTIME,
                symbolic_sample_size: K_SAMPLE32,
                num_samples: BLOCK as i32,
                num_inputs: 0,
                num_outputs: 1,
                inputs: std::ptr::null_mut(),
                outputs: &mut bus,
                input_parameter_changes: std::ptr::null_mut(),
                output_parameter_changes: std::ptr::null_mut(),
                input_events: std::ptr::null_mut(),
                output_events: std::ptr::null_mut(),
                process_context: std::ptr::null_mut(),
            };
            unsafe { ((*pvt).process)(proc_, &mut data) };
            let m = chans.iter().flatten().fold(0f32, |a, x| a.max(x.abs()));
            if m > f32::from_bits(peak.load(Ordering::Relaxed)) {
                peak.store(m.to_bits(), Ordering::Relaxed);
            }
            i += 1;
        }
    })
}
