//! SD-90 (EDIROL Studio Canvas) を DAW から「ただのソフトシンセ」に見せる。
//!
//! MIDI 入力は IF2 の bulk、音声出力は IF1 の iso IN。ハードウェア音源を
//! 使うのに要る 3 つの手間 — ASIO で音を入れる / MIDI ポートを生やす /
//! トラックの遅延を手で合わせる — を 1 個のプラグインに畳み込む。
//! レイテンシさえ正しく申告すれば、位置合わせはホストの PDC がやる。
//!
//! デバイスを触る層は [`sd90_rs`] にある。ここに置くのはタイムラインの
//! 対応づけと、ホストのブロックに合わせた読み出しまで。
//!
//! 経緯と実測値は README.md を参照。

pub mod control;
pub mod engine;
pub mod factory;
pub mod gui;
pub mod plugin;
pub mod resample;
pub mod sd90;
pub mod tones;
pub mod vst3;
