//! SD-90 (EDIROL Studio Canvas) を WinUSB 経由でユーザーモードから扱う。
//!
//! * `sd90-rs` バイナリ … 記述子ダンプと iso IN の取り込みプローブ
//! * `sd90_asio.dll`    … ASIO ドライバ (入出力 2ch)
//! * `usbmidi`          … IF2 の MIDI エンドポイント
//!
//! ここに置くのはデバイスを触る層まで。SMF を読む・曲を流すといった話は
//! 別クレート (sd90-seq) に置く。
//!
//! 経緯と実測値は README.md を参照。

pub mod asio;
pub mod comserver;
pub mod descriptor;
pub mod device;
pub mod iso;
pub mod latency;
pub mod log;
pub mod midisched;
pub mod ring;
pub mod stream;
pub mod usbmidi;
