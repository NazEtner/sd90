//! 単体版の音声出力。WASAPI の共有モードでいつもの出力デバイスに出す。
//!
//! DAW がやるのと同じく、**出力デバイスの時計で `process` を回す**。
//! デバイスが「これだけ書け」と言ってきた分を、`MAX_BLOCK` 以下に刻んで
//! `process` に渡し、返ってきた音をそのまま書く。SD-90 の水晶と PC の
//! サウンドデバイスの水晶のずれは、プラグインのリサンプラがいつも通り吸う。
//!
//! 形式は 48kHz / float32 / ステレオ固定。デバイスのミックス形式が違っても
//! `AUTOCONVERTPCM` で Windows に変換させる (プラグインは 48kHz 以外を断る)。

use crate::hostutil::SendPtr;
use sd90_vst::vst3::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, WAVEFORMATEX, eConsole, eRender,
};
use windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT;
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

/// `setupProcessing` に申告する最大ブロック長。プラグインの滞留量 B
/// (= レイテンシ) はこれで決まるので、WASAPI の 1 周期 (10ms = 480) が
/// 1 回で収まる最小の値にする。
pub const MAX_BLOCK: usize = 512;
/// WASAPI に頼むバッファの長さ (100ns 単位)。20ms。
const BUFFER_HNS: i64 = 200_000;

/// 出力デバイスを開いて回し始める。開けなければ `Err` (呼び手はタイマ駆動に
/// 切り替える)。`stop` が立つと止まる。
pub fn start(p: SendPtr, stop: Arc<AtomicBool>) -> Result<std::thread::JoinHandle<()>, String> {
    // 開くところまでを呼び手のスレッドで待ち、成否を返す。
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let h = std::thread::Builder::new()
        .name("sd90-audio".into())
        .spawn(move || {
            let p = p;
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            match open() {
                Ok((client, render, event, frames)) => {
                    let _ = tx.send(Ok(()));
                    let _mmcss = sd90_rs::iso::join_pro_audio();
                    run(&p, &client, &render, event, frames, &stop);
                    unsafe {
                        let _ = client.Stop();
                        let _ = CloseHandle(event);
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
            unsafe { CoUninitialize() };
        })
        .map_err(|e| e.to_string())?;
    match rx.recv() {
        Ok(Ok(())) => Ok(h),
        Ok(Err(e)) => {
            let _ = h.join();
            Err(e)
        }
        Err(_) => Err("音声スレッドが落ちた".into()),
    }
}

type Opened = (IAudioClient, IAudioRenderClient, windows::Win32::Foundation::HANDLE, u32);

fn open() -> Result<Opened, String> {
    unsafe {
        let en: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| format!("デバイス列挙: {e}"))?;
        let dev = en
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("既定の出力デバイスが無い: {e}"))?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(|e| format!("Activate: {e}"))?;
        let fmt = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT as u16,
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 48_000 * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                BUFFER_HNS,
                0,
                &fmt,
                None,
            )
            .map_err(|e| format!("IAudioClient::Initialize: {e}"))?;
        let event = CreateEventW(None, false, false, None).map_err(|e| e.to_string())?;
        client.SetEventHandle(event).map_err(|e| e.to_string())?;
        let frames = client.GetBufferSize().map_err(|e| e.to_string())?;
        let render: IAudioRenderClient = client.GetService().map_err(|e| e.to_string())?;
        client.Start().map_err(|e| format!("Start: {e}"))?;
        Ok((client, render, event, frames))
    }
}

fn run(
    p: &SendPtr,
    client: &IAudioClient,
    render: &IAudioRenderClient,
    event: windows::Win32::Foundation::HANDLE,
    buffer_frames: u32,
    stop: &AtomicBool,
) {
    let (proc_, pvt) = (p.0, p.1);
    let mut chans = [[0f32; MAX_BLOCK]; 2];
    while !stop.load(Ordering::Relaxed) {
        // デバイスが取りに来るまで待つ。止める合図を見るために時間は区切る。
        if unsafe { WaitForSingleObject(event, 100) } != WAIT_OBJECT_0 {
            continue;
        }
        let Ok(padding) = (unsafe { client.GetCurrentPadding() }) else {
            // デバイスが抜けた等。続けても書けないので畳む。
            return;
        };
        let want = buffer_frames.saturating_sub(padding) as usize;
        if want == 0 {
            continue;
        }
        let Ok(buf) = (unsafe { render.GetBuffer(want as u32) }) else {
            return;
        };
        let out = unsafe { std::slice::from_raw_parts_mut(buf as *mut f32, want * 2) };
        let mut done = 0usize;
        while done < want {
            let n = (want - done).min(MAX_BLOCK);
            let mut ptrs = [chans[0].as_mut_ptr(), chans[1].as_mut_ptr()];
            let mut bus = AudioBusBuffers {
                num_channels: 2,
                silence_flags: 0,
                channel_buffers: ptrs.as_mut_ptr(),
            };
            let mut data = ProcessData {
                process_mode: K_REALTIME,
                symbolic_sample_size: K_SAMPLE32,
                num_samples: n as i32,
                num_inputs: 0,
                num_outputs: 1,
                inputs: std::ptr::null_mut(),
                outputs: &mut bus,
                input_parameter_changes: std::ptr::null_mut(),
                output_parameter_changes: std::ptr::null_mut(),
                input_events: std::ptr::null_mut(),
                output_events: std::ptr::null_mut(),
                process_context: std::ptr::null_mut(),
            };
            unsafe { ((*pvt).process)(proc_, &mut data) };
            for k in 0..n {
                out[(done + k) * 2] = chans[0][k];
                out[(done + k) * 2 + 1] = chans[1][k];
            }
            done += n;
        }
        unsafe {
            let _ = render.ReleaseBuffer(want as u32, 0);
        }
    }
}
