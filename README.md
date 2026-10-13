# sd90

EDIROL SD-90 (Roland Studio Canvas) を、今の Windows x64 で WinUSB 経由で使うためのもの。
純正ドライバは署名の問題で読み込まれないので、カーネルドライバを書かずに全部ユーザーモードでやる。

| クレート | 中身 |
| --- | --- |
| [`sd90-rs`](sd90-rs) | デバイス層。ASIO ドライバ (`sd90_asio.dll`) と調査用のプローブ |
| [`sd90-seq`](sd90-seq) | SMF を流しながら同じ USB で音を録る |
| [`sd90-vst`](sd90-vst) | SD-90 を VST3 インストゥルメントとして見せるプラグインと単体エディタ |

```
cargo build --release
```

成果物はルートの `target\release` にできる。準備 (Vendor モード + Zadig で WinUSB) は
[`sd90-rs` の README](sd90-rs/README.md#準備) を参照。

## ライセンス

MIT ([LICENSE](LICENSE))。
