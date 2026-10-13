//! ホストの中で動くので stdout が無い。診断は %TEMP%\sd90_asio.log に落とす。
//!
//! DAW には環境変数を渡せないので、有効化は**ファイルの有無**で行う。
//! 空の %TEMP%\sd90_asio.log を作っておくと、以後そこに追記する。
//! 無ければ何もしない (既定は無効)。
//!
//! 制御系メソッドからしか呼ばないこと。RT スレッド (bufferSwitch 経路) から
//! 呼ぶと当然そこで詰まる。

use std::io::Write;
use std::sync::{Mutex, OnceLock};

static LOCK: Mutex<()> = Mutex::new(());
static ENABLED: OnceLock<bool> = OnceLock::new();

pub fn path() -> std::path::PathBuf {
    std::env::temp_dir().join("sd90_asio.log")
}

pub fn log(msg: &str) {
    if !*ENABLED.get_or_init(|| path().exists()) {
        return;
    }
    let Ok(_g) = LOCK.lock() else { return };
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "?".into());
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path()) {
        let _ = writeln!(f, "{t} [{exe} tid={:?}] {msg}", std::thread::current().id());
    }
}
