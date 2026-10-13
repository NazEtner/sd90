//! ホストがくれた HWND の下に子ウィンドウを作り、WGL の上で egui を回す。
//!
//! メッセージループはホストのものに乗る。こちらはウィンドウプロシージャで
//! 入力を egui の `RawInput` に積み、`WM_TIMER` で 1 フレーム回して描く。
//!
//! # 気をつけるところ
//!
//! * **ウィンドウクラスは DLL の HINSTANCE で登録し、最後のウィンドウを
//!   閉じたら外す。** EXE の HINSTANCE で登録したままにすると、DLL が
//!   アンロードされた後にクラスの WNDPROC が宙に浮き、同じプロセスで
//!   もう一度開いた瞬間に落ちる。
//! * **GL のコンテキストは描く前に必ず current にし、終わったら元に戻す。**
//!   同じスレッドで他のプラグインやホスト自身が GL を使っている。
//! * VST3 の Windows 版は座標が物理ピクセル。egui の point との換算は
//!   `pixels_per_point` だけで行う。

use super::app::App;
use egui_glow::Painter;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetDC, HDC, PAINTSTRUCT, ReleaseDC, ValidateRect,
};
use windows::Win32::Graphics::OpenGL::{
    ChoosePixelFormat, HGLRC, PFD_DOUBLEBUFFER, PFD_DRAW_TO_WINDOW, PFD_MAIN_PLANE,
    PFD_SUPPORT_OPENGL, PFD_TYPE_RGBA, PIXELFORMATDESCRIPTOR, SetPixelFormat, SwapBuffers,
    wglCreateContext, wglDeleteContext, wglGetCurrentContext, wglGetCurrentDC, wglGetProcAddress,
    wglMakeCurrent,
};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleHandleExW, GetProcAddress, LoadLibraryA,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT,
    TrackMouseEvent, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE,
    VK_HOME, VK_LEFT, VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_OWNDC, CreateWindowExW, DLGC_WANTALLKEYS, DefWindowProcW, DestroyWindow, GWLP_USERDATA,
    GetClientRect, GetWindowLongPtrW, HCURSOR, IDC_ARROW, IDC_HAND, IDC_IBEAM, IDC_SIZENS,
    IDC_SIZEWE, KillTimer, LoadCursorW, RegisterClassW, SWP_NOMOVE, SWP_NOZORDER, SetCursor,
    SetTimer, SetWindowLongPtrW, SetWindowPos, UnregisterClassW, WHEEL_DELTA, WINDOW_EX_STYLE,
    WM_CHAR, WM_DESTROY, WM_GETDLGCODE, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WM_TIMER, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_VISIBLE,
};
use windows::core::{PCSTR, PCWSTR, w};

const CLASS_NAME: PCWSTR = w!("SD90VstEditor");
const TIMER_ID: usize = 1;
/// windows クレートでは KeyboardAndMouse 側にあるので自前で持つ。
const WM_MOUSELEAVE: u32 = 0x02A3;
/// 描画間隔。メータを動かすので常に回す。
const FRAME_MS: u32 = 33;

/// クラスを使っているウィンドウの数。0 になったら登録を外す。
static CLASS_USERS: Mutex<usize> = Mutex::new(0);

/// この DLL 自身の HMODULE。
fn this_module() -> HMODULE {
    let mut m = HMODULE::default();
    unsafe {
        let _ = GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(this_module as *const () as *const u16),
            &mut m,
        );
    }
    m
}

fn register_class() -> Result<(), String> {
    let mut users = CLASS_USERS.lock().map_err(|_| "クラスの登録表が壊れている")?;
    if *users == 0 {
        let wc = WNDCLASSW {
            style: CS_OWNDC,
            lpfnWndProc: Some(wndproc),
            hInstance: this_module().into(),
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if unsafe { RegisterClassW(&wc) } == 0 {
            return Err("ウィンドウクラスを登録できない".into());
        }
    }
    *users += 1;
    Ok(())
}

fn unregister_class() {
    if let Ok(mut users) = CLASS_USERS.lock() {
        *users = users.saturating_sub(1);
        if *users == 0 {
            unsafe {
                let _ = UnregisterClassW(CLASS_NAME, Some(this_module().into()));
            }
        }
    }
}

/// 子ウィンドウ 1 枚ぶん。`Box` に入れてアドレスを `GWLP_USERDATA` に置く。
struct State {
    hwnd: HWND,
    hdc: HDC,
    glrc: HGLRC,
    painter: Option<Painter>,
    ctx: egui::Context,
    app: App,
    input: egui::RawInput,
    start: Instant,
    /// 物理ピクセル / point。
    ppp: f32,
    size_px: [u32; 2],
    pointer: egui::Pos2,
    tracking: bool,
    cursor: egui::CursorIcon,
}

pub struct Window {
    state: *mut State,
}

impl Window {
    /// `parent` の下に作る。`size_pt` は point 単位、`scale` はホストの
    /// 申告したスケール (無ければ 0)。
    pub fn open(parent: HWND, app: App, size_pt: [f32; 2], scale: f32) -> Result<Window, String> {
        register_class()?;
        let w = match Self::create(parent, app, size_pt, scale) {
            Ok(w) => w,
            Err(e) => {
                unregister_class();
                return Err(e);
            }
        };
        Ok(w)
    }

    fn create(parent: HWND, app: App, size_pt: [f32; 2], scale: f32) -> Result<Window, String> {
        let dpi = unsafe { GetDpiForWindow(parent) };
        let ppp = if scale > 0.0 {
            scale
        } else if dpi > 0 {
            dpi as f32 / 96.0
        } else {
            1.0
        };
        let (wpx, hpx) = ((size_pt[0] * ppp).round() as i32, (size_pt[1] * ppp).round() as i32);
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS_NAME,
                w!("SD-90"),
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                wpx,
                hpx,
                Some(parent),
                None,
                Some(this_module().into()),
                None,
            )
        }
        .map_err(|e| format!("子ウィンドウを作れない: {e}"))?;

        let (hdc, glrc) = match unsafe { create_gl(hwnd) } {
            Ok(v) => v,
            Err(e) => {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return Err(e);
            }
        };

        let ctx = egui::Context::default();
        app.setup(&ctx);
        let mut st = Box::new(State {
            hwnd,
            hdc,
            glrc,
            painter: None,
            ctx,
            app,
            input: egui::RawInput::default(),
            start: Instant::now(),
            ppp,
            size_px: [wpx as u32, hpx as u32],
            pointer: egui::Pos2::ZERO,
            tracking: false,
            cursor: egui::CursorIcon::Default,
        });

        // painter は GL が current の間に作る。
        let painter = unsafe {
            with_current(hdc, glrc, || {
                let gl = glow::Context::from_loader_function_cstr(|name| gl_proc(name));
                Painter::new(Arc::new(gl), "", None, false)
            })
        };
        match painter {
            Ok(p) => st.painter = Some(p),
            Err(e) => {
                unsafe {
                    let _ = wglDeleteContext(glrc);
                    ReleaseDC(Some(hwnd), hdc);
                    let _ = DestroyWindow(hwnd);
                }
                return Err(format!("OpenGL の初期化に失敗: {e}"));
            }
        }

        let ptr = Box::into_raw(st);
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
            SetTimer(Some(hwnd), TIMER_ID, FRAME_MS, None);
        }
        Ok(Window { state: ptr })
    }

    /// point 単位の大きさ → 物理ピクセルの大きさ。
    pub fn size_px(&self) -> [u32; 2] {
        unsafe { (*self.state).size_px }
    }

    pub fn ppp(&self) -> f32 {
        unsafe { (*self.state).ppp }
    }

    /// ホストが大きさを決めたとき (onSize)。
    pub fn resize(&mut self, w: i32, h: i32) {
        unsafe {
            let st = &mut *self.state;
            st.size_px = [w.max(1) as u32, h.max(1) as u32];
            let _ = SetWindowPos(st.hwnd, None, 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER);
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        unsafe {
            (*self.state).ppp = scale;
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            let st = Box::from_raw(self.state);
            let hwnd = st.hwnd;
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            let mut st = st;
            st.app.closing();
            if let Some(mut p) = st.painter.take() {
                let (hdc, glrc) = (st.hdc, st.glrc);
                with_current(hdc, glrc, || p.destroy());
            }
            let _ = wglDeleteContext(st.glrc);
            ReleaseDC(Some(hwnd), st.hdc);
            let _ = DestroyWindow(hwnd);
            drop(st);
        }
        unregister_class();
    }
}

unsafe fn create_gl(hwnd: HWND) -> Result<(HDC, HGLRC), String> {
    unsafe {
        let hdc = GetDC(Some(hwnd));
        let pfd = PIXELFORMATDESCRIPTOR {
            nSize: std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16,
            nVersion: 1,
            dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
            iPixelType: PFD_TYPE_RGBA,
            cColorBits: 32,
            cAlphaBits: 8,
            cDepthBits: 0,
            cStencilBits: 8,
            iLayerType: PFD_MAIN_PLANE.0 as u8,
            ..Default::default()
        };
        let fmt = ChoosePixelFormat(hdc, &pfd);
        if fmt == 0 || SetPixelFormat(hdc, fmt, &pfd).is_err() {
            ReleaseDC(Some(hwnd), hdc);
            return Err("ピクセルフォーマットを設定できない".into());
        }
        let glrc = match wglCreateContext(hdc) {
            Ok(g) => g,
            Err(e) => {
                ReleaseDC(Some(hwnd), hdc);
                return Err(format!("GL コンテキストを作れない: {e}"));
            }
        };
        Ok((hdc, glrc))
    }
}

/// `f` の間だけこちらのコンテキストを current にする。元のは戻す。
unsafe fn with_current<R>(hdc: HDC, glrc: HGLRC, f: impl FnOnce() -> R) -> R {
    unsafe {
        let (old_dc, old_rc) = (wglGetCurrentDC(), wglGetCurrentContext());
        let _ = wglMakeCurrent(hdc, glrc);
        let r = f();
        let _ = wglMakeCurrent(old_dc, old_rc);
        r
    }
}

/// GL の関数を引く。1.1 までは opengl32.dll にしか無い。
fn gl_proc(name: &std::ffi::CStr) -> *const c_void {
    unsafe {
        let p = PCSTR(name.as_ptr() as *const u8);
        if let Some(f) = wglGetProcAddress(p) {
            let a = f as usize;
            // 実装によっては失敗を 1, 2, 3, -1 で返す。
            if a > 3 && a != usize::MAX {
                return a as *const c_void;
            }
        }
        match LoadLibraryA(windows::core::s!("opengl32.dll")) {
            Ok(m) => GetProcAddress(m, p).map_or(std::ptr::null(), |f| f as *const c_void),
            Err(_) => std::ptr::null(),
        }
    }
}

fn modifiers() -> egui::Modifiers {
    let down = |vk: VIRTUAL_KEY| unsafe { GetKeyState(vk.0 as i32) } < 0;
    let ctrl = down(VK_CONTROL);
    egui::Modifiers {
        alt: down(VK_MENU),
        ctrl,
        shift: down(VK_SHIFT),
        mac_cmd: false,
        command: ctrl,
    }
}

fn key_of(vk: u16) -> Option<egui::Key> {
    use egui::Key;
    let k = VIRTUAL_KEY(vk);
    Some(match k {
        VK_LEFT => Key::ArrowLeft,
        VK_RIGHT => Key::ArrowRight,
        VK_UP => Key::ArrowUp,
        VK_DOWN => Key::ArrowDown,
        VK_BACK => Key::Backspace,
        VK_DELETE => Key::Delete,
        VK_RETURN => Key::Enter,
        VK_ESCAPE => Key::Escape,
        VK_TAB => Key::Tab,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        _ => match vk {
            // VK_OEM_COMMA。画面の鍵盤の下の段の終わり (C の 1 オクターブ上)。
            0xBC => Key::Comma,
            0x30..=0x39 => Key::from_name(&((vk - 0x30) as u8 as char).to_string())?,
            0x41..=0x5A => Key::from_name(&(vk as u8 as char).to_string())?,
            _ => return None,
        },
    })
}

fn lparam_pos(lp: LPARAM, ppp: f32) -> egui::Pos2 {
    let x = (lp.0 & 0xFFFF) as i16 as f32;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32;
    egui::pos2(x / ppp, y / ppp)
}

impl State {
    fn frame(&mut self) {
        let Some(painter) = self.painter.as_mut() else {
            return;
        };
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let (w, h) = ((rc.right - rc.left).max(1) as u32, (rc.bottom - rc.top).max(1) as u32);
        self.size_px = [w, h];

        let mut input = std::mem::take(&mut self.input);
        input.time = Some(self.start.elapsed().as_secs_f64());
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(w as f32 / self.ppp, h as f32 / self.ppp),
        ));
        input.focused = true;
        input.modifiers = modifiers();
        input.max_texture_side = Some(painter.max_texture_side());
        if let Some(vp) = input.viewports.get_mut(&input.viewport_id) {
            vp.native_pixels_per_point = Some(self.ppp);
        }
        self.ctx.set_pixels_per_point(self.ppp);

        let app = &mut self.app;
        let out = self.ctx.run(input, |ctx| app.ui(ctx));
        self.cursor = out.platform_output.cursor_icon;
        let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);

        let (hdc, glrc, ppp) = (self.hdc, self.glrc, out.pixels_per_point);
        unsafe {
            with_current(hdc, glrc, || {
                painter.clear([w, h], [0.0, 0.0, 0.0, 1.0]);
                painter.paint_and_update_textures([w, h], ppp, &prims, &out.textures_delta);
                let _ = SwapBuffers(hdc);
            });
        }
    }

    fn push(&mut self, e: egui::Event) {
        self.input.events.push(e);
    }

    fn button(&mut self, b: egui::PointerButton, pressed: bool, lp: LPARAM) {
        self.pointer = lparam_pos(lp, self.ppp);
        unsafe {
            if pressed {
                SetCapture(self.hwnd);
                // キー入力 (数値の直接入力) を受けるためにフォーカスを取る。
                let _ = SetFocus(Some(self.hwnd));
            } else {
                let _ = ReleaseCapture();
            }
        }
        let modifiers = modifiers();
        self.push(egui::Event::PointerButton {
            pos: self.pointer,
            button: b,
            pressed,
            modifiers,
        });
    }

    fn set_cursor(&self) -> bool {
        use egui::CursorIcon as C;
        let id = match self.cursor {
            C::PointingHand => IDC_HAND,
            C::Text => IDC_IBEAM,
            C::ResizeHorizontal | C::ResizeColumn => IDC_SIZEWE,
            C::ResizeVertical | C::ResizeRow => IDC_SIZENS,
            _ => IDC_ARROW,
        };
        unsafe {
            let c: HCURSOR = LoadCursorW(None, id).unwrap_or_default();
            SetCursor(Some(c));
        }
        true
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut State;
    if p.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    let st = unsafe { &mut *p };
    match msg {
        WM_TIMER if wp.0 == TIMER_ID => {
            st.frame();
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                BeginPaint(hwnd, &mut ps);
            }
            st.frame();
            unsafe {
                let _ = EndPaint(hwnd, &ps);
                let _ = ValidateRect(Some(hwnd), None);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if !st.tracking {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                unsafe {
                    let _ = TrackMouseEvent(&mut tme);
                }
                st.tracking = true;
            }
            st.pointer = lparam_pos(lp, st.ppp);
            let pos = st.pointer;
            st.push(egui::Event::PointerMoved(pos));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            st.tracking = false;
            st.push(egui::Event::PointerGone);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            st.button(egui::PointerButton::Primary, true, lp);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            st.button(egui::PointerButton::Primary, false, lp);
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            st.button(egui::PointerButton::Secondary, true, lp);
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            st.button(egui::PointerButton::Secondary, false, lp);
            LRESULT(0)
        }
        WM_MBUTTONDOWN => {
            st.button(egui::PointerButton::Middle, true, lp);
            LRESULT(0)
        }
        WM_MBUTTONUP => {
            st.button(egui::PointerButton::Middle, false, lp);
            LRESULT(0)
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let d = ((wp.0 >> 16) & 0xFFFF) as i16 as f32 / WHEEL_DELTA as f32;
            let delta = if msg == WM_MOUSEWHEEL {
                egui::vec2(0.0, d)
            } else {
                egui::vec2(-d, 0.0)
            };
            let modifiers = modifiers();
            st.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta,
                modifiers,
            });
            LRESULT(0)
        }
        WM_GETDLGCODE => LRESULT(DLGC_WANTALLKEYS as isize),
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            let pressed = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
            if let Some(key) = key_of(wp.0 as u16) {
                let modifiers = modifiers();
                st.push(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: pressed && (lp.0 >> 30) & 1 == 1,
                    modifiers,
                });
            }
            LRESULT(0)
        }
        WM_CHAR => {
            if let Some(c) = char::from_u32(wp.0 as u32)
                && !c.is_control()
            {
                st.push(egui::Event::Text(c.to_string()));
            }
            LRESULT(0)
        }
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == 1 => {
            // HTCLIENT のときだけ egui のカーソルにする。
            st.set_cursor();
            LRESULT(1)
        }
        WM_DESTROY => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_ID);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
