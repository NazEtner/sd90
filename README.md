# sd90

<p>
<img src="assets/trademarks/asio-compatible.png" alt="ASIO Compatible" width="120">
<img src="assets/trademarks/vst-compatible.png" alt="VST Compatible" width="120">
</p>

EDIROL SD-90 (Roland Studio Canvas) を、現行の Windows x64 で WinUSB 経由で使うためのプロジェクトです。
純正ドライバは署名の問題で読み込まれないため、カーネルドライバは書かず、すべてユーザーモードで処理します。

| クレート | 内容 |
| --- | --- |
| [`sd90-rs`](sd90-rs) | デバイス層。ASIO® ドライバ (`sd90_asio.dll`) と調査用のプローブ |
| [`sd90-seq`](sd90-seq) | SMF を再生しながら、同じ USB で音を録音するツール |
| [`sd90-vst`](sd90-vst) | SD-90 を VST® 3 インストゥルメントとして扱えるようにするプラグインと単体エディタ |

```
cargo build --release
```

成果物はルートの `target\release` に出力されます。ビルド済みのものは
[Releases](https://github.com/NazEtner/sd90/releases) からも入手できます。事前準備 (Vendor モードへの切り替えと Zadig での WinUSB の割り当て) は
[`sd90-rs` の README](sd90-rs/README.md#準備) を参照してください。

## ライセンス

GPL-3.0 ([LICENSE](LICENSE))。Copyright (C) 2026 Nananami (NazEtner)

ASIO の部分は ASIO SDK の GPLv3 オプションに従います。VST 3 の ABI 宣言は
VST 3 SDK 3.8 (MIT) をもとに書き起こしたもので、Steinberg の著作権表示は
[`sd90-vst/src/vst3/mod.rs`](sd90-vst/src/vst3/mod.rs) の冒頭にあります。

## 商標

ASIO is a registered trademark of Steinberg Media Technologies GmbH.
VST is a registered trademark of Steinberg Media Technologies GmbH.
Roland and EDIROL are trademarks of Roland Corporation.

本プロジェクトは非公式のもので、Steinberg および Roland とは一切関係ありません。

`assets/trademarks/` のロゴは Steinberg の配布物 (ASIO SDK 2.3.4 / VST 3 SDK の artwork)
を無加工で収録したもので、**GPL の対象外**です。取り扱いは Steinberg の Usage Guidelines に従います。
