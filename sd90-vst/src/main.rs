//! ホストのふりをして VST3 プラグインの中身だけを回すプローブ。
//!
//! DAW も VST3 SDK も無しに、「一定ブロックで `pull` し続けながら、
//! ブロック相対のオフセットで MIDI を積む」という本番と同じ経路を叩く。
//! ASIO ドライバに対する `asiotest` と同じ役割。
//!
//! 見るのは 3 つ:
//!   1. アンダーランが 0 か (滞留量の目標 B が足りているか)
//!   2. 滞留量が目標に張り付くか (サーボが効いているか)
//!   3. ブロックの継ぎ目で波形が飛んでいないか (補間の位相が正しいか)
//!
//! 3 は統計に出ない。リサンプラが 1 ブロックあたり 1 サンプル捨てていても、
//! サーボが比を緩めて水位だけ辻褄を合わせるので、水位もアンダーランも
//! 正常なまま波形にだけ不連続が出る。必ず継ぎ目を直接見ること。

mod editorhost;
mod hostutil;
mod vst3host;

use sd90_rs::device::{CHANNELS, SAMPLE_RATE};
use sd90_vst::engine::{Engine, safety_for};
use std::io::Write;
use std::time::{Duration, Instant};

fn usage() {
    eprintln!(
        r#"sd90-vst-probe — VST3 プラグインの中身をホスト無しで回す

  sd90-vst-probe [ms] [ブロック長] [out.wav] [B]
  sd90-vst-probe vst3test [dll] [ms] [ブロック長]
  sd90-vst-probe editor [dll] [手順]

    ms         回す長さ (既定 8000)
    ブロック長 ホストのバッファサイズ (既定 256)
    out.wav    出力の書き出し先 (32bit float, 既定 probe.wav)
    B          リングの目標滞留量フレーム (既定 ブロック長 + 96)

前提: 本体を Vendor モード + Zadig で WinUSB。ASIO ドライバとは併用できない。
"#
    );
}

/// 32bit float の WAVE。fmt は cbSize つきの 18 バイト + fact を付ける。
fn write_wav(path: &str, samples: &[f32]) -> std::io::Result<()> {
    let data_len = (samples.len() * 4) as u32;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let ch = CHANNELS as u16;
    let byte_rate = SAMPLE_RATE * ch as u32 * 4;
    f.write_all(b"RIFF")?;
    f.write_all(&(50 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&18u32.to_le_bytes())?;
    f.write_all(&3u16.to_le_bytes())?; // IEEE float
    f.write_all(&ch.to_le_bytes())?;
    f.write_all(&SAMPLE_RATE.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&(ch * 4).to_le_bytes())?;
    f.write_all(&32u16.to_le_bytes())?;
    f.write_all(&0u16.to_le_bytes())?; // cbSize
    f.write_all(b"fact")?;
    f.write_all(&4u32.to_le_bytes())?;
    f.write_all(&((samples.len() / CHANNELS) as u32).to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()
}

/// ブロックの継ぎ目で波形が飛んでいないか。
///
/// iso.rs の転送境界の検査と考え方は同じ (平均段差の 8 倍を超えたら不連続)
/// だが、基準を**全体平均ではなく継ぎ目の周りの局所平均**にする。全体平均だと
/// 減衰音のように区間内で振幅が 60dB 変わる信号で機能しない。無音区間に平均が
/// 引っ張られ、鳴っている最中は継ぎ目でなくても全サンプルが閾値を超えるため。
fn boundary_report(left: &[f32], block: usize) {
    const WIN: usize = 64;
    if left.len() < block * 2 {
        return;
    }
    let step = |i: usize| (left[i] - left[i - 1]).abs() as f64;
    let (mut n, mut bad, mut sum_mean) = (0usize, 0usize, 0f64);
    let mut b = block;
    while b < left.len() {
        let (lo, hi) = (b.saturating_sub(WIN).max(1), (b + WIN).min(left.len()));
        // 継ぎ目そのものを平均に入れると自分で閾値を上げてしまう。
        let mut s = 0f64;
        let mut c = 0usize;
        for i in lo..hi {
            if i != b {
                s += step(i);
                c += 1;
            }
        }
        let local = if c > 0 { s / c as f64 } else { 0.0 };
        sum_mean += local;
        n += 1;
        if step(b) > (local * 8.0).max(1.0 / 65536.0) {
            bad += 1;
        }
        b += block;
    }
    if n == 0 {
        return;
    }
    let mean = sum_mean / n as f64;
    let thr = mean * 8.0;
    let pct = 100.0 * bad as f64 / n as f64;
    let verdict = if bad == 0 {
        "OK (継ぎ目で飛んでいない)"
    } else if pct < 1.0 {
        "OK (過渡音との区別がつかない水準)"
    } else {
        "NG (補間の位相かリングの吸い出し数が合っていない)"
    };
    println!("ブロック境界の不連続: {bad}/{n} ({pct:.1}%)  {verdict}");
    println!("  局所平均段差 {mean:.6} / 閾値の平均 {thr:.6}");
}

fn env_num(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("vst3test") {
        let dll = args
            .get(2)
            .map(String::as_str)
            .unwrap_or("target/release/sd90_vst.dll");
        let ms: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(6000);
        let block: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(256);
        return vst3host::run(dll, ms, block);
    }
    if args.get(1).map(String::as_str) == Some("editor") {
        let dll = args
            .get(2)
            .map(String::as_str)
            .unwrap_or("target/release/sd90_vst.dll");
        let script = args.get(3).map(String::as_str).unwrap_or("wait:1500,shot:editor.bmp");
        return editorhost::run(dll, script);
    }
    let ms: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(8000);
    let block: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(256);
    let out = args.get(3).map(String::as_str).unwrap_or("probe.wav");
    let safety: usize = args
        .get(4)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| safety_for(block));

    let eng = Engine::acquire(safety)?;
    let mut reader = eng.reader(block);
    println!(
        "ブロック {block} フレーム ({:.2}ms) / B = {} フレーム ({:.2}ms)\n\
         申告レイテンシ L = {} フレーム ({:.2}ms)",
        block as f64 * 1000.0 / SAMPLE_RATE as f64,
        safety,
        safety as f64 * 1000.0 / SAMPLE_RATE as f64,
        eng.latency_frames(),
        eng.latency_frames() as f64 * 1000.0 / SAMPLE_RATE as f64
    );

    let blocks = (ms * SAMPLE_RATE as u64 / 1000) as usize / block;
    let mut l = vec![0f32; block];
    let mut r = vec![0f32; block];
    let mut rec: Vec<f32> = Vec::with_capacity(blocks * block * CHANNELS);
    // ホストのコールバック間隔。デバイスの水晶とは当然ずれる。
    let period = Duration::from_secs_f64(block as f64 / SAMPLE_RATE as f64);
    let t0 = Instant::now();

    // 継ぎ目の検査には連続した波形が要るので、既定は弦の和音を鳴らしっぱなし。
    // 音が出ないときの切り分け用に、鳴らす中身を環境変数で差し替えられる。
    //   SD90_PROBE_CH=10 SD90_PROBE_PROG=- SD90_PROBE_NOTES=38,42
    let ch: u8 = env_num("SD90_PROBE_CH", 1).clamp(1, 16) as u8;
    let notes: Vec<u8> = match std::env::var("SD90_PROBE_NOTES") {
        Ok(s) => s.split(',').filter_map(|t| t.trim().parse().ok()).collect(),
        Err(_) => vec![60, 64, 67],
    };
    let st = 0x90 | (ch - 1);
    let mut intro: Vec<(u64, Vec<u8>)> = Vec::new();
    // 既知の状態から始める。パートが黙っている個体でもこれで鳴るようになる。
    // (SD90_PROBE_RESET=- で抑止)
    if std::env::var("SD90_PROBE_RESET").as_deref() != Ok("-") {
        intro.push((0, vec![0xF0, 0x7E, 0x7F, 0x09, 0x01, 0xF7])); // GM System On
    }
    match std::env::var("SD90_PROBE_PROG").as_deref() {
        Ok("-") => {}
        Ok(s) => intro.push((
            SAMPLE_RATE as u64 / 8,
            vec![0xC0 | (ch - 1), s.parse().unwrap_or(48)],
        )),
        // 既定は String Ensemble。減衰しない音のほうが継ぎ目を見やすい。
        Err(_) => intro.push((SAMPLE_RATE as u64 / 8, vec![0xC0 | (ch - 1), 48])),
    }
    for n in &notes {
        intro.push((SAMPLE_RATE as u64 / 4, vec![st, *n, 100]));
    }
    println!("鳴らす: ch {ch} / note {notes:?}");
    let mut sent = false;
    let mut silenced = false;

    for i in 0..blocks {
        // ホストのコールバックが来たつもりで待つ。
        //
        // sleep 任せにしてはいけない。Windows の既定のタイマ分解能は 15.6ms も
        // あり、10ms 級のブロックだと 1 周期まるごと寝過ごす。実際のホストは
        // オーディオデバイスの割り込みで駆動されていてこんなに粗くないので、
        // 粗いまま測ると「ホストのふりが下手なせいのアンダーラン」を
        // プラグインの欠陥として数えることになる。締め切り直前は譲るだけにする。
        let due = t0 + period.mul_f64(i as f64);
        while let Some(w) = due.checked_duration_since(Instant::now()) {
            if w > Duration::from_millis(2) {
                std::thread::sleep(w - Duration::from_millis(2));
            } else {
                std::thread::yield_now();
            }
        }

        // 本番と同じ順序: ブロック先頭の送出基点を取り、offset を足して積む。
        let base = eng.dispatch_base();
        if !sent {
            for (off, msg) in &intro {
                eng.schedule(base + off, msg, 0);
            }
            eng.flush();
            sent = true;
        }
        if !silenced && i + blocks / 8 >= blocks {
            for n in &notes {
                eng.schedule(base, &[0x80 | (ch - 1), *n, 0], 0);
            }
            eng.flush();
            silenced = true;
        }

        {
            let mut bufs: [&mut [f32]; CHANNELS] = [&mut l, &mut r];
            reader.pull(eng.ring(), &mut bufs);
        }
        for k in 0..block {
            rec.push(l[k]);
            rec.push(r[k]);
        }
    }

    eng.panic();
    std::thread::sleep(Duration::from_millis(100));

    let st = reader.stats();
    let (captured, overruns, dropped) = eng.diag();
    let wall = t0.elapsed().as_secs_f64();
    println!(
        "\n実時間 {wall:.2}s / 取り込み {captured} フレーム ({:.2}s)",
        captured as f64 / SAMPLE_RATE as f64
    );
    println!(
        "出力 {} フレーム / 吸い出し {} フレーム",
        rec.len() / CHANNELS,
        st.consumed
    );
    println!(
        "アンダーラン {} / リング溢れ {overruns} / 送出キュー溢れ {dropped}",
        st.underruns
    );
    println!(
        "滞留量の平滑値 {:.1} フレーム (目標 {safety}) / 読み出し比 {:.6}",
        st.level, st.ratio
    );

    let left: Vec<f32> = rec.iter().step_by(CHANNELS).copied().collect();
    let peak = left.iter().fold(0f32, |m, s| m.max(s.abs()));
    println!("左chピーク {:.4} ({:.1} dBFS)", peak, 20.0 * peak.log10());
    if peak < 0.001 {
        eprintln!("ほぼ無音。音源が鳴っているか、録音バスに乗っているかを確認すること");
    }
    boundary_report(&left, block);

    write_wav(out, &rec).map_err(|e| format!("{out} を書けない: {e}"))?;
    println!("-> {out}");
    Ok(())
}

fn main() -> std::process::ExitCode {
    if std::env::args().any(|a| a == "-h" || a == "--help") {
        usage();
        return std::process::ExitCode::SUCCESS;
    }
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("エラー: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
