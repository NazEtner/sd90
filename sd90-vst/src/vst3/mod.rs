//! VST3 の ABI を Rust 側で宣言し直したもの。
//!
//! 出典は `vst3_pluginterfaces` 3.8.1 (MIT。表示は末尾)。
//! 構造体のサイズとオフセットはヘッダを clang に食わせて実測し、下の
//! `const _: () = assert!(...)` で固定してある。ずれたらビルドが止まる。
//!
//! # 踏みやすいところ
//!
//! * **`TUID` のバイト順**。SDK の `INLINE_UID` は `COM_COMPATIBLE`
//!   (= Windows) とそれ以外で並びが違う。Windows 側は先頭 8 バイトが
//!   GUID 構造体の並び (`Data1` が LE、`Data2`/`Data3` が LE)、残り 8 バイトが
//!   ビッグエンディアンという混成。間違えると `queryInterface` が一度も
//!   成立せず、ホストからは「読み込めるが何も起きない」に見える。
//! * **`Event` は 48 バイト**。共用体が `DataEvent` のポインタで 8 バイト境界に
//!   なるため、`type` (offset 18) の後ろに 6 バイトの詰め物が入って共用体は 24 から。
//! * x64 には呼び出し規約が 1 つしかないので `extern "system"` で一致する。
//!   多重継承ぶんの vtable ポインタは構造体の先頭に並べ、各メソッドは
//!   自分のスロットのオフセットを引いてオブジェクト先頭に戻る。
//!
//! # 出典のライセンス
//!
//! ```text
//! Copyright (c) 2025, Steinberg Media Technologies GmbH
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.
//! ```

#![allow(dead_code)]

use std::ffi::c_void;

pub type Tuid = [u8; 16];
pub type Tresult = i32;
pub type String128 = [u16; 128];
pub type SpeakerArrangement = u64;

pub const K_RESULT_OK: Tresult = 0;
pub const K_RESULT_TRUE: Tresult = 0;
pub const K_RESULT_FALSE: Tresult = 1;
pub const K_NO_INTERFACE: Tresult = 0x8000_4002u32 as i32;
pub const K_INVALID_ARGUMENT: Tresult = 0x8007_0057u32 as i32;
pub const K_NOT_IMPLEMENTED: Tresult = 0x8000_4001u32 as i32;
pub const K_INTERNAL_ERROR: Tresult = 0x8000_4005u32 as i32;
pub const K_NOT_INITIALIZED: Tresult = 0x8000_FFFFu32 as i32;
pub const K_OUT_OF_MEMORY: Tresult = 0x8007_000Eu32 as i32;

/// `INLINE_UID` の `COM_COMPATIBLE` 版。SDK ヘッダの 4 ワード表記をそのまま渡す。
pub const fn uid(l1: u32, l2: u32, l3: u32, l4: u32) -> Tuid {
    [
        (l1 & 0xFF) as u8,
        ((l1 >> 8) & 0xFF) as u8,
        ((l1 >> 16) & 0xFF) as u8,
        ((l1 >> 24) & 0xFF) as u8,
        ((l2 >> 16) & 0xFF) as u8,
        ((l2 >> 24) & 0xFF) as u8,
        (l2 & 0xFF) as u8,
        ((l2 >> 8) & 0xFF) as u8,
        ((l3 >> 24) & 0xFF) as u8,
        ((l3 >> 16) & 0xFF) as u8,
        ((l3 >> 8) & 0xFF) as u8,
        (l3 & 0xFF) as u8,
        ((l4 >> 24) & 0xFF) as u8,
        ((l4 >> 16) & 0xFF) as u8,
        ((l4 >> 8) & 0xFF) as u8,
        (l4 & 0xFF) as u8,
    ]
}

pub const IID_FUNKNOWN: Tuid = uid(0x0000_0000, 0x0000_0000, 0xC000_0000, 0x0000_0046);
pub const IID_IPLUGIN_BASE: Tuid = uid(0x22888DDB, 0x156E45AE, 0x8358B348, 0x08190625);
pub const IID_IPLUGIN_FACTORY: Tuid = uid(0x7A4D811C, 0x52114A1F, 0xAED9D2EE, 0x0B43BF9F);
pub const IID_IPLUGIN_FACTORY2: Tuid = uid(0x0007B650, 0xF24B4C0B, 0xA464EDB9, 0xF00B2ABB);
pub const IID_IPLUGIN_FACTORY3: Tuid = uid(0x4555A2AB, 0xC1234E57, 0x9B122910, 0x36878931);
pub const IID_ICOMPONENT: Tuid = uid(0xE831FF31, 0xF2D54301, 0x928EBBEE, 0x25697802);
pub const IID_IAUDIO_PROCESSOR: Tuid = uid(0x42043F99, 0xB7DA453C, 0xA569E79D, 0x9AAEC33D);
pub const IID_IEDIT_CONTROLLER: Tuid = uid(0xDCD7BBE3, 0x7742448D, 0xA874AACC, 0x979C759E);
pub const IID_IMIDI_MAPPING: Tuid = uid(0xDF0FF9F7, 0x49B74669, 0xB63AB732, 0x7ADBF5E5);
pub const IID_IEVENT_LIST: Tuid = uid(0x3A2C4214, 0x346349FE, 0xB2C4F397, 0xB9695A44);
pub const IID_IPARAMETER_CHANGES: Tuid = uid(0xA4779663, 0x0BB64A56, 0xB44384A8, 0x466FEB9D);
pub const IID_ICOMPONENT_HANDLER: Tuid = uid(0x93A0BEA3, 0x0BD045DB, 0x8E890B0C, 0xC1E46AC6);
pub const IID_IPLUG_VIEW: Tuid = uid(0x5BC32507, 0xD06049EA, 0xA6151B52, 0x2B755B29);
pub const IID_IPLUG_FRAME: Tuid = uid(0x367FAF01, 0xAFA94693, 0x8D4DA2A0, 0xED0882A3);
pub const IID_IPLUG_VIEW_CONTENT_SCALE_SUPPORT: Tuid =
    uid(0x65ED9690, 0x8AC44525, 0x8AADEF7A, 0x72EA703F);
pub const IID_IBSTREAM: Tuid = uid(0xC3BF6EA2, 0x30994752, 0x9B6BF990, 0x1EE33E9B);

/// `kPlatformTypeHWND`。
pub const PLATFORM_TYPE_HWND: &std::ffi::CStr = c"HWND";
/// `ViewType::kEditor`。
pub const VIEW_TYPE_EDITOR: &std::ffi::CStr = c"editor";

pub const CLASS_CATEGORY_AUDIO_MODULE: &str = "Audio Module Class";
pub const SUBCATEGORY_INSTRUMENT_SYNTH: &str = "Instrument|Synth";
pub const SDK_VERSION: &str = "VST 3.8.1";
pub const CARDINALITY_MANY_INSTANCES: i32 = 0x7FFF_FFFF;

// MediaTypes
pub const K_AUDIO: i32 = 0;
pub const K_EVENT: i32 = 1;
// BusDirections
pub const K_INPUT: i32 = 0;
pub const K_OUTPUT: i32 = 1;
// BusTypes
pub const K_MAIN: i32 = 0;
pub const K_AUX: i32 = 1;
// BusFlags
pub const K_DEFAULT_ACTIVE: u32 = 1;
// SymbolicSampleSizes
pub const K_SAMPLE32: i32 = 0;
pub const K_SAMPLE64: i32 = 1;
// ProcessModes
pub const K_REALTIME: i32 = 0;
pub const K_PREFETCH: i32 = 1;
pub const K_OFFLINE: i32 = 2;
// EventTypes
pub const K_NOTE_ON_EVENT: u16 = 0;
pub const K_NOTE_OFF_EVENT: u16 = 1;
pub const K_DATA_EVENT: u16 = 2;
pub const K_POLY_PRESSURE_EVENT: u16 = 3;
/// `DataEvent::type` の MIDI SysEx。
pub const K_MIDI_SYS_EX: u32 = 0;
// ParameterFlags
pub const K_CAN_AUTOMATE: i32 = 1;
pub const K_IS_READ_ONLY: i32 = 1 << 1;
pub const K_IS_HIDDEN: i32 = 1 << 4;
// RestartFlags
pub const K_LATENCY_CHANGED: i32 = 1 << 3;
// ControllerNumbers
pub const K_AFTER_TOUCH: i32 = 128;
pub const K_PITCH_BEND: i32 = 129;
pub const K_COUNT_CTRL_NUMBER: i32 = 130;

pub const K_SPEAKER_STEREO: SpeakerArrangement = 0x3;

// ---- 構造体 ---------------------------------------------------------------

#[repr(C)]
pub struct PFactoryInfo {
    pub vendor: [u8; 64],
    pub url: [u8; 256],
    pub email: [u8; 128],
    pub flags: i32,
}

#[repr(C)]
pub struct PClassInfo {
    pub cid: Tuid,
    pub cardinality: i32,
    pub category: [u8; 32],
    pub name: [u8; 64],
}

#[repr(C)]
pub struct PClassInfo2 {
    pub cid: Tuid,
    pub cardinality: i32,
    pub category: [u8; 32],
    pub name: [u8; 64],
    pub class_flags: u32,
    pub sub_categories: [u8; 128],
    pub vendor: [u8; 64],
    pub version: [u8; 64],
    pub sdk_version: [u8; 64],
}

#[repr(C)]
pub struct PClassInfoW {
    pub cid: Tuid,
    pub cardinality: i32,
    pub category: [u8; 32],
    pub name: [u16; 64],
    pub class_flags: u32,
    pub sub_categories: [u8; 128],
    pub vendor: [u16; 64],
    pub version: [u16; 64],
    pub sdk_version: [u16; 64],
}

#[repr(C)]
pub struct BusInfo {
    pub media_type: i32,
    pub direction: i32,
    pub channel_count: i32,
    pub name: String128,
    pub bus_type: i32,
    pub flags: u32,
}

#[repr(C)]
pub struct RoutingInfo {
    pub media_type: i32,
    pub bus_index: i32,
    pub channel: i32,
}

#[repr(C)]
pub struct ProcessSetup {
    pub process_mode: i32,
    pub symbolic_sample_size: i32,
    pub max_samples_per_block: i32,
    pub sample_rate: f64,
}

#[repr(C)]
pub struct AudioBusBuffers {
    pub num_channels: i32,
    pub silence_flags: u64,
    /// `Sample32**` / `Sample64**` の共用体。32bit しか受けないので前者で読む。
    pub channel_buffers: *mut *mut f32,
}

#[repr(C)]
pub struct ProcessData {
    pub process_mode: i32,
    pub symbolic_sample_size: i32,
    pub num_samples: i32,
    pub num_inputs: i32,
    pub num_outputs: i32,
    pub inputs: *mut AudioBusBuffers,
    pub outputs: *mut AudioBusBuffers,
    pub input_parameter_changes: *mut IParameterChanges,
    pub output_parameter_changes: *mut IParameterChanges,
    pub input_events: *mut IEventList,
    pub output_events: *mut IEventList,
    pub process_context: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NoteOnEvent {
    pub channel: i16,
    pub pitch: i16,
    pub tuning: f32,
    pub velocity: f32,
    pub length: i32,
    pub note_id: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NoteOffEvent {
    pub channel: i16,
    pub pitch: i16,
    pub velocity: f32,
    pub note_id: i32,
    pub tuning: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DataEvent {
    pub size: u32,
    pub data_type: u32,
    pub bytes: *const u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PolyPressureEvent {
    pub channel: i16,
    pub pitch: i16,
    pub pressure: f32,
    pub note_id: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union EventUnion {
    pub note_on: NoteOnEvent,
    pub note_off: NoteOffEvent,
    pub data: DataEvent,
    pub poly_pressure: PolyPressureEvent,
    /// 触らないイベント型のぶんの場所取り。共用体の最大は 24 バイト。
    pub raw: [u8; 24],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Event {
    pub bus_index: i32,
    pub sample_offset: i32,
    pub ppq_position: f64,
    pub flags: u16,
    pub event_type: u16,
    pub ev: EventUnion,
}

#[repr(C)]
pub struct ParameterInfo {
    pub id: u32,
    pub title: String128,
    pub short_title: String128,
    pub units: String128,
    pub step_count: i32,
    pub default_normalized_value: f64,
    pub unit_id: i32,
    pub flags: i32,
}

/// `ViewRect`。Windows では物理ピクセル。
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ViewRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ViewRect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

// ヘッダを clang に食わせて実測した値。ここがずれたらホストとの解釈が食い違う。
const _: () = {
    assert!(std::mem::size_of::<ViewRect>() == 16);
    use std::mem::{align_of, offset_of, size_of};
    assert!(size_of::<PFactoryInfo>() == 452);
    assert!(size_of::<PClassInfo>() == 116);
    assert!(size_of::<PClassInfo2>() == 440);
    assert!(size_of::<PClassInfoW>() == 696);
    assert!(offset_of!(PClassInfoW, name) == 52);
    assert!(offset_of!(PClassInfoW, sub_categories) == 184);
    assert!(offset_of!(PClassInfoW, sdk_version) == 568);
    assert!(size_of::<BusInfo>() == 276);
    assert!(offset_of!(BusInfo, name) == 12);
    assert!(offset_of!(BusInfo, flags) == 272);
    assert!(size_of::<RoutingInfo>() == 12);
    assert!(size_of::<ProcessSetup>() == 24);
    assert!(offset_of!(ProcessSetup, sample_rate) == 16);
    assert!(size_of::<AudioBusBuffers>() == 24);
    assert!(offset_of!(AudioBusBuffers, silence_flags) == 8);
    assert!(offset_of!(AudioBusBuffers, channel_buffers) == 16);
    assert!(size_of::<ProcessData>() == 80);
    assert!(offset_of!(ProcessData, inputs) == 24);
    assert!(offset_of!(ProcessData, input_events) == 56);
    assert!(offset_of!(ProcessData, process_context) == 72);
    assert!(size_of::<Event>() == 48 && align_of::<Event>() == 8);
    assert!(offset_of!(Event, ppq_position) == 8);
    assert!(offset_of!(Event, event_type) == 18);
    assert!(offset_of!(Event, ev) == 24);
    assert!(size_of::<NoteOnEvent>() == 20);
    assert!(size_of::<NoteOffEvent>() == 16);
    assert!(size_of::<DataEvent>() == 16);
    assert!(size_of::<ParameterInfo>() == 792);
    assert!(offset_of!(ParameterInfo, short_title) == 260);
    assert!(offset_of!(ParameterInfo, step_count) == 772);
    assert!(offset_of!(ParameterInfo, flags) == 788);
};

// ---- こちらが実装する側の vtable -------------------------------------------
//
// FUnknown の 3 メソッドがどれの先頭にも並ぶ。`this` は常にその vtable
// ポインタが置かれているアドレスであって、オブジェクトの先頭とは限らない。

pub type QueryInterfaceFn =
    unsafe extern "system" fn(*mut c_void, *const Tuid, *mut *mut c_void) -> Tresult;
pub type RefFn = unsafe extern "system" fn(*mut c_void) -> u32;

#[repr(C)]
pub struct IComponentVtbl {
    pub query_interface: QueryInterfaceFn,
    pub add_ref: RefFn,
    pub release: RefFn,
    // IPluginBase
    pub initialize: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Tresult,
    pub terminate: unsafe extern "system" fn(*mut c_void) -> Tresult,
    // IComponent
    pub get_controller_class_id: unsafe extern "system" fn(*mut c_void, *mut Tuid) -> Tresult,
    pub set_io_mode: unsafe extern "system" fn(*mut c_void, i32) -> Tresult,
    pub get_bus_count: unsafe extern "system" fn(*mut c_void, i32, i32) -> i32,
    pub get_bus_info:
        unsafe extern "system" fn(*mut c_void, i32, i32, i32, *mut BusInfo) -> Tresult,
    pub get_routing_info:
        unsafe extern "system" fn(*mut c_void, *mut RoutingInfo, *mut RoutingInfo) -> Tresult,
    pub activate_bus: unsafe extern "system" fn(*mut c_void, i32, i32, i32, u8) -> Tresult,
    pub set_active: unsafe extern "system" fn(*mut c_void, u8) -> Tresult,
    pub set_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Tresult,
    pub get_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Tresult,
}

#[repr(C)]
pub struct IAudioProcessorVtbl {
    pub query_interface: QueryInterfaceFn,
    pub add_ref: RefFn,
    pub release: RefFn,
    pub set_bus_arrangements: unsafe extern "system" fn(
        *mut c_void,
        *mut SpeakerArrangement,
        i32,
        *mut SpeakerArrangement,
        i32,
    ) -> Tresult,
    pub get_bus_arrangement:
        unsafe extern "system" fn(*mut c_void, i32, i32, *mut SpeakerArrangement) -> Tresult,
    pub can_process_sample_size: unsafe extern "system" fn(*mut c_void, i32) -> Tresult,
    pub get_latency_samples: unsafe extern "system" fn(*mut c_void) -> u32,
    pub setup_processing: unsafe extern "system" fn(*mut c_void, *mut ProcessSetup) -> Tresult,
    pub set_processing: unsafe extern "system" fn(*mut c_void, u8) -> Tresult,
    pub process: unsafe extern "system" fn(*mut c_void, *mut ProcessData) -> Tresult,
    pub get_tail_samples: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
pub struct IEditControllerVtbl {
    pub query_interface: QueryInterfaceFn,
    pub add_ref: RefFn,
    pub release: RefFn,
    // IPluginBase
    pub initialize: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Tresult,
    pub terminate: unsafe extern "system" fn(*mut c_void) -> Tresult,
    // IEditController
    pub set_component_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Tresult,
    pub set_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Tresult,
    pub get_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Tresult,
    pub get_parameter_count: unsafe extern "system" fn(*mut c_void) -> i32,
    pub get_parameter_info:
        unsafe extern "system" fn(*mut c_void, i32, *mut ParameterInfo) -> Tresult,
    pub get_param_string_by_value:
        unsafe extern "system" fn(*mut c_void, u32, f64, *mut u16) -> Tresult,
    pub get_param_value_by_string:
        unsafe extern "system" fn(*mut c_void, u32, *const u16, *mut f64) -> Tresult,
    pub normalized_param_to_plain: unsafe extern "system" fn(*mut c_void, u32, f64) -> f64,
    pub plain_param_to_normalized: unsafe extern "system" fn(*mut c_void, u32, f64) -> f64,
    pub get_param_normalized: unsafe extern "system" fn(*mut c_void, u32) -> f64,
    pub set_param_normalized: unsafe extern "system" fn(*mut c_void, u32, f64) -> Tresult,
    pub set_component_handler: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Tresult,
    pub create_view: unsafe extern "system" fn(*mut c_void, *const u8) -> *mut c_void,
}

#[repr(C)]
pub struct IMidiMappingVtbl {
    pub query_interface: QueryInterfaceFn,
    pub add_ref: RefFn,
    pub release: RefFn,
    pub get_midi_controller_assignment:
        unsafe extern "system" fn(*mut c_void, i32, i16, i32, *mut u32) -> Tresult,
}

#[repr(C)]
pub struct IPlugViewVtbl {
    pub query_interface: QueryInterfaceFn,
    pub add_ref: RefFn,
    pub release: RefFn,
    pub is_platform_type_supported: unsafe extern "system" fn(*mut c_void, *const u8) -> Tresult,
    pub attached: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u8) -> Tresult,
    pub removed: unsafe extern "system" fn(*mut c_void) -> Tresult,
    pub on_wheel: unsafe extern "system" fn(*mut c_void, f32) -> Tresult,
    pub on_key_down: unsafe extern "system" fn(*mut c_void, u16, i16, i16) -> Tresult,
    pub on_key_up: unsafe extern "system" fn(*mut c_void, u16, i16, i16) -> Tresult,
    pub get_size: unsafe extern "system" fn(*mut c_void, *mut ViewRect) -> Tresult,
    pub on_size: unsafe extern "system" fn(*mut c_void, *mut ViewRect) -> Tresult,
    pub on_focus: unsafe extern "system" fn(*mut c_void, u8) -> Tresult,
    pub set_frame: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Tresult,
    pub can_resize: unsafe extern "system" fn(*mut c_void) -> Tresult,
    pub check_size_constraint: unsafe extern "system" fn(*mut c_void, *mut ViewRect) -> Tresult,
}

#[repr(C)]
pub struct IPlugViewContentScaleSupportVtbl {
    pub query_interface: QueryInterfaceFn,
    pub add_ref: RefFn,
    pub release: RefFn,
    pub set_content_scale_factor: unsafe extern "system" fn(*mut c_void, f32) -> Tresult,
}

#[repr(C)]
pub struct IPluginFactory3Vtbl {
    pub query_interface: QueryInterfaceFn,
    pub add_ref: RefFn,
    pub release: RefFn,
    pub get_factory_info: unsafe extern "system" fn(*mut c_void, *mut PFactoryInfo) -> Tresult,
    pub count_classes: unsafe extern "system" fn(*mut c_void) -> i32,
    pub get_class_info: unsafe extern "system" fn(*mut c_void, i32, *mut PClassInfo) -> Tresult,
    pub create_instance:
        unsafe extern "system" fn(*mut c_void, *const u8, *const u8, *mut *mut c_void) -> Tresult,
    // IPluginFactory2
    pub get_class_info2: unsafe extern "system" fn(*mut c_void, i32, *mut PClassInfo2) -> Tresult,
    // IPluginFactory3
    pub get_class_info_unicode:
        unsafe extern "system" fn(*mut c_void, i32, *mut PClassInfoW) -> Tresult,
    pub set_host_context: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Tresult,
}

// ---- ホスト側を呼ぶための宣言 ----------------------------------------------

#[repr(C)]
pub struct IEventListVtbl {
    pub query_interface: QueryInterfaceFn,
    pub add_ref: RefFn,
    pub release: RefFn,
    pub get_event_count: unsafe extern "system" fn(*mut c_void) -> i32,
    pub get_event: unsafe extern "system" fn(*mut c_void, i32, *mut Event) -> Tresult,
    pub add_event: unsafe extern "system" fn(*mut c_void, *mut Event) -> Tresult,
}

#[repr(C)]
pub struct IEventList {
    pub vtbl: *const IEventListVtbl,
}

impl IEventList {
    /// # Safety
    /// ホストから渡された生ポインタが有効な間だけ使えること。
    pub unsafe fn count(this: *mut IEventList) -> i32 {
        unsafe { ((*(*this).vtbl).get_event_count)(this as *mut c_void) }
    }

    /// # Safety
    /// 同上。
    pub unsafe fn get(this: *mut IEventList, index: i32, e: &mut Event) -> Tresult {
        unsafe { ((*(*this).vtbl).get_event)(this as *mut c_void, index, e) }
    }
}

#[repr(C)]
pub struct IParamValueQueueVtbl {
    pub query_interface: QueryInterfaceFn,
    pub add_ref: RefFn,
    pub release: RefFn,
    pub get_parameter_id: unsafe extern "system" fn(*mut c_void) -> u32,
    pub get_point_count: unsafe extern "system" fn(*mut c_void) -> i32,
    pub get_point: unsafe extern "system" fn(*mut c_void, i32, *mut i32, *mut f64) -> Tresult,
    pub add_point: unsafe extern "system" fn(*mut c_void, i32, f64, *mut i32) -> Tresult,
}

#[repr(C)]
pub struct IParamValueQueue {
    pub vtbl: *const IParamValueQueueVtbl,
}

#[repr(C)]
pub struct IParameterChangesVtbl {
    pub query_interface: QueryInterfaceFn,
    pub add_ref: RefFn,
    pub release: RefFn,
    pub get_parameter_count: unsafe extern "system" fn(*mut c_void) -> i32,
    pub get_parameter_data: unsafe extern "system" fn(*mut c_void, i32) -> *mut IParamValueQueue,
    pub add_parameter_data:
        unsafe extern "system" fn(*mut c_void, *const u32, *mut i32) -> *mut IParamValueQueue,
}

#[repr(C)]
pub struct IParameterChanges {
    pub vtbl: *const IParameterChangesVtbl,
}

#[repr(C)]
pub struct IComponentHandlerVtbl {
    pub query_interface: QueryInterfaceFn,
    pub add_ref: RefFn,
    pub release: RefFn,
    pub begin_edit: unsafe extern "system" fn(*mut c_void, u32) -> Tresult,
    pub perform_edit: unsafe extern "system" fn(*mut c_void, u32, f64) -> Tresult,
    pub end_edit: unsafe extern "system" fn(*mut c_void, u32) -> Tresult,
    pub restart_component: unsafe extern "system" fn(*mut c_void, i32) -> Tresult,
}

#[repr(C)]
pub struct IComponentHandler {
    pub vtbl: *const IComponentHandlerVtbl,
}

#[repr(C)]
pub struct IPlugFrameVtbl {
    pub query_interface: QueryInterfaceFn,
    pub add_ref: RefFn,
    pub release: RefFn,
    pub resize_view: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut ViewRect) -> Tresult,
}

#[repr(C)]
pub struct IPlugFrame {
    pub vtbl: *const IPlugFrameVtbl,
}

#[repr(C)]
pub struct IBStreamVtbl {
    pub query_interface: QueryInterfaceFn,
    pub add_ref: RefFn,
    pub release: RefFn,
    pub read: unsafe extern "system" fn(*mut c_void, *mut c_void, i32, *mut i32) -> Tresult,
    pub write: unsafe extern "system" fn(*mut c_void, *const c_void, i32, *mut i32) -> Tresult,
    pub seek: unsafe extern "system" fn(*mut c_void, i64, i32, *mut i64) -> Tresult,
    pub tell: unsafe extern "system" fn(*mut c_void, *mut i64) -> Tresult,
}

#[repr(C)]
pub struct IBStream {
    pub vtbl: *const IBStreamVtbl,
}

impl IBStream {
    /// 全部書く。
    ///
    /// # Safety
    /// `this` はホストから渡された有効な `IBStream`。
    pub unsafe fn write_all(this: *mut IBStream, data: &[u8]) -> bool {
        let mut done = 0usize;
        while done < data.len() {
            let mut n = 0i32;
            let rest = &data[done..];
            let r = unsafe {
                ((*(*this).vtbl).write)(
                    this as *mut c_void,
                    rest.as_ptr() as *const c_void,
                    rest.len().min(i32::MAX as usize) as i32,
                    &mut n,
                )
            };
            if r != K_RESULT_OK || n <= 0 {
                return false;
            }
            done += n as usize;
        }
        true
    }

    /// 終わりまで (高々 `limit` バイト) 読む。
    ///
    /// # Safety
    /// 同上。
    pub unsafe fn read_to_end(this: *mut IBStream, limit: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        while out.len() < limit {
            let mut n = 0i32;
            let r = unsafe {
                ((*(*this).vtbl).read)(
                    this as *mut c_void,
                    buf.as_mut_ptr() as *mut c_void,
                    buf.len() as i32,
                    &mut n,
                )
            };
            if r != K_RESULT_OK || n <= 0 {
                break;
            }
            out.extend_from_slice(&buf[..n as usize]);
        }
        out
    }
}

// ---- 小道具 ---------------------------------------------------------------

pub fn set_str8(dst: &mut [u8], s: &str) {
    dst.fill(0);
    let n = s.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&s.as_bytes()[..n]);
}

pub fn set_str16(dst: &mut [u16], s: &str) {
    dst.fill(0);
    // 終端の 0 を必ず残す。
    for (i, c) in s
        .encode_utf16()
        .take(dst.len().saturating_sub(1))
        .enumerate()
    {
        dst[i] = c;
    }
}

/// # Safety
/// `a` は null か、`Tuid` として読める 16 バイトを指していること。
pub unsafe fn tuid_eq(a: *const Tuid, b: &Tuid) -> bool {
    !a.is_null() && unsafe { *a } == *b
}
