//! SD-90 を WinUSB 経由で開く。
//!
//! Vendor モードの SD-90 は bDeviceClass=0xFF の非コンポジットデバイスなので
//! usbccgp は介在せず、WinUSB がデバイス全体に 1 つだけ割り当たる。
//! WinUsb_Initialize が返すのは IF0 のハンドルで、IF1/IF2 は
//! WinUsb_GetAssociatedInterface(0/1) で辿る。

use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Devices::Usb::*;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::core::{PCWSTR, Result};

pub const VID: u16 = 0x0582;
pub const PID_VENDOR: u16 = 0x0016;
pub const PID_GENERIC: u16 = 0x0017;

// 実機の記述子から確定しているレイアウト (Vendor モード)。
//   IF0 alt1 : EP 0x01 OUT iso  再生
//   IF1 alt1 : EP 0x81 IN  iso  録音
//   IF2 alt0 : EP 0x02/0x82 bulk  MIDI
pub const IF_PLAYBACK: u8 = 0;
pub const IF_CAPTURE: u8 = 1;
pub const ALT_STREAMING: u8 = 1;
pub const EP_CAPTURE_IN: u8 = 0x81;
pub const EP_PLAYBACK_OUT: u8 = 0x01;

/// 24bit / 48kHz / 2ch 固定。1 フレーム(1ms)あたり 48 サンプル。
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
pub const BYTES_PER_SAMPLE: usize = 3;
/// 1 オーディオフレーム = 24bit x 2ch。
pub const FRAME_BYTES: usize = CHANNELS * BYTES_PER_SAMPLE;
/// wMaxPacketSize は 320 だが、実際に使われるのは 48*2*3 = 288。
pub const EXPECTED_PACKET_BYTES: usize = 288;

fn wide_to_string(p: *const u16) -> String {
    let mut n = 0usize;
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

/// VID/PID が一致するデバイスパスを返す。
///
/// 注意: ここで引っかかるのは「デバイスインタフェースが有効なもの」だけ。
/// 関数ドライバが起動に失敗している (Code 10 等) デバイスは列挙されない。
pub fn find_device_paths(vid: u16, pid: u16) -> Result<Vec<String>> {
    let want = format!("VID_{vid:04X}&PID_{pid:04X}");
    Ok(enumerate_usb_device_paths()?
        .into_iter()
        .filter(|p| p.to_uppercase().contains(&want))
        .collect())
}

/// GUID_DEVINTERFACE_USB_DEVICE を全部列挙する。列挙処理そのものの健全性確認にも使う。
pub fn enumerate_usb_device_paths() -> Result<Vec<String>> {
    let mut out = Vec::new();
    unsafe {
        let set = SetupDiGetClassDevsW(
            Some(&GUID_DEVINTERFACE_USB_DEVICE),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )?;
        let mut idx = 0u32;
        loop {
            let mut ifdata = SP_DEVICE_INTERFACE_DATA {
                cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInterfaces(
                set,
                None,
                &GUID_DEVINTERFACE_USB_DEVICE,
                idx,
                &mut ifdata,
            )
            .is_err()
            {
                break;
            }
            idx += 1;

            // 可変長構造体なので、まず必要サイズを聞く。
            let mut needed = 0u32;
            let _ =
                SetupDiGetDeviceInterfaceDetailW(set, &ifdata, None, 0, Some(&mut needed), None);
            if needed == 0 {
                continue;
            }
            let mut buf = vec![0u8; needed as usize];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &ifdata, Some(detail), needed, None, None)
                .is_err()
            {
                continue;
            }
            out.push(wide_to_string((*detail).DevicePath.as_ptr()));
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    Ok(out)
}

pub struct Sd90 {
    file: HANDLE,
    pub if0: WINUSB_INTERFACE_HANDLE,
    pub if1: WINUSB_INTERFACE_HANDLE,
    pub if2: WINUSB_INTERFACE_HANDLE,
    pub path: String,
}

// WinUSB のインタフェースハンドルは、別々のパイプを別々のスレッドから叩く
// ぶんには安全 (同じパイプに同時に I/O を出さないこと)。ここでは録音スレッドが
// IF1 の iso IN、送出スレッドが IF2 の bulk OUT を持つので条件を満たす。
unsafe impl Send for Sd90 {}
unsafe impl Sync for Sd90 {}

impl Sd90 {
    pub fn open(pid: u16) -> Result<Self> {
        let paths = find_device_paths(VID, pid)?;
        let path = paths.into_iter().next().ok_or_else(|| {
            windows::core::Error::new(
                ERROR_FILE_NOT_FOUND.to_hresult(),
                "SD-90 が見つからない (電源とモードスイッチ、Zadig の適用を確認)",
            )
        })?;
        Self::open_path(&path)
    }

    pub fn open_path(path: &str) -> Result<Self> {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        // iso 転送は必ず overlapped。
        let file = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )?
        };
        let mut if0 = WINUSB_INTERFACE_HANDLE::default();
        unsafe { WinUsb_Initialize(file, &mut if0) }.inspect_err(|_| unsafe {
            let _ = CloseHandle(file);
        })?;

        // 非コンポジットなので、associated index は IF0 の「次」から 0 始まり。
        let mut if1 = WINUSB_INTERFACE_HANDLE::default();
        let mut if2 = WINUSB_INTERFACE_HANDLE::default();
        unsafe {
            WinUsb_GetAssociatedInterface(if0, 0, &mut if1)?;
            WinUsb_GetAssociatedInterface(if0, 1, &mut if2)?;
        }
        Ok(Self {
            file,
            if0,
            if1,
            if2,
            path: path.to_string(),
        })
    }

    /// CancelIoEx など、デバイスハンドル側に対する操作用。
    pub fn file(&self) -> HANDLE {
        self.file
    }

    pub fn iface(&self, n: u8) -> WINUSB_INTERFACE_HANDLE {
        match n {
            0 => self.if0,
            1 => self.if1,
            _ => self.if2,
        }
    }
}

impl Drop for Sd90 {
    fn drop(&mut self) {
        unsafe {
            let _ = WinUsb_Free(self.if2);
            let _ = WinUsb_Free(self.if1);
            let _ = WinUsb_Free(self.if0);
            let _ = CloseHandle(self.file);
        }
    }
}
