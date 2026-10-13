//! エディタ (`IPlugView`)。
//!
//! プラグイン本体と同じく、vtable ポインタを構造体の先頭に並べて C++ の
//! 多重継承と同じレイアウトを手で作る。持つのは `IPlugView` と
//! `IPlugViewContentScaleSupport` の 2 つ。
//!
//! 画面の中身は `app.rs`、ウィンドウと GL は `window.rs`。

mod app;
mod keyboard;
mod widgets;
mod window;

use crate::control::Control;
use crate::vst3::*;
use app::App;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use window::Window;
use windows::Win32::Foundation::HWND;

/// エディタの大きさ (point)。
pub const SIZE_PT: [f32; 2] = [1120.0, 700.0];

/// 鍵盤を最初から出しておくか。単体版が立てる (DAW の中では最初は閉じておく)。
static KEYBOARD_BY_DEFAULT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 単体版から。これ以降に開くエディタは鍵盤を出した状態で開く。
pub fn show_keyboard_by_default() {
    KEYBOARD_BY_DEFAULT.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn keyboard_by_default() -> bool {
    KEYBOARD_BY_DEFAULT.load(std::sync::atomic::Ordering::Relaxed)
}

#[repr(C)]
pub struct View {
    view_vtbl: *const IPlugViewVtbl,
    scale_vtbl: *const IPlugViewContentScaleSupportVtbl,
    refs: AtomicU32,
    ctrl: Arc<Control>,
    window: Option<Window>,
    frame: *mut IPlugFrame,
    /// ホストが申告したスケール。申告が無ければ 0 (ウィンドウの DPI に従う)。
    scale: f32,
}

macro_rules! view {
    ($this:expr, $field:ident) => {
        ($this as *mut u8).sub(std::mem::offset_of!(View, $field)) as *mut View
    };
}

impl View {
    /// 参照 1 で返す。
    pub fn create(ctrl: Arc<Control>) -> *mut c_void {
        let v = Box::new(View {
            view_vtbl: &VIEW_VTBL,
            scale_vtbl: &SCALE_VTBL,
            refs: AtomicU32::new(1),
            ctrl,
            window: None,
            frame: std::ptr::null_mut(),
            scale: 0.0,
        });
        Box::into_raw(v) as *mut c_void
    }

    unsafe fn query(this: *mut View, iid: *const Tuid, out: *mut *mut c_void) -> Tresult {
        if out.is_null() || iid.is_null() {
            return K_INVALID_ARGUMENT;
        }
        unsafe {
            *out = std::ptr::null_mut();
            let p = if tuid_eq(iid, &IID_FUNKNOWN) || tuid_eq(iid, &IID_IPLUG_VIEW) {
                std::ptr::addr_of!((*this).view_vtbl) as *mut c_void
            } else if tuid_eq(iid, &IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT) {
                std::ptr::addr_of!((*this).scale_vtbl) as *mut c_void
            } else {
                return K_NO_INTERFACE;
            };
            Self::add_ref(this);
            *out = p;
            K_RESULT_OK
        }
    }

    unsafe fn add_ref(this: *mut View) -> u32 {
        unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
    }

    unsafe fn release(this: *mut View) -> u32 {
        let n = unsafe { (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1 };
        if n == 0 {
            unsafe {
                // removed() を呼ばずに捨てるホストもある。窓はここで必ず畳む。
                (*this).window = None;
                set_frame(this, std::ptr::null_mut());
                drop(Box::from_raw(this));
            }
        }
        n
    }

    fn size_px(&self) -> ViewRect {
        let ppp = match &self.window {
            Some(w) => w.ppp(),
            None if self.scale > 0.0 => self.scale,
            None => 1.0,
        };
        ViewRect {
            left: 0,
            top: 0,
            right: (SIZE_PT[0] * ppp).round() as i32,
            bottom: (SIZE_PT[1] * ppp).round() as i32,
        }
    }
}

unsafe fn set_frame(this: *mut View, frame: *mut IPlugFrame) {
    unsafe {
        let old = (*this).frame;
        if !frame.is_null() {
            ((*(*frame).vtbl).add_ref)(frame as *mut c_void);
        }
        (*this).frame = frame;
        if !old.is_null() {
            ((*(*old).vtbl).release)(old as *mut c_void);
        }
    }
}

// ---- IPlugView ------------------------------------------------------------

unsafe extern "system" fn v_query(this: *mut c_void, iid: *const Tuid, out: *mut *mut c_void) -> Tresult {
    unsafe { View::query(view!(this, view_vtbl), iid, out) }
}
unsafe extern "system" fn v_add_ref(this: *mut c_void) -> u32 {
    unsafe { View::add_ref(view!(this, view_vtbl)) }
}
unsafe extern "system" fn v_release(this: *mut c_void) -> u32 {
    unsafe { View::release(view!(this, view_vtbl)) }
}

unsafe fn is_hwnd(ty: *const u8) -> bool {
    if ty.is_null() {
        return false;
    }
    unsafe { std::ffi::CStr::from_ptr(ty as *const std::ffi::c_char) }.to_bytes() == PLATFORM_TYPE_HWND.to_bytes()
}

unsafe extern "system" fn v_is_platform_type_supported(_this: *mut c_void, ty: *const u8) -> Tresult {
    if unsafe { is_hwnd(ty) } {
        K_RESULT_TRUE
    } else {
        K_RESULT_FALSE
    }
}

unsafe extern "system" fn v_attached(this: *mut c_void, parent: *mut c_void, ty: *const u8) -> Tresult {
    if parent.is_null() || !unsafe { is_hwnd(ty) } {
        return K_RESULT_FALSE;
    }
    let v = unsafe { &mut *view!(this, view_vtbl) };
    v.window = None;
    let app = App::new(v.ctrl.clone());
    match Window::open(HWND(parent), app, SIZE_PT, v.scale) {
        Ok(w) => {
            let px = w.size_px();
            v.window = Some(w);
            // DPI から決めた大きさがホストの思っている大きさと違えば頼む。
            let frame = v.frame;
            if !frame.is_null() {
                let mut r = ViewRect {
                    left: 0,
                    top: 0,
                    right: px[0] as i32,
                    bottom: px[1] as i32,
                };
                unsafe {
                    ((*(*frame).vtbl).resize_view)(frame as *mut c_void, this, &mut r);
                }
            }
            K_RESULT_OK
        }
        Err(e) => {
            if let Ok(mut s) = v.ctrl.status.lock() {
                *s = format!("エディタを開けない: {e}");
            }
            K_RESULT_FALSE
        }
    }
}

unsafe extern "system" fn v_removed(this: *mut c_void) -> Tresult {
    let v = unsafe { &mut *view!(this, view_vtbl) };
    v.window = None;
    K_RESULT_OK
}

unsafe extern "system" fn v_on_wheel(_this: *mut c_void, _d: f32) -> Tresult {
    // ホイールは子ウィンドウが WM_MOUSEWHEEL で直接受ける。
    K_RESULT_FALSE
}

unsafe extern "system" fn v_on_key(_this: *mut c_void, _key: u16, _code: i16, _mods: i16) -> Tresult {
    K_RESULT_FALSE
}

unsafe extern "system" fn v_get_size(this: *mut c_void, size: *mut ViewRect) -> Tresult {
    if size.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let v = unsafe { &*view!(this, view_vtbl) };
    unsafe { *size = v.size_px() };
    K_RESULT_OK
}

unsafe extern "system" fn v_on_size(this: *mut c_void, size: *mut ViewRect) -> Tresult {
    if size.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let v = unsafe { &mut *view!(this, view_vtbl) };
    let r = unsafe { *size };
    if let Some(w) = v.window.as_mut() {
        w.resize(r.width(), r.height());
    }
    K_RESULT_OK
}

unsafe extern "system" fn v_on_focus(_this: *mut c_void, _state: u8) -> Tresult {
    K_RESULT_OK
}

unsafe extern "system" fn v_set_frame(this: *mut c_void, frame: *mut c_void) -> Tresult {
    unsafe { set_frame(view!(this, view_vtbl), frame as *mut IPlugFrame) };
    K_RESULT_OK
}

unsafe extern "system" fn v_can_resize(_this: *mut c_void) -> Tresult {
    // 中身は固定レイアウト。DPI による拡大縮小だけ受ける。
    K_RESULT_FALSE
}

unsafe extern "system" fn v_check_size_constraint(this: *mut c_void, rect: *mut ViewRect) -> Tresult {
    if rect.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let v = unsafe { &*view!(this, view_vtbl) };
    let s = v.size_px();
    unsafe {
        (*rect).right = (*rect).left + s.width();
        (*rect).bottom = (*rect).top + s.height();
    }
    K_RESULT_OK
}

static VIEW_VTBL: IPlugViewVtbl = IPlugViewVtbl {
    query_interface: v_query,
    add_ref: v_add_ref,
    release: v_release,
    is_platform_type_supported: v_is_platform_type_supported,
    attached: v_attached,
    removed: v_removed,
    on_wheel: v_on_wheel,
    on_key_down: v_on_key,
    on_key_up: v_on_key,
    get_size: v_get_size,
    on_size: v_on_size,
    on_focus: v_on_focus,
    set_frame: v_set_frame,
    can_resize: v_can_resize,
    check_size_constraint: v_check_size_constraint,
};

// ---- IPlugViewContentScaleSupport -----------------------------------------

unsafe extern "system" fn s_query(this: *mut c_void, iid: *const Tuid, out: *mut *mut c_void) -> Tresult {
    unsafe { View::query(view!(this, scale_vtbl), iid, out) }
}
unsafe extern "system" fn s_add_ref(this: *mut c_void) -> u32 {
    unsafe { View::add_ref(view!(this, scale_vtbl)) }
}
unsafe extern "system" fn s_release(this: *mut c_void) -> u32 {
    unsafe { View::release(view!(this, scale_vtbl)) }
}

unsafe extern "system" fn s_set_content_scale_factor(this: *mut c_void, factor: f32) -> Tresult {
    let v = unsafe { &mut *view!(this, scale_vtbl) };
    if !(0.5..=4.0).contains(&factor) {
        return K_RESULT_FALSE;
    }
    v.scale = factor;
    let view_this = unsafe { std::ptr::addr_of_mut!((*view!(this, scale_vtbl)).view_vtbl) } as *mut c_void;
    if let Some(w) = v.window.as_mut() {
        w.set_scale(factor);
        let r = v.size_px();
        let frame = v.frame;
        if !frame.is_null() {
            let mut r = r;
            unsafe {
                ((*(*frame).vtbl).resize_view)(frame as *mut c_void, view_this, &mut r);
            }
        }
    }
    K_RESULT_OK
}

static SCALE_VTBL: IPlugViewContentScaleSupportVtbl = IPlugViewContentScaleSupportVtbl {
    query_interface: s_query,
    add_ref: s_add_ref,
    release: s_release,
    set_content_scale_factor: s_set_content_scale_factor,
};
