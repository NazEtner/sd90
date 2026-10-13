//! SMF を SD-90 に流しながら、同じ USB で音を録る。
//!
//! MIDI は IF2 の bulk、音は IF1 の iso。どちらも同じデバイスなので、
//! 「録音の何サンプル目でどのイベントを送ったか」が確定する。
//! 送出位置は out.wav.map に残るので、解析側はそれを見れば楽譜と音が繋がる。

mod render;
mod smf;

fn usage() {
    eprintln!(
        r#"sd90-seq — SMF を SD-90 で鳴らして録る

  sd90-seq render <in.mid> [out.wav] [cable]
        既定 out=render.wav / cable=0 (PART A)

前提: SD-90 の MIDI StartUp = USB、Vendor モード、Zadig で WinUSB。
      音源が無反応になったら Native On を送るか本体を入れ直す。
"#
    );
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let r = match args.get(1).map(String::as_str) {
        Some("render") => match args.get(2) {
            Some(mid) => {
                let out = args.get(3).map(String::as_str).unwrap_or("render.wav");
                let mut opts = render::Opts::default();
                if let Some(c) = args.get(4).and_then(|s| s.parse().ok()) {
                    opts.cable = c;
                }
                render::run(mid, out, &opts)
            }
            None => {
                usage();
                Ok(())
            }
        },
        _ => {
            usage();
            Ok(())
        }
    };
    match r {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("エラー: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
