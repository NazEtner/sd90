//! `sd90-editor` — DAW 無しでエディタだけを使う単体版。
//!
//! SD-90 は本体に OUTPUT / ヘッドホン端子があるので、音色やエフェクトを
//! いじるだけなら DAW は要らない。ここでは DAW がやっていることを最小限
//! なぞる: プラグイン (同じクレートの中身をそのまま) を生成し、別スレッドで
//! `process` を回し、エディタを自前のウィンドウに `attached` する。
//! エディタからの送信は `process` 経由でしか本体に出ないので、`process` を
//! 回すこと自体が必須 (`control.rs` の頭を参照)。
//!
//! 設定は `%APPDATA%\sd90-vst\standalone.state` に `getState` の中身を置き、
//! 次に開いたときに `setState` で戻す (送り直しはプラグイン側がやる)。
//!
//! 音はいつもの出力デバイス (WASAPI 共有モード) にも出す (`audio.rs`)。
//!
//! ASIO ドライバや DAW の中のプラグインと同時には使えない。本体の録音バスは
//! 1 系統しかなく、掴めなければ OFFLINE のまま開く。

#![windows_subsystem = "windows"]

mod audio;
mod hostutil;

use hostutil::{MemStream, SendPtr, process_thread};
use sd90_vst::factory::GetPluginFactory;
use sd90_vst::plugin::CID_SD90;
use sd90_vst::vst3::*;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GetMessageW, MB_ICONERROR, MB_OK, MSG, MessageBoxW, PostQuitMessage,
    RegisterClassW, SW_SHOW, SWP_NOMOVE, SWP_NOZORDER, SetWindowPos, ShowWindow,
    TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WNDCLASSW, WS_CAPTION,
    WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, GW_CHILD, GetWindow, WM_SETFOCUS,
};
use windows::core::{HSTRING, w};

const STYLE: WINDOW_STYLE = WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);

fn state_path() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("APPDATA")?).join("sd90-vst");
    Some(dir.join("standalone.state"))
}

fn error_box(msg: &str) {
    unsafe {
        MessageBoxW(None, &HSTRING::from(msg), w!("SD-90 Editor"), MB_OK | MB_ICONERROR);
    }
}

unsafe extern "system" fn top_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_SETFOCUS {
        // キー入力 (画面の鍵盤) はエディタの子ウィンドウで受ける。
        if let Ok(child) = unsafe { GetWindow(hwnd, GW_CHILD) } {
            unsafe {
                let _ = SetFocus(Some(child));
            }
        }
        return LRESULT(0);
    }
    if msg == WM_CLOSE {
        // 窓はまだ壊さない。エディタを先に外してから畳む (run の後半)。
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// クライアント領域が `w` x `h` になるように外枠を合わせる。
fn fit(top: HWND, w: i32, h: i32) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    unsafe {
        let _ = AdjustWindowRect(&mut r, STYLE, false);
        let _ = SetWindowPos(top, None, 0, 0, r.right - r.left, r.bottom - r.top, SWP_NOMOVE | SWP_NOZORDER);
    }
}

fn run() -> Result<(), String> {
    unsafe {
        // ぼやけないよう、モニタごとの DPI を自分で扱う。エディタは親の DPI から
        // 倍率を決める。
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    // ---- プラグインを生成 (DAW と同じ経路) ----
    let factory = GetPluginFactory();
    let fvt = unsafe { *(factory as *const *const IPluginFactory3Vtbl) };
    let mut comp: *mut c_void = std::ptr::null_mut();
    unsafe {
        ((*fvt).create_instance)(factory, CID_SD90.as_ptr(), IID_ICOMPONENT.as_ptr(), &mut comp);
    }
    if comp.is_null() {
        return Err("プラグインを生成できない".into());
    }
    let cvt = unsafe { *(comp as *const *const IComponentVtbl) };
    let mut proc_: *mut c_void = std::ptr::null_mut();
    let mut ctrl: *mut c_void = std::ptr::null_mut();
    unsafe {
        ((*cvt).initialize)(comp, std::ptr::null_mut());
        ((*cvt).query_interface)(comp, &IID_IAUDIO_PROCESSOR, &mut proc_);
        ((*cvt).query_interface)(comp, &IID_IEDIT_CONTROLLER, &mut ctrl);
    }
    if proc_.is_null() || ctrl.is_null() {
        return Err("必要なインタフェースが無い".into());
    }
    let pvt = unsafe { *(proc_ as *const *const IAudioProcessorVtbl) };
    let evt = unsafe { *(ctrl as *const *const IEditControllerVtbl) };

    // ---- 前回の設定 ----
    // setActive より前に戻す。「送り直す」が立っていれば、掴んだ時点で送られる。
    if let Some(data) = state_path().and_then(|p| std::fs::read(p).ok()) {
        let mut s = MemStream::new(data);
        unsafe { ((*cvt).set_state)(comp, &mut s as *mut MemStream as *mut c_void) };
    }

    // ---- 本体を掴んで process を回す ----
    let mut setup = ProcessSetup {
        process_mode: K_REALTIME,
        symbolic_sample_size: K_SAMPLE32,
        max_samples_per_block: audio::MAX_BLOCK as i32,
        sample_rate: 48000.0,
    };
    unsafe {
        ((*pvt).setup_processing)(proc_, &mut setup);
        ((*cvt).set_active)(comp, 1);
        ((*pvt).set_processing)(proc_, 1);
    }
    let stop = Arc::new(AtomicBool::new(false));
    // 出力デバイスの時計で process を回し、音をそこに出す。出力デバイスが
    // 無ければ、編集だけはできるようにタイマで回す。
    let (worker, audio_error) = match audio::start(SendPtr(proc_, pvt), stop.clone()) {
        Ok(h) => (h, None),
        Err(e) => (
            process_thread(SendPtr(proc_, pvt), stop.clone(), Arc::new(AtomicU32::new(0))),
            Some(e),
        ),
    };

    // ---- ウィンドウとエディタ ----
    // 単体版では鍵盤を最初から出しておく (DAW には鍵盤があるので、プラグインでは閉じておく)。
    sd90_vst::gui::show_keyboard_by_default();
    let view = unsafe { ((*evt).create_view)(ctrl, VIEW_TYPE_EDITOR.as_ptr() as *const u8) };
    if view.is_null() {
        return Err("エディタを作れない".into());
    }
    let vvt = unsafe { *(view as *const *const IPlugViewVtbl) };
    let mut size = ViewRect::default();
    unsafe { ((*vvt).get_size)(view, &mut size) };

    let hinst = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
    let wc = WNDCLASSW {
        lpfnWndProc: Some(top_proc),
        hInstance: hinst.into(),
        lpszClassName: w!("SD90EditorStandalone"),
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };
    let top = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("SD90EditorStandalone"),
            w!("SD-90 Editor"),
            STYLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            size.width(),
            size.height(),
            None,
            None,
            Some(hinst.into()),
            None,
        )
    }
    .map_err(|e| format!("ウィンドウを作れない: {e}"))?;
    fit(top, size.width(), size.height());

    if unsafe { ((*vvt).attached)(view, top.0, PLATFORM_TYPE_HWND.as_ptr() as *const u8) } != K_RESULT_OK {
        return Err("エディタを開けない (OpenGL 2.0 以上が要る)".into());
    }
    // attached でモニタの DPI に合わせた大きさが決まる。それに外枠を合わせる。
    unsafe { ((*vvt).get_size)(view, &mut size) };
    fit(top, size.width(), size.height());
    unsafe {
        let _ = ShowWindow(top, SW_SHOW);
    }
    if let Some(e) = audio_error {
        error_box(&format!("音声出力を開けない ({e})。
音は本体の OUTPUT / ヘッドホン端子から聞くこと。編集はそのまま使える。"));
    }

    // ---- メッセージループ ----
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // ---- 畳む ----
    unsafe {
        ((*vvt).removed)(view);
        ((*vvt).release)(view);
        let _ = DestroyWindow(top);
    }
    stop.store(true, Ordering::Relaxed);
    let _ = worker.join();

    // 設定を残す。本体へ送りかけのものはここで捨てる (次回送り直す)。
    let mut s = MemStream::new(Vec::new());
    if unsafe { ((*cvt).get_state)(comp, &mut s as *mut MemStream as *mut c_void) } == K_RESULT_OK
        && let Some(path) = state_path()
    {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, &s.data);
    }

    unsafe {
        ((*pvt).set_processing)(proc_, 0);
        ((*cvt).set_active)(comp, 0);
        ((*pvt).release)(proc_);
        ((*evt).release)(ctrl);
        ((*cvt).terminate)(comp);
        ((*cvt).release)(comp);
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        error_box(&e);
    }
}
