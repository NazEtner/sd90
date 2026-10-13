//! `editor` 用の最小ホスト (bin 側のみ)。
//!
//! DLL から `IEditController::createView("editor")` を引き、トップレベルの
//! ウィンドウに `attached` して、ホストと同じくこちらのメッセージループで
//! 回す。手順を文字列で渡すと、その通りにクリックしたり画面を撮ったりする。
//!
//! ```text
//! wait:500,shot:a.bmp,click:640,60,wait:300,shot:b.bmp
//! ```
//!
//! 座標はエディタの子ウィンドウのクライアント座標 (物理ピクセル)。
//! ついでに `getState` → `setState` の往復もここで確かめる。
//!
//! 手順の先頭に `active` を置くと、本物のホストと同じく別スレッドで
//! `process` を回す。エディタの操作が本体に届き、音が返ってくるかを
//! `peak:名前` (前回の peak からの出力の最大値) で確かめられる。

use crate::hostutil::{MemStream, SendPtr, process_thread};
use sd90_vst::plugin::CID_SD90;
use sd90_vst::vst3::*;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{FreeLibrary, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, ClientToScreen, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    SRCCOPY, SelectObject,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GW_CHILD,
    GetClientRect, GetWindow, MSG, PM_REMOVE, PeekMessageW, PostQuitMessage, RegisterClassW,
    SW_SHOW, SendMessageW, SetForegroundWindow, ShowWindow, TranslateMessage, WINDOW_EX_STYLE,
    WM_DESTROY, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WNDCLASSW, WS_CAPTION, WS_OVERLAPPED,
    WS_SYSMENU,
};
use windows::core::{PCSTR, PCWSTR, w};

type GetFactoryFn = unsafe extern "system" fn() -> *mut c_void;

fn u16str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---- トップレベルのウィンドウ -----------------------------------------------

unsafe extern "system" fn top_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn pump(d: Duration) {
    let until = Instant::now() + d;
    while Instant::now() < until {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 子ウィンドウの見えている絵を BMP で書き出す (画面から BitBlt)。
fn shot(child: HWND, path: &str) -> Result<(), String> {
    unsafe {
        let mut rc = RECT::default();
        GetClientRect(child, &mut rc).map_err(|e| e.to_string())?;
        let (w, h) = (rc.right, rc.bottom);
        let mut org = windows::Win32::Foundation::POINT::default();
        let _ = ClientToScreen(child, &mut org);
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        BitBlt(mem, 0, 0, w, h, Some(screen), org.x, org.y, SRCCOPY).map_err(|e| e.to_string())?;
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        GetDIBits(mem, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut c_void), &mut bi, DIB_RGB_COLORS);
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);

        let mut f = Vec::with_capacity(54 + px.len());
        f.extend_from_slice(b"BM");
        f.extend_from_slice(&(54 + px.len() as u32).to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
        f.extend_from_slice(&54u32.to_le_bytes());
        f.extend_from_slice(&40u32.to_le_bytes());
        f.extend_from_slice(&w.to_le_bytes());
        f.extend_from_slice(&h.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&32u16.to_le_bytes());
        f.extend_from_slice(&[0u8; 24]);
        f.extend_from_slice(&px);
        std::fs::write(path, f).map_err(|e| e.to_string())?;
    }
    println!("  撮影: {path}");
    Ok(())
}

fn click(child: HWND, x: i32, y: i32) {
    let lp = LPARAM(((y as isize & 0xFFFF) << 16) | (x as isize & 0xFFFF));
    unsafe {
        SendMessageW(child, WM_MOUSEMOVE, Some(WPARAM(0)), Some(lp));
    }
    pump(Duration::from_millis(80));
    unsafe {
        SendMessageW(child, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp));
    }
    pump(Duration::from_millis(80));
    unsafe {
        SendMessageW(child, WM_LBUTTONUP, Some(WPARAM(0)), Some(lp));
    }
    pump(Duration::from_millis(80));
}

pub fn run(dll: &str, script: &str) -> Result<(), String> {
    let module = unsafe { LoadLibraryW(PCWSTR(u16str(dll).as_ptr())) }
        .map_err(|e| format!("{dll} を読み込めない: {e}"))?;
    let get_factory: GetFactoryFn = unsafe {
        let p = GetProcAddress(module, PCSTR(c"GetPluginFactory".as_ptr() as *const u8))
            .ok_or("GetPluginFactory が無い")?;
        std::mem::transmute(p)
    };
    let factory = unsafe { get_factory() };
    let fvt = unsafe { *(factory as *const *const IPluginFactory3Vtbl) };
    let mut comp: *mut c_void = std::ptr::null_mut();
    unsafe {
        ((*fvt).create_instance)(factory, CID_SD90.as_ptr(), IID_ICOMPONENT.as_ptr(), &mut comp);
    }
    if comp.is_null() {
        return Err("createInstance に失敗".into());
    }
    let cvt = unsafe { *(comp as *const *const IComponentVtbl) };
    unsafe { ((*cvt).initialize)(comp, std::ptr::null_mut()) };
    let mut ctrl: *mut c_void = std::ptr::null_mut();
    unsafe { ((*cvt).query_interface)(comp, &IID_IEDIT_CONTROLLER, &mut ctrl) };
    let evt = unsafe { *(ctrl as *const *const IEditControllerVtbl) };

    // ---- 必要なら本体を掴んで process を回す ----
    let active = script.trim_start().starts_with("active");
    let stop = Arc::new(AtomicBool::new(false));
    let peak = Arc::new(AtomicU32::new(0));
    let mut worker = None;
    let mut proc_: *mut c_void = std::ptr::null_mut();
    let mut pvt: *const IAudioProcessorVtbl = std::ptr::null();
    if active {
        unsafe { ((*cvt).query_interface)(comp, &IID_IAUDIO_PROCESSOR, &mut proc_) };
        pvt = unsafe { *(proc_ as *const *const IAudioProcessorVtbl) };
        let mut setup = ProcessSetup {
            process_mode: K_REALTIME,
            symbolic_sample_size: K_SAMPLE32,
            max_samples_per_block: 256,
            sample_rate: 48000.0,
        };
        unsafe {
            ((*pvt).setup_processing)(proc_, &mut setup);
            ((*cvt).set_active)(comp, 1);
            ((*pvt).set_processing)(proc_, 1);
        }
        worker = Some(process_thread(SendPtr(proc_, pvt), stop.clone(), peak.clone()));
        println!("  setActive / process 開始");
    }

    // ---- createView ----
    let view = unsafe { ((*evt).create_view)(ctrl, c"editor".as_ptr() as *const u8) };
    if view.is_null() {
        return Err("createView が null を返した".into());
    }
    let vvt = unsafe { *(view as *const *const IPlugViewVtbl) };
    let hwnd_ok = unsafe { ((*vvt).is_platform_type_supported)(view, c"HWND".as_ptr() as *const u8) };
    println!("  isPlatformTypeSupported(HWND): {}", hwnd_ok == K_RESULT_TRUE);
    let mut r = ViewRect::default();
    unsafe { ((*vvt).get_size)(view, &mut r) };
    println!("  getSize: {} x {}", r.width(), r.height());

    let mut scale: *mut c_void = std::ptr::null_mut();
    unsafe { ((*vvt).query_interface)(view, &IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT, &mut scale) };
    println!("  IPlugViewContentScaleSupport: {}", !scale.is_null());
    if !scale.is_null() {
        let svt = unsafe { *(scale as *const *const IPlugViewContentScaleSupportVtbl) };
        unsafe { ((*svt).release)(scale) };
    }

    // ---- ウィンドウ ----
    let hinst = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
    let wc = WNDCLASSW {
        lpfnWndProc: Some(top_proc),
        hInstance: hinst.into(),
        lpszClassName: w!("SD90ProbeHost"),
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    let mut wr = RECT {
        left: 0,
        top: 0,
        right: r.width(),
        bottom: r.height(),
    };
    unsafe {
        let _ = AdjustWindowRect(&mut wr, style, false);
    }
    let top = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("SD90ProbeHost"),
            w!("sd90-vst-probe editor"),
            style,
            40,
            40,
            wr.right - wr.left,
            wr.bottom - wr.top,
            None,
            None,
            Some(hinst.into()),
            None,
        )
    }
    .map_err(|e| e.to_string())?;
    unsafe {
        let _ = ShowWindow(top, SW_SHOW);
        let _ = SetForegroundWindow(top);
    }
    let hr = unsafe { ((*vvt).attached)(view, top.0, c"HWND".as_ptr() as *const u8) };
    println!("  attached: {}", hr == K_RESULT_OK);
    if hr != K_RESULT_OK {
        return Err("attached に失敗".into());
    }
    let child = unsafe { GetWindow(top, GW_CHILD) }.map_err(|e| e.to_string())?;
    pump(Duration::from_millis(300));

    for step in script.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (cmd, arg) = step.split_once(':').unwrap_or((step, ""));
        match cmd {
            "active" => {}
            "wait" => pump(Duration::from_millis(arg.parse().unwrap_or(300))),
            "peak" => {
                let v = f32::from_bits(peak.swap(0, Ordering::Relaxed));
                println!("  peak[{arg}]: {v:.4} ({:.1} dBFS)", 20.0 * v.max(1e-9).log10());
            }
            "shot" => shot(child, arg)?,
            "click" => {
                let mut it = arg.split(':').flat_map(|s| s.split(' ')).filter_map(|s| s.parse().ok());
                let x = it.next().unwrap_or(0);
                // "click:x:y" と "click:x y" のどちらでも。
                let y = it.next().unwrap_or(0);
                click(child, x, y);
            }
            _ => println!("  知らない手順: {step}"),
        }
    }

    // ---- 状態の往復 ----
    let mut s1 = MemStream::new(Vec::new());
    let hr = unsafe { ((*cvt).get_state)(comp, &mut s1 as *mut MemStream as *mut c_void) };
    println!("  getState: {} バイト ({})", s1.data.len(), if hr == K_RESULT_OK { "ok" } else { "失敗" });
    let mut s2 = MemStream::new(s1.data.clone());
    unsafe { ((*cvt).set_state)(comp, &mut s2 as *mut MemStream as *mut c_void) };
    let mut s3 = MemStream::new(Vec::new());
    unsafe { ((*cvt).get_state)(comp, &mut s3 as *mut MemStream as *mut c_void) };
    println!("  setState → getState で一致: {}", s1.data == s3.data);

    // ---- 片付け ----
    if let Some(h) = worker.take() {
        stop.store(true, Ordering::Relaxed);
        let _ = h.join();
        unsafe {
            ((*pvt).set_processing)(proc_, 0);
            ((*cvt).set_active)(comp, 0);
            ((*pvt).release)(proc_);
        }
    }
    unsafe {
        ((*vvt).removed)(view);
        ((*vvt).release)(view);
        let _ = DestroyWindow(top);
    }
    pump(Duration::from_millis(50));
    unsafe {
        ((*evt).release)(ctrl);
        ((*cvt).terminate)(comp);
        let n = ((*cvt).release)(comp);
        println!("  最後の release 後の参照数: {n}");
        let _ = FreeLibrary(module);
    }
    Ok(())
}
