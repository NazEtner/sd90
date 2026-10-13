//! SD-90 (EDIROL Studio Canvas) を WinUSB 経由で直接叩くプローブ。
//!
//! Roland 純正ドライバは署名が現行 Windows のカーネル CI を通らず
//! (STATUS_INVALID_IMAGE_HASH)、Generic モードは記述子の仕様違反で
//! 標準 usbaudio.sys に蹴られる (STATUS_DEVICE_CONFIGURATION_ERROR)。
//! そこでカーネルには一切踏み込まず、WinUSB + ユーザーモードで完結させる。
//!
//! 最終目標は ASIO ドライバだが、その前に確かめるべきことが 4 つある:
//!   1. WinUSB から記述子が読めるか
//!   2. iso IN が取りこぼさずに回るか
//!   3. 24bit のバイト配置が実際どうなっているか (bSubframeSize=2 の謎)
//!   4. 録音ストリームに内蔵音源の音が乗っているか
//! このバイナリはそこまでを担当する。

mod asiohost;

use sd90_asio::device::{self, PID_GENERIC, PID_VENDOR, Sd90};
use sd90_asio::{descriptor, iso, latency};

fn usage() {
    eprintln!(
        r#"sd90-rs — SD-90 WinUSB プローブ

  sd90-rs list                       WinUSB から見える SD-90 を探す
  sd90-rs desc                       デバイス/コンフィグ記述子をダンプ
  sd90-rs capture [ms] [out.raw]     IF1 / EP 0x81 から iso IN を取り込む
                                     既定 2000ms / capture.raw (.wav なら WAV で書く)
  sd90-rs asiotest [ms]              ASIO ドライバをレジストリ登録なしで検証
                                     既定 3000ms
  sd90-rs latency [回数] [ch] [note] 音源の応答 d (MIDI 送出 -> 発音) を実測する
                                     既定 8 回 / ch 10 (ドラム) / note 38

前提:
  * 本体の SYSTEM で USB Driver を Vender にして電源を入れ直す (PID 0x0016)
  * Zadig で PID 0x0016 に WinUSB を割り当てる
"#
    );
}

fn open_any() -> windows::core::Result<Sd90> {
    match Sd90::open(PID_VENDOR) {
        Ok(d) => Ok(d),
        Err(e) => {
            // Generic モードのまま挿している場合に気づけるようにする。
            if let Ok(paths) = device::find_device_paths(device::VID, PID_GENERIC)
                && !paths.is_empty()
            {
                eprintln!(
                    "PID 0x0017 (Generic モード) が見つかった。\n\
                     本体の SYSTEM で USB Driver を Vender にして電源を入れ直すこと。"
                );
            }
            Err(e)
        }
    }
}

fn run() -> windows::core::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("list") => {
            let all = device::enumerate_usb_device_paths()?;
            println!("USB デバイスインタフェース総数: {}", all.len());
            for p in all.iter().filter(|p| p.to_uppercase().contains("VID_0582")) {
                println!("  (VID_0582 該当) {p}");
            }
            for (label, pid) in [("Vendor ", PID_VENDOR), ("Generic", PID_GENERIC)] {
                let paths = device::find_device_paths(device::VID, pid)?;
                if paths.is_empty() {
                    println!("[{label} 0x{pid:04X}] 見つからない");
                    continue;
                }
                for p in paths {
                    // WinUSB が当たっていなければ Initialize が失敗する。
                    let ok = match Sd90::open_path(&p) {
                        Ok(_) => "WinUSB OK".to_string(),
                        Err(e) => format!("WinUSB 不可 ({})", e.message()),
                    };
                    println!("[{label} 0x{pid:04X}] {ok}\n    {p}");
                }
            }
            Ok(())
        }
        Some("desc") => {
            let dev = open_any()?;
            println!("path: {}\n", dev.path);
            descriptor::dump(&dev)
        }
        Some("capture") => {
            let ms: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2000);
            let out = args.get(3).map(String::as_str).unwrap_or("capture.raw");
            let dev = open_any()?;
            println!("path: {}", dev.path);
            println!("{ms}ms 取り込む…");
            let stats = iso::capture(&dev, ms, out)?;
            iso::report(&stats, out);
            Ok(())
        }
        Some("latency") => {
            let d = latency::Opts::default();
            let opts = latency::Opts {
                count: args.get(2).and_then(|s| s.parse().ok()).unwrap_or(d.count),
                channel: args
                    .get(3)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(d.channel),
                note: args.get(4).and_then(|s| s.parse().ok()).unwrap_or(d.note),
                ..d
            };
            latency::measure(&opts)
                .map_err(|e| windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e))
        }
        Some("asiotest") => {
            let ms: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3000);
            match asiohost::run(ms) {
                Ok(()) => Ok(()),
                Err(e) => Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_FAIL,
                    e,
                )),
            }
        }
        _ => {
            usage();
            Ok(())
        }
    }
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("エラー: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
