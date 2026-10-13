//! in-proc COM サーバの外殻。クラスファクトリと DLL エクスポート、
//! および regsvr32 用の自己登録。

use crate::asio::{
    AsioDriver, CLSID_SD90_ASIO, DRIVER_NAME, driver_query_interface, driver_release,
};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_NOINTERFACE, E_POINTER, S_FALSE,
    S_OK,
};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows::Win32::System::Registry::*;
use windows::core::{GUID, HRESULT, PCWSTR};

const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
const IID_ICLASSFACTORY: GUID = GUID::from_u128(0x00000001_0000_0000_c000_000000000046);

static MODULE_LOCKS: AtomicU32 = AtomicU32::new(0);

pub fn lock_module() {
    MODULE_LOCKS.fetch_add(1, Ordering::Relaxed);
}

pub fn unlock_module() {
    MODULE_LOCKS.fetch_sub(1, Ordering::Relaxed);
}

// ---- クラスファクトリ -----------------------------------------------------

#[repr(C)]
struct IClassFactoryVtbl {
    query_interface:
        unsafe extern "system" fn(*mut ClassFactory, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut ClassFactory) -> u32,
    release: unsafe extern "system" fn(*mut ClassFactory) -> u32,
    create_instance: unsafe extern "system" fn(
        *mut ClassFactory,
        *mut c_void,
        *const GUID,
        *mut *mut c_void,
    ) -> HRESULT,
    lock_server: unsafe extern "system" fn(*mut ClassFactory, i32) -> HRESULT,
}

#[repr(C)]
struct ClassFactory {
    vtbl: *const IClassFactoryVtbl,
    refs: AtomicU32,
}

static FACTORY_VTBL: IClassFactoryVtbl = IClassFactoryVtbl {
    query_interface: cf_query_interface,
    add_ref: cf_add_ref,
    release: cf_release,
    create_instance: cf_create_instance,
    lock_server: cf_lock_server,
};

unsafe extern "system" fn cf_query_interface(
    this: *mut ClassFactory,
    riid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    unsafe {
        *out = std::ptr::null_mut();
        let iid = *riid;
        if iid == IID_IUNKNOWN || iid == IID_ICLASSFACTORY {
            cf_add_ref(this);
            *out = this as *mut c_void;
            return S_OK;
        }
    }
    E_NOINTERFACE
}

unsafe extern "system" fn cf_add_ref(this: *mut ClassFactory) -> u32 {
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn cf_release(this: *mut ClassFactory) -> u32 {
    let n = unsafe { (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1 };
    if n == 0 {
        unsafe { drop(Box::from_raw(this)) };
        unlock_module();
    }
    n
}

unsafe extern "system" fn cf_create_instance(
    _this: *mut ClassFactory,
    outer: *mut c_void,
    riid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    crate::log::log("cf_create_instance");
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    unsafe {
        *out = std::ptr::null_mut();
        let obj = AsioDriver::new();
        if obj.is_null() {
            return E_FAIL;
        }
        // ASIO ホストは IID に CLSID を渡してくる。判定は driver 側に任せる。
        let hr = driver_query_interface(obj, riid, out);
        // 成功していれば参照が 2 になっているので、生成時の 1 を落とす。
        driver_release(obj);
        hr
    }
}

unsafe extern "system" fn cf_lock_server(_this: *mut ClassFactory, lock: i32) -> HRESULT {
    if lock != 0 {
        lock_module();
    } else {
        unlock_module();
    }
    S_OK
}

// ---- DLL エクスポート -----------------------------------------------------

/// # Safety
/// COM ランタイムから呼ばれる。
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    crate::log::log(&format!(
        "DllGetClassObject rclsid={:?}",
        if rclsid.is_null() {
            GUID::zeroed()
        } else {
            unsafe { *rclsid }
        }
    ));
    if rclsid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    unsafe {
        if *rclsid != CLSID_SD90_ASIO {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let f = Box::into_raw(Box::new(ClassFactory {
            vtbl: &FACTORY_VTBL,
            refs: AtomicU32::new(1),
        }));
        lock_module();
        let hr = cf_query_interface(f, riid, ppv);
        cf_release(f);
        hr
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if MODULE_LOCKS.load(Ordering::Acquire) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

// ---- 自己登録 -------------------------------------------------------------

fn guid_string(g: &GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7]
    )
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn module_path() -> Option<String> {
    unsafe {
        let mut h = Default::default();
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(module_path as *const () as *const u16),
            &mut h,
        )
        .ok()?;
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(Some(h), &mut buf);
        if n == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    }
}

fn set_str(key: HKEY, name: Option<&str>, value: &str) -> bool {
    let v = wide(value);
    let bytes = unsafe {
        std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(&v[..]))
    };
    let n = name.map(wide);
    let pname = n
        .as_ref()
        .map(|x| PCWSTR(x.as_ptr()))
        .unwrap_or(PCWSTR::null());
    unsafe { RegSetValueExW(key, pname, None, REG_SZ, Some(bytes)).is_ok() }
}

fn create_key(root: HKEY, path: &str) -> Option<HKEY> {
    let p = wide(path);
    let mut key = HKEY::default();
    let r = unsafe {
        RegCreateKeyExW(
            root,
            PCWSTR(p.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
    };
    if r.is_ok() { Some(key) } else { None }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    let Some(path) = module_path() else {
        return E_FAIL;
    };
    let clsid = guid_string(&CLSID_SD90_ASIO);

    // HKCR\CLSID\{..}
    let Some(k) = create_key(HKEY_CLASSES_ROOT, &format!("CLSID\\{clsid}")) else {
        return E_FAIL;
    };
    set_str(k, None, DRIVER_NAME);
    unsafe {
        let _ = RegCloseKey(k);
    }

    let Some(k) = create_key(
        HKEY_CLASSES_ROOT,
        &format!("CLSID\\{clsid}\\InprocServer32"),
    ) else {
        return E_FAIL;
    };
    set_str(k, None, &path);
    set_str(k, Some("ThreadingModel"), "Both");
    unsafe {
        let _ = RegCloseKey(k);
    }

    // ASIO ホストはここを見てドライバを列挙する。
    let Some(k) = create_key(
        HKEY_LOCAL_MACHINE,
        &format!("SOFTWARE\\ASIO\\{DRIVER_NAME}"),
    ) else {
        return E_FAIL;
    };
    set_str(k, Some("CLSID"), &clsid);
    set_str(k, Some("Description"), DRIVER_NAME);
    unsafe {
        let _ = RegCloseKey(k);
    }

    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    let clsid = guid_string(&CLSID_SD90_ASIO);
    unsafe {
        let p = wide(&format!("CLSID\\{clsid}"));
        let _ = RegDeleteTreeW(HKEY_CLASSES_ROOT, PCWSTR(p.as_ptr()));
        let p = wide(&format!("SOFTWARE\\ASIO\\{DRIVER_NAME}"));
        let _ = RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(p.as_ptr()));
    }
    S_OK
}
