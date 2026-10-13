//! IF2 (class FF/FF, bulk 32B) の MIDI エンドポイントを叩く。
//!
//! SD-90 は MIDI StartUp = USB のときしか USB に出てこない。その設定では
//! **背面の MIDI IN は音源に届かず、USB 経由でホストに転送される**。つまり
//! 音源に何か言えるのは USB だけ。純正ドライバは現行 Windows のカーネル CI を
//! 通らないので、ここも自前で書く。
//!
//! クラスコードは 0xFF だが、中身は標準の USB-MIDI 1.0。Linux の
//! `sound/usb/quirks-table.h` にある 0582:0016 の項が
//! `QUIRK_DATA_MIDI_FIXED_ENDPOINT(2) { .out_cables = 0x000f, .in_cables = 0x000f }`
//! で、IF2 に 4 ポートぶんの標準 MIDI が生えていることを示している。
//!
//!   cable 0 = PART A, 1 = PART B, 2/3 = 背面の MIDI OUT コネクタ
//!
//! **書けるのは 4 バイトのパケットだけ**。生の MIDI バイト列をそのまま書くと
//! CIN=0 の未定義パケットになり、SysEx の終端 (F7) が来ないままパーサが
//! 待ち状態に入って以後の演奏を全部飲み込む。そうなったら Native On
//! (F0 41 10 00 48 12 00 00 00 00 00 00 F7) を送るか、本体を入れ直す。

use crate::device::Sd90;
use std::ffi::c_void;
use std::sync::Arc;
use windows::Win32::Devices::Usb::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::IO::OVERLAPPED;
use windows::Win32::System::Threading::CreateEventW;
use windows::core::{PCWSTR, Result};

pub const IF_MIDI: u8 = 2;
pub const EP_MIDI_OUT: u8 = 0x02;
pub const EP_MIDI_IN: u8 = 0x82;
/// alt0 = OUT/IN とも bulk。alt1 は IN が interrupt になる。
pub const ALT_MIDI: u8 = 0;

pub struct UsbMidi {
    dev: Arc<Sd90>,
    /// 送出と受信は別パイプなので同時に走る。OVERLAPPED のイベントも別にする。
    ev_out: HANDLE,
    ev_in: HANDLE,
    /// 送出先。0 = PART A, 1 = PART B, 2/3 = 背面の MIDI OUT コネクタ。
    pub cable: u8,
}

fn set_timeout(iface: WINUSB_INTERFACE_HANDLE, pipe: u8, ms: u32) -> Result<()> {
    unsafe {
        WinUsb_SetPipePolicy(
            iface,
            pipe,
            PIPE_TRANSFER_TIMEOUT,
            std::mem::size_of::<u32>() as u32,
            &ms as *const u32 as *const c_void,
        )
    }
}

impl UsbMidi {
    pub fn open(dev: Arc<Sd90>) -> Result<Self> {
        Self::open_cable(dev, 0)
    }

    pub fn open_cable(dev: Arc<Sd90>, cable: u8) -> Result<Self> {
        let iface = dev.iface(IF_MIDI);
        unsafe { WinUsb_SetCurrentAlternateSetting(iface, ALT_MIDI)? };
        // 応答が無いときに固まらないよう、両方向にタイムアウトを入れておく。
        set_timeout(iface, EP_MIDI_OUT, 1000)?;
        set_timeout(iface, EP_MIDI_IN, 300)?;
        // SHORT_PACKET_TERMINATE は付けない。USB-MIDI のパケットは 4 バイト単位で
        // 自己完結しているので ZLP は要らず、付けると鳴らなくなる個体がある。
        let ev_out = unsafe { CreateEventW(None, true, false, PCWSTR::null())? };
        let ev_in = unsafe { CreateEventW(None, true, false, PCWSTR::null())? };
        Ok(Self {
            dev,
            ev_out,
            ev_in,
            cable,
        })
    }

    fn iface(&self) -> WINUSB_INTERFACE_HANDLE {
        self.dev.iface(IF_MIDI)
    }

    /// デバイスハンドルが FILE_FLAG_OVERLAPPED で開いているので、
    /// 同期のつもりでも OVERLAPPED を渡して完了を待つ。
    fn io(&self, write: bool, buf: &mut [u8]) -> Result<u32> {
        let ev = if write { self.ev_out } else { self.ev_in };
        let mut ov = OVERLAPPED {
            hEvent: ev,
            ..Default::default()
        };
        unsafe {
            windows::Win32::System::Threading::ResetEvent(ev)?;
            let r = if write {
                WinUsb_WritePipe(self.iface(), EP_MIDI_OUT, buf, None, Some(&ov))
            } else {
                WinUsb_ReadPipe(self.iface(), EP_MIDI_IN, Some(buf), None, Some(&ov))
            };
            match r {
                Ok(()) => {}
                Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
                Err(e) => return Err(e),
            }
            let mut n = 0u32;
            WinUsb_GetOverlappedResult(self.iface(), &mut ov, &mut n, true)?;
            Ok(n)
        }
    }

    pub fn write(&self, data: &[u8]) -> Result<u32> {
        let mut b = data.to_vec();
        self.io(true, &mut b)
    }

    /// 送出スレッドのように自前のバッファを持っている側はこちらを使う。
    /// WinUSB は書き込みバッファを可変で受けるので、コピーを 1 回省ける。
    pub fn write_in_place(&self, buf: &mut [u8]) -> Result<u32> {
        self.io(true, buf)
    }

    /// 返ってきたバイト列。タイムアウトなら空。
    pub fn read(&self) -> Vec<u8> {
        let mut buf = [0u8; 64];
        match self.io(false, &mut buf) {
            Ok(n) => buf[..n as usize].to_vec(),
            Err(_) => Vec::new(),
        }
    }

    /// 完全な MIDI メッセージを 1 つ以上、1 転送にまとめて送る。
    pub fn send(&self, msgs: &[&[u8]]) -> Result<()> {
        let mut buf = Vec::with_capacity(msgs.len() * 4);
        for m in msgs {
            buf.extend_from_slice(&usb_midi_packets(m, self.cable));
        }
        self.write(&buf).map(|_| ())
    }

    /// 全 ch の All Sound Off / Reset All Controllers。
    pub fn silence(&self) {
        for ch in 0..16u8 {
            let _ = self.send(&[&[0xB0 | ch, 120, 0], &[0xB0 | ch, 121, 0]]);
        }
    }

    /// 溜まっている入力を捨てる。
    pub fn drain(&self) {
        for _ in 0..8 {
            if self.read().is_empty() {
                break;
            }
        }
    }
}

// HANDLE (= *mut c_void) が入っているので自動では付かない。読みと書きで
// パイプもイベントも分けてあるので、2 スレッドから同時に使って問題ない。
unsafe impl Send for UsbMidi {}
unsafe impl Sync for UsbMidi {}

impl Drop for UsbMidi {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.ev_out);
            let _ = CloseHandle(self.ev_in);
        }
    }
}

/// 完全な MIDI メッセージ 1 個を USB-MIDI 1.0 の 4 バイトパケットに分解し、
/// 1 個ずつ渡す。チャンネルメッセージなら先頭バイトの上位 4bit がそのまま CIN。
///
/// SysEx は長さが read だけでは決まらないので、Vec を作らずに済むこちらを
/// 基本形にしてある。RT スレッド (VST3 の `process`) から呼ぶため確保をしない。
pub fn for_each_packet(m: &[u8], cable: u8, mut f: impl FnMut([u8; 4])) {
    if m.is_empty() {
        return;
    }
    let c = cable << 4;
    if m[0] == 0xF0 {
        // SysEx は 3 バイトずつ。最後の 1〜3 バイトは CIN 5/6/7 で閉じる。
        let mut i = 0;
        while i < m.len() {
            let rest = m.len() - i;
            let (cin, n) = if rest > 3 {
                (0x4, 3)
            } else {
                (0x4 + rest as u8, rest)
            };
            let mut p = [c | cin, 0, 0, 0];
            p[1..1 + n].copy_from_slice(&m[i..i + n]);
            f(p);
            i += n;
        }
    } else if m[0] >= 0xF8 {
        // システムリアルタイムは 1 バイト。
        f([c | 0xF, m[0], 0, 0]);
    } else if m[0] == 0xF1 || m[0] == 0xF3 {
        f([c | 0x2, m[0], *m.get(1).unwrap_or(&0), 0]);
    } else if m[0] == 0xF2 {
        f([
            c | 0x3,
            m[0],
            *m.get(1).unwrap_or(&0),
            *m.get(2).unwrap_or(&0),
        ]);
    } else {
        f([
            c | (m[0] >> 4),
            m[0],
            *m.get(1).unwrap_or(&0),
            *m.get(2).unwrap_or(&0),
        ]);
    }
}

/// `for_each_packet` を連結したもの。プローブや診断のように確保を気にしない側用。
pub fn usb_midi_packets(m: &[u8], cable: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(m.len() + 4);
    for_each_packet(m, cable, |p| out.extend_from_slice(&p));
    out
}
