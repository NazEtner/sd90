//! クラスファクトリと DLL のエクスポート。
//!
//! VST3 のモジュールが外に出すのは `GetPluginFactory` (と任意の
//! `InitDll` / `ExitDll`) の 3 つだけ。ファクトリはプロセスに 1 つの
//! 静的オブジェクトで足りるので、参照カウントは形だけ持つ。

use crate::plugin::{CID_SD90, PLUGIN_NAME, Plugin, VENDOR, VERSION};
use crate::vst3::*;
use std::ffi::c_void;

#[repr(C)]
struct Factory {
    vtbl: *const IPluginFactory3Vtbl,
}

// プロセスに 1 つの静的オブジェクト。中身は不変で、状態を持たない。
unsafe impl Sync for Factory {}

static FACTORY: Factory = Factory {
    vtbl: &FACTORY_VTBL,
};

unsafe extern "system" fn f_query(
    this: *mut c_void,
    iid: *const Tuid,
    out: *mut *mut c_void,
) -> Tresult {
    if out.is_null() || iid.is_null() {
        return K_INVALID_ARGUMENT;
    }
    unsafe { *out = std::ptr::null_mut() };
    let want = unsafe {
        tuid_eq(iid, &IID_FUNKNOWN)
            || tuid_eq(iid, &IID_IPLUGIN_FACTORY)
            || tuid_eq(iid, &IID_IPLUGIN_FACTORY2)
            || tuid_eq(iid, &IID_IPLUGIN_FACTORY3)
    };
    if want {
        unsafe { *out = this };
        return K_RESULT_OK;
    }
    K_NO_INTERFACE
}

unsafe extern "system" fn f_add_ref(_this: *mut c_void) -> u32 {
    1
}
unsafe extern "system" fn f_release(_this: *mut c_void) -> u32 {
    1
}

unsafe extern "system" fn f_get_factory_info(
    _this: *mut c_void,
    info: *mut PFactoryInfo,
) -> Tresult {
    if info.is_null() {
        return K_INVALID_ARGUMENT;
    }
    let info = unsafe { &mut *info };
    set_str8(&mut info.vendor, VENDOR);
    set_str8(&mut info.url, "https://github.com/");
    set_str8(&mut info.email, "");
    // kUnicode。文字列は UTF-16 で返す。
    info.flags = 1 << 4;
    K_RESULT_OK
}

unsafe extern "system" fn f_count_classes(_this: *mut c_void) -> i32 {
    1
}

unsafe extern "system" fn f_get_class_info(
    _this: *mut c_void,
    index: i32,
    info: *mut PClassInfo,
) -> Tresult {
    if info.is_null() || index != 0 {
        return K_INVALID_ARGUMENT;
    }
    let info = unsafe { &mut *info };
    info.cid = CID_SD90;
    info.cardinality = CARDINALITY_MANY_INSTANCES;
    set_str8(&mut info.category, CLASS_CATEGORY_AUDIO_MODULE);
    set_str8(&mut info.name, PLUGIN_NAME);
    K_RESULT_OK
}

unsafe extern "system" fn f_get_class_info2(
    _this: *mut c_void,
    index: i32,
    info: *mut PClassInfo2,
) -> Tresult {
    if info.is_null() || index != 0 {
        return K_INVALID_ARGUMENT;
    }
    let info = unsafe { &mut *info };
    info.cid = CID_SD90;
    info.cardinality = CARDINALITY_MANY_INSTANCES;
    set_str8(&mut info.category, CLASS_CATEGORY_AUDIO_MODULE);
    set_str8(&mut info.name, PLUGIN_NAME);
    info.class_flags = 0;
    set_str8(&mut info.sub_categories, SUBCATEGORY_INSTRUMENT_SYNTH);
    set_str8(&mut info.vendor, VENDOR);
    set_str8(&mut info.version, VERSION);
    set_str8(&mut info.sdk_version, SDK_VERSION);
    K_RESULT_OK
}

unsafe extern "system" fn f_get_class_info_unicode(
    _this: *mut c_void,
    index: i32,
    info: *mut PClassInfoW,
) -> Tresult {
    if info.is_null() || index != 0 {
        return K_INVALID_ARGUMENT;
    }
    let info = unsafe { &mut *info };
    info.cid = CID_SD90;
    info.cardinality = CARDINALITY_MANY_INSTANCES;
    set_str8(&mut info.category, CLASS_CATEGORY_AUDIO_MODULE);
    set_str16(&mut info.name, PLUGIN_NAME);
    info.class_flags = 0;
    set_str8(&mut info.sub_categories, SUBCATEGORY_INSTRUMENT_SYNTH);
    set_str16(&mut info.vendor, VENDOR);
    set_str16(&mut info.version, VERSION);
    set_str16(&mut info.sdk_version, SDK_VERSION);
    K_RESULT_OK
}

unsafe extern "system" fn f_set_host_context(_this: *mut c_void, _ctx: *mut c_void) -> Tresult {
    K_NOT_IMPLEMENTED
}

unsafe extern "system" fn f_create_instance(
    _this: *mut c_void,
    cid: *const u8,
    iid: *const u8,
    obj: *mut *mut c_void,
) -> Tresult {
    if obj.is_null() || cid.is_null() || iid.is_null() {
        return K_INVALID_ARGUMENT;
    }
    unsafe { *obj = std::ptr::null_mut() };
    if !unsafe { tuid_eq(cid as *const Tuid, &CID_SD90) } {
        return K_NO_INTERFACE;
    }
    let p = Plugin::new();
    if p.is_null() {
        return K_OUT_OF_MEMORY;
    }
    unsafe {
        // 生成時の参照が 1。要求されたインタフェースを返して、こちらのぶんを落とす。
        let vtbl = *(p as *const *const IComponentVtbl);
        let hr = ((*vtbl).query_interface)(p as *mut c_void, iid as *const Tuid, obj);
        ((*vtbl).release)(p as *mut c_void);
        hr
    }
}

static FACTORY_VTBL: IPluginFactory3Vtbl = IPluginFactory3Vtbl {
    query_interface: f_query,
    add_ref: f_add_ref,
    release: f_release,
    get_factory_info: f_get_factory_info,
    count_classes: f_count_classes,
    get_class_info: f_get_class_info,
    create_instance: f_create_instance,
    get_class_info2: f_get_class_info2,
    get_class_info_unicode: f_get_class_info_unicode,
    set_host_context: f_set_host_context,
};

// ---- DLL のエクスポート ----------------------------------------------------

/// モジュールの入口。ホストはこれ 1 本からすべてを辿る。
///
/// # Safety
/// ホストが VST3 の規約どおりに呼ぶこと。
#[unsafe(no_mangle)]
pub extern "system" fn GetPluginFactory() -> *mut c_void {
    &FACTORY as *const Factory as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "system" fn InitDll() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "system" fn ExitDll() -> bool {
    true
}
