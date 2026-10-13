# sd90

<p>
<img src="assets/trademarks/asio-compatible.png" alt="ASIO Compatible" width="120">
<img src="assets/trademarks/vst-compatible.png" alt="VST Compatible" width="120">
</p>

EDIROL SD-90 (Roland Studio Canvas) を、今の Windows x64 で WinUSB 経由で使うためのもの。
純正ドライバは署名の問題で読み込まれないので、カーネルドライバを書かずに全部ユーザーモードでやる。

| クレート | 中身 |
| --- | --- |
| [`sd90-rs`](sd90-rs) | デバイス層。ASIO® ドライバ (`sd90_asio.dll`) と調査用のプローブ |
| [`sd90-seq`](sd90-seq) | SMF を流しながら同じ USB で音を録る |
| [`sd90-vst`](sd90-vst) | SD-90 を VST® 3 インストゥルメントとして見せるプラグインと単体エディタ |

```
cargo build --release
```

成果物はルートの `target\release` にできる。準備 (Vendor モード + Zadig で WinUSB) は
[`sd90-rs` の README](sd90-rs/README.md#準備) を参照。

## ライセンス

GPL-3.0 ([LICENSE](LICENSE))。Copyright (C) 2026 Nananami (NazEtner)

ASIO の部分は ASIO SDK の GPLv3 オプションに従う。VST 3 の ABI 宣言は
VST 3 SDK 3.8 (MIT) から起こしたもので、Steinberg の著作権表示は
[`sd90-vst/src/vst3/mod.rs`](sd90-vst/src/vst3/mod.rs) の頭にある。

## 商標

ASIO is a registered trademark of Steinberg Media Technologies GmbH.
VST is a registered trademark of Steinberg Media Technologies GmbH.
Roland and EDIROL are trademarks of Roland Corporation.

非公式のプロジェクトで、Steinberg とも Roland とも関係は無い。

`assets/trademarks/` のロゴは Steinberg の配布物 (ASIO SDK 2.3.4 / VST 3 SDK の artwork)
を無加工で置いたもので、**GPL の対象外**。扱いは Steinberg の Usage Guidelines に従う。
