# sd90-vst

<p>
<img src="../assets/trademarks/vst-compatible.png" alt="VST Compatible" width="120">
</p>

EDIROL SD-90 を DAW から「ただのソフトシンセ」に見せる。
デバイスを触る層は [`sd90-rs`](../sd90-rs) にある。

現状 SD-90 を DAW で使うには、オーディオドライバで音を入れて / MIDI ポートを別途生やして /
トラックの遅延を手で合わせる、の 3 つを人間が繋ぐ必要がある。仮想 MIDI ポートを
足しても、いちばん面倒な**手動のレイテンシ合わせ**は残る。

MIDI 入力と音声出力を 1 個のプラグインに封じ込めれば、遅延の申告はプラグインの
仕事になり、位置合わせはホストの PDC がやる。専用のオーディオドライバも仮想 MIDI ポートも
要らなくなる。

```
DAW の MIDI イベント          RT スレッド          送出スレッド        SD-90
 process() の inputEvents ---> 4byte パケット + ---> デバイスフレーム ---> IF2 bulk
   (sampleOffset 付き)          送出フレーム        まで待って送る       EP 0x02

 process() の出力バッファ <--- Hermite 補間で <--- ring (24bit 詰め) <--- IF1 iso IN
   (f32 stereo)                 ドリフト吸収                             EP 0x81
```

iso OUT (IF0) は使わない。VST® 3 プラグインは音を送り込む必要がないので、`StreamCfg::capture_only`
で IF0 の alt を上げずに回す。

## いまどこまで

VST3 プラグインとして一通り動く。DAW での確認はこれから。

* `engine.rs` / `resample.rs` … 実機で検証済み
* `vst3/` / `plugin.rs` / `factory.rs` … VST3 の殻。手書き
* `main.rs` … プローブ 3 本 (中身直叩き / DLL 越しの最小 VST3 ホスト / エディタ)
* `gui/` / `sd90.rs` / `tones.rs` / `control.rs` … 専用エディタと状態の保存 (下の「エディタ」)

まだ無いもの: DIN IN のイベント出力、MFX / リバーブ / コーラスの詳細パラメータ
(タイプとレベルまで)、GS / XGlite の音色名。

## エディタ

本体のパネルと Owner's Manual の章立てに合わせてある。仕様の出典は Roland の
公式 PDF 3 本 (Owner's Manual `SD-90_OM.pdf` / MIDI Implementation
`SD-90_MI.pdf` / Parameter List `SD-90_PR.pdf`)。

| 画面 | 中身 | 出典 |
| --- | --- | --- |
| 上段 | 音源モード (GM2 / Native / GS / XGlite)、接続状態、出力メータ、PANIC | OM p.20, p.77 |
| 左 | 32 パート (A01-B16)。INST/DRUM、セット、番号、音色名、M / S | OM p.19-27 |
| INST/DRUM SET | 音色選択 + PREVIEW、パートパラメータ一式 (Native 専用分を含む) | OM p.21-37 |
| EFFECTS | リバーブ / コーラス (GM2 は GM2 のタイプ)、MFX-A/B/C (Native)、EQ | OM p.38-47 |
| ALL | Master Volume / Fine (Hz) / Coarse | OM p.31, p.37 |
| SYSTEM | Device ID、一括送信、INIT、診断 | OM p.70, p.72 |

送り分けはマニュアルの記述どおり。音色は Bank Select + Program Change
(GM2 の音色セットだけ DT1 `10 00 pp 3F`)、それ以外のパートパラメータと
エフェクトは DT1、GM2 のリバーブ / コーラスのタイプは Universal Realtime の
Global Parameter Control。音色名の表は OM 付録 (p.129-134, p.139) から起こした。

### 単体版 (`sd90-editor.exe`)

DAW 無しでエディタだけを使う。SD-90 は本体に OUTPUT / ヘッドホン端子が
あるので、音色やエフェクトをいじるだけなら DAW は要らない。

```
cargo build --release      ルートの target\release\sd90-editor.exe ができる
```

中身はプラグインそのもの。DAW と同じ経路で生成し、エディタを自前のウィンドウに
付ける (`src/standalone.rs`)。

音は Windows の既定の出力デバイスにも出す (`src/audio.rs`)。WASAPI の共有
モード / イベント駆動で、**出力デバイスの時計で `process` を回す** (DAW と同じ)。
形式は 48kHz / float32 / ステレオで、デバイスのミックス形式が違えば Windows に
変換させる (`AUTOCONVERTPCM`)。SD-90 と PC のサウンドデバイスの水晶のずれは
プラグインのリサンプラが吸う。最大ブロックを 512 にしているので申告レイテンシは
770 フレーム (16ms)。出力デバイスが無ければタイマで回し、音は本体の端子だけになる。設定は
`%APPDATA%\sd90-vst\standalone.state` に残り、次に開いたときに本体へ送り直す。
[`sd90-rs`](../sd90-rs) のドライバや DAW の中のプラグインとは同時に使えない (掴めなければ OFFLINE で開く)。

### 画面の鍵盤

右上の KEYBOARD で出し入れする (単体版は最初から出ている)。選んでいるパートの
Port / Rx CH に送る。送り道はエディタの他の操作と同じ (`process` 経由) なので、
DAW の中で使っても DAW のノートとぶつからない (`src/gui/keyboard.rs`)。

* マウス: 押すと鳴り、押したまま滑らせるとグリッサンド
* PC のキー: `Z S X D C V G B H N J M ,` が下の段、`Q 2 W 3 E R 5 T 6 Y 7 U I` が 1 オクターブ上
  (数値欄に打ち込んでいる間は効かない)
* OCT -/+、Vel、HOLD (CC#64)、ピッチベンド (離すと中央に戻る)
* パートを替えたとき、鍵盤を閉じたとき、ウィンドウを閉じたときは押さえている音を全部離す

### エディタからの送信は必ず process を通す

`MidiSched` は単一生産者のリングで、送出スレッドは「フレームの昇順に積まれる」
前提で先頭から待つ。UI スレッドが勝手に積むと DAW のノートを追い越したり、
未来のフレームに積んだものが後続を塞いだりする。なのでエディタは `Control` の
送信待ちに置くだけにし、`process` が毎ブロック `try_lock` で取り出して
**そのブロックの先頭フレーム**に積む。取り出す量は DIN の速度 (3125 B/s) に
絞る。モード切り替え直後は 300ms 空ける。

### 状態の保存と送り直し

エディタの状態は `getState` / `setState` でプロジェクトに入る。エディタで何か
触ると「開いたときに送り直す」が立ち、プロジェクトを開いたときと本体を掴み
直したときに、モード切り替えから始めて初期値と違うところだけを送る。触って
いなければ何も送らない (曲頭の GM リセット等は DAW 側のデータに任せる)。

### ウィンドウ

ホストの HWND の下に子ウィンドウを作り、WGL の上で egui (glow) を描く。
メッセージループはホストのもの。ウィンドウクラスは DLL の HINSTANCE で登録し、
最後のウィンドウを閉じたら外す (EXE の HINSTANCE のままだと、DLL が外れた後に
WNDPROC が宙に浮く)。GL のコンテキストは描く間だけ current にして元に戻す。

### ライセンス

GPL-3.0 (ワークスペース共通)。VST3 の ABI 宣言 (`src/vst3/`) は `vst3_pluginterfaces` 3.8.1
から起こしたもので、SDK は 3.8 から MIT なので GPL に取り込める。Steinberg の著作権表示は
`src/vst3/mod.rs` の頭に置いてある。

## レイテンシの決め方

プラグインは「イベントを先に受け取る」ことができない。できるのは実際の遅れを
正しく申告することだけで、それさえ合っていればホストが他トラックをずらして
辻褄を合わせる。

* `B` … リングの目標滞留量。出力はライブの取り込みから常に B 遅れる。
* `d` … 音源の MIDI 受信から発音までの応答。**実測 162 フレーム / 3.38ms**。
* 申告レイテンシ **`L = B + d`**。

このとき、ブロック先頭の読み出し位置を `R` として、
**sampleOffset `k` のイベントはデバイスフレーム `R + B + k` に送ればよい**。

導出は `engine.rs` の頭に書いた。要点は、ここにホストの時計が一切出てこないこと。
`R` はリングの読み出し位置から厳密に決まり、`B` は定数。デバイスクロックを
見るのは送出スレッドだけになる。

### B はブロック長を下回ってはいけない

ホストは 1 ブロックぶんを一度に吸っていくので、その瞬間にリングがブロック長を
持っていなければ底を打つ。B を 4ms 固定にして 256 フレーム (5.33ms) のブロックを
回すと、水位の平均は足りているのにアンダーランが出る。`safety_for(max_block)` は
`max_block + 96` (2ms) を返す。VST3 では `setupProcessing` の `maxSamplesPerBlock`
を入れること。

### 水位が目標からずれたら捨ててでも戻す

サーボの引き戻しは高々 0.2%/s しかない。目標から数百フレームずれた状態を
放っておくと数秒間ずれたままで、そのあいだ**申告レイテンシが実際の遅れと
合わない** — ホストの PDC がその分そのままずれる。

なので助走のやり直し時 (起動直後とアンダーラン直後) に余剰をまとめて捨て、
水位を目標ちょうどから始める。どのみちそこでは波形が切れているので、
連続性より申告値の正しさを取る。これを入れる前は、ブロック 512 で
目標 608 に対して水位が 860 に張り付いていた (5ms のずれ)。

## 踏んだ罠: リサンプラが 1 ブロックに 1 サンプル捨てても統計に出ない

補間のために吸い出すフレーム数は
`floor(位相 + 比 x (n-1))` であって `floor(位相 + 比 x n)` ではない。後者だと
毎ブロック 1 フレーム多く吸い出して使わないまま捨てる。

始末が悪いのは、**サーボが比を緩めて水位だけ辻褄を合わせてしまう**こと。
水位もアンダーラン数も読み出し比も正常に見えたまま、波形にだけ不連続が出る。
`sd90-rs` の「パケット記述子の Offset は転送相対」と同じ形の罠。

プローブがブロック境界の段差を直接数えているのはこのため。判定基準は
`iso.rs` の転送境界の検査と同じ (平均段差の 8 倍) だが、**全体平均ではなく
継ぎ目の周りの局所平均**で見る。減衰音のように区間内で振幅が 60dB 変わる
信号だと、全体平均は無音区間に引っ張られて機能しない。

## 組む / 入れる

```
.\install.ps1              ルートの target\release にバンドルを組む
.\install.ps1 -Install     共通の VST3 フォルダに入れる (管理者権限)
```

`.vst3` は DLL 1 個ではなく**フォルダ**で、中身は

```
SD-90.vst3\Contents\x86_64-win\SD-90.vst3    <- 拡張子 .vst3 の DLL
```

DLL 単体を置いても拾うホストはあるが、規格はこちら。

## 使い方

```
sd90-vst-probe [ms] [ブロック長] [out.wav] [B]     中身を直接回す
sd90-vst-probe vst3test [dll] [ms] [ブロック長]    DLL 越しに VST3 として回す
sd90-vst-probe editor [dll] [手順]                 エディタを開いて操作・撮影する
```

`vst3test` は DAW も Steinberg の validator も無しに、`LoadLibrary` →
`GetPluginFactory` → クラスファクトリ → `IComponent` / `IAudioProcessor` /
`IEditController` / `IMidiMapping` の全経路を叩く。`sd90-rs asiotest` と同じ役割。

```
SD-90 / Audio Module Class / Instrument|Synth / SDK VST 3.8.1
音声出力: 1 本  [0] SD-90 (2ch, 既定で有効)
イベント入力: 4 本  [0] PART A  [1] PART B  [2] MIDI OUT 1  [3] MIDI OUT 2
パラメータ数: 8320
CC7 -> id 7 "PART A ch1 CC7"
setupProcessing(44.1kHz): 正しく断った
申告レイテンシ: 514 フレーム (10.71ms)
最後の release 後の参照数: 0
288000 フレーム / 左chピーク 0.0786 (-22.1 dBFS)
```

DAW も VST3 SDK も無しに、「一定ブロックで `pull` し続けながら、ブロック相対の
オフセットで MIDI を積む」という本番と同じ経路を叩く。`sd90-rs` 側の
ドライバ自己検証と同じ役割。

鳴らす中身は環境変数で差し替えられる:

```
SD90_PROBE_CH=10 SD90_PROBE_PROG=- SD90_PROBE_NOTES=38,42
SD90_PROBE_RESET=-        GM System On を送らない
```

既定では先頭に **GM System On** を送る。パートが黙っている状態の個体でも
これで鳴るようになる (実際、ch 1 が無音で ch 10 だけ鳴る状態から始まった)。

### 実測 (15 秒 x 4 条件, 弦の和音を鳴らしながら)

| ブロック | B | アンダーラン | 水位 (目標) | 読み出し比 | 境界の不連続 |
| --- | --- | --- | --- | --- | --- |
| 128 | 224 | 0 | 224.2 (224) | 1.000046 | 1/5624 |
| 256 | 352 | 0 | 352.6 (352) | 1.000082 | 1/2811 |
| 512 | 608 | 0 | 607.9 (608) | 0.999989 | 1/1405 |
| 1024 | 1120 | 0 | 1120.5 (1120) | 1.000022 | 1/702 |

読み出し比が 1.0 の 100ppm 以内に収まるのは、両者の水晶の差がその程度という
こと。境界の不連続 1 は、いずれもノートのアタックが継ぎ目に当たったもの。

なお `sleep` 任せでホストのふりをしてはいけない。Windows の既定のタイマ分解能は
15.6ms あり、10ms 級のブロックだと 1 周期まるごと寝過ごす。実際のホストは
オーディオデバイスの割り込みで駆動されていてこんなに粗くないので、粗いまま
測ると「ホストのふりが下手なせいのアンダーラン」をプラグインの欠陥として
数えることになる。プローブは締め切りの 2ms 前からは譲るだけにしている。

## 前提

* 本体を Vendor モード (PID 0x0016) + Zadig で WinUSB
* **`sd90-rs` のドライバとは併用できない。** どちらも IF1 の iso IN を要求する
* 音を出せるのはプロセス内で 1 インスタンスだけ (録音バスが物理的に 1 系統)

## 構成

| ファイル | 役割 |
| --- | --- |
| `src/engine.rs` | デバイスの所有 (プロセス内シングルトン) とタイムラインの対応づけ |
| `src/resample.rs` | 24bit を f32 に開きつつ Hermite 補間でドリフトを吸収する読み出し |
| `src/vst3/mod.rs` | VST3 の ABI 宣言。サイズとオフセットはヘッダから実測して assert |
| `src/plugin.rs` | IComponent / IAudioProcessor / IEditController / IMidiMapping |
| `src/factory.rs` | クラスファクトリと DLL のエクスポート |
| `src/main.rs` | `sd90-vst-probe`。中身の直叩きと `vst3test` |
| `src/vst3host.rs` | `vst3test` 用の最小 VST3 ホスト (bin 側のみ) |
| `src/editorhost.rs` | `editor` 用の最小ホスト (bin 側のみ) |
| `src/standalone.rs` | 単体版 `sd90-editor` |
| `src/audio.rs` | 単体版の音声出力 (WASAPI) |
| `src/hostutil.rs` | 自前ホスト 2 つが共有する部品 (メモリ上の IBStream、process スレッド) |
| `src/sd90.rs` | 音源設定のモデル、DT1 等のメッセージ生成、保存形式 |
| `src/tones.rs` | 音色名の表 (OM 付録から) |
| `src/control.rs` | プラグインとエディタで共有する状態と送信待ち |
| `src/gui/` | `IPlugView`、Win32 + WGL のウィンドウ、egui の画面、画面の鍵盤 |

## 商標

VST is a registered trademark of Steinberg Media Technologies GmbH.
Roland and EDIROL are trademarks of Roland Corporation.

非公式のプロジェクトで、Steinberg とも Roland とも関係は無い。
