//! 記述子の取得と表示。
//!
//! SD-90 の Vendor モードはクラスコードこそ 0xFF だが、中身は UAC1 の
//! AS_GENERAL / FORMAT_TYPE_I がそのまま入っている。標準として解釈する。

use crate::device::Sd90;
use windows::Win32::Devices::Usb::*;
use windows::core::Result;

const DT_DEVICE: u8 = 0x01;
const DT_CONFIG: u8 = 0x02;
const DT_INTERFACE: u8 = 0x04;
const DT_ENDPOINT: u8 = 0x05;
const DT_CS_INTERFACE: u8 = 0x24;
const DT_CS_ENDPOINT: u8 = 0x25;

pub fn get_descriptor(dev: &Sd90, dtype: u8, index: u8, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let mut got = 0u32;
    unsafe {
        WinUsb_GetDescriptor(dev.if0, dtype, index, 0, Some(&mut buf), &mut got)?;
    }
    buf.truncate(got as usize);
    Ok(buf)
}

pub fn get_config(dev: &Sd90) -> Result<Vec<u8>> {
    let head = get_descriptor(dev, DT_CONFIG, 0, 9)?;
    if head.len() < 4 {
        return Ok(head);
    }
    let total = u16::from_le_bytes([head[2], head[3]]) as usize;
    get_descriptor(dev, DT_CONFIG, 0, total)
}

pub fn hex_dump(data: &[u8]) {
    for (i, chunk) in data.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
        println!("{:04X}: {}", i * 16, hex.join(" "));
    }
}

fn xfer_type(bm: u8) -> &'static str {
    match bm & 0x03 {
        0 => "Control",
        1 => "Isochronous",
        2 => "Bulk",
        _ => "Interrupt",
    }
}

fn sync_type(bm: u8) -> &'static str {
    match (bm >> 2) & 0x03 {
        0 => "None",
        1 => "Asynchronous",
        2 => "Adaptive",
        _ => "Synchronous",
    }
}

/// コンフィグ記述子を頭から歩いて、必要なところだけ人間向けに出す。
pub fn print_config(data: &[u8]) {
    let mut i = 0usize;
    let mut cur_if: i32 = -1;
    let mut cur_alt: i32 = -1;
    while i + 2 <= data.len() {
        let len = data[i] as usize;
        if len < 2 || i + len > data.len() {
            println!("{:04X}: 壊れた記述子 (bLength={})", i, len);
            break;
        }
        let d = &data[i..i + len];
        match d[1] {
            DT_CONFIG if len >= 9 => {
                println!(
                    "CONFIG  wTotalLength={} bNumInterfaces={} bmAttributes=0x{:02X} bMaxPower={}mA",
                    u16::from_le_bytes([d[2], d[3]]),
                    d[4],
                    d[7],
                    d[8] as u32 * 2
                );
            }
            DT_INTERFACE if len >= 9 => {
                cur_if = d[2] as i32;
                cur_alt = d[3] as i32;
                println!(
                    "  IF{} alt{}  endpoints={} class=0x{:02X} subclass=0x{:02X} protocol=0x{:02X}",
                    d[2], d[3], d[4], d[5], d[6], d[7]
                );
            }
            DT_ENDPOINT if len >= 7 => {
                let addr = d[2];
                let dir = if addr & 0x80 != 0 { "IN " } else { "OUT" };
                println!(
                    "    EP 0x{:02X} {} {:<11} sync={:<12} wMaxPacketSize={} bInterval={}",
                    addr,
                    dir,
                    xfer_type(d[3]),
                    if d[3] & 0x03 == 1 {
                        sync_type(d[3])
                    } else {
                        "-"
                    },
                    u16::from_le_bytes([d[4], d[5]]),
                    d[6]
                );
            }
            DT_CS_INTERFACE if len >= 3 => match d[2] {
                0x01 if len >= 7 => println!(
                    "    CS_IF AS_GENERAL   bTerminalLink={} wFormatTag=0x{:04X}",
                    d[3],
                    u16::from_le_bytes([d[5], d[6]])
                ),
                0x02 if len >= 8 => {
                    let nch = d[4];
                    let subframe = d[5];
                    let bits = d[6];
                    let ftype = d[7];
                    let mut freqs = Vec::new();
                    if ftype == 0 {
                        // 連続レンジ (下限/上限)
                        if len >= 14 {
                            freqs.push(u32::from_le_bytes([d[8], d[9], d[10], 0]));
                            freqs.push(u32::from_le_bytes([d[11], d[12], d[13], 0]));
                        }
                    } else {
                        for k in 0..ftype as usize {
                            let o = 8 + k * 3;
                            if o + 3 <= len {
                                freqs.push(u32::from_le_bytes([d[o], d[o + 1], d[o + 2], 0]));
                            }
                        }
                    }
                    println!(
                        "    CS_IF FORMAT_TYPE ch={} bSubframeSize={} bBitResolution={} freq={:?}",
                        nch, subframe, bits, freqs
                    );
                    if (bits as usize).div_ceil(8) > subframe as usize {
                        println!(
                            "      ※ bSubframeSize={} に {}bit は入らない。UAC1 的には不整合",
                            subframe, bits
                        );
                    }
                }
                st => println!("    CS_IF subtype=0x{st:02X} len={len}"),
            },
            DT_CS_ENDPOINT if len >= 3 => {
                println!("    CS_EP subtype=0x{:02X} len={}", d[2], len);
            }
            t => println!("  (IF{cur_if} alt{cur_alt}) 記述子 type=0x{t:02X} len={len}"),
        }
        i += len;
    }
}

pub fn print_device_descriptor(d: &[u8]) {
    if d.len() < 18 {
        println!("デバイス記述子が短い ({} bytes)", d.len());
        return;
    }
    println!(
        "DEVICE  bcdUSB=0x{:04X} class=0x{:02X}/0x{:02X} VID=0x{:04X} PID=0x{:04X} bcdDevice=0x{:04X} numConfig={}",
        u16::from_le_bytes([d[2], d[3]]),
        d[4],
        d[5],
        u16::from_le_bytes([d[8], d[9]]),
        u16::from_le_bytes([d[10], d[11]]),
        u16::from_le_bytes([d[12], d[13]]),
        d[17]
    );
}

pub fn dump(dev: &Sd90) -> Result<()> {
    let dd = get_descriptor(dev, DT_DEVICE, 0, 18)?;
    print_device_descriptor(&dd);
    println!();
    let cfg = get_config(dev)?;
    print_config(&cfg);
    println!("\n=== HEX ({} bytes) ===", cfg.len());
    hex_dump(&cfg);
    Ok(())
}
