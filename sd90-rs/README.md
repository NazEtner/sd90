# sd90-rs

<p>
<img src="../assets/trademarks/asio-compatible.png" alt="ASIO Compatible" width="120">
</p>

EDIROL SD-90 (Roland Studio Canvas) を WinUSB 経由でユーザーモードから扱う。

* `sd90_asio.dll` … ASIO® ドライバ (入出力 2ch)
* `sd90-rs.exe` … 記述子ダンプ / iso 取り込み / ASIO 自己検証のプローブ
* `usbmidi` … IF2 の MIDI エンドポイント (音源に MIDI を渡す唯一の口)

SMF を読んで曲を流す話は別クレート [`sd90-seq`](../sd90-seq) に置いてある。
ここはデバイスを触る層まで。

カーネルドライバは一切書いていない。したがってカーネル署名も EV 証明書も要らない。

## なぜカーネルに行かないか

- Roland 純正ドライバ (`rdif1016.inf` / `Rdwm1016.sys`, 2007年) は
  VeriSign Class 3 Code Signing 2004 CA / SHA-1 署名で、現行 Windows の
  カーネルコード整合性を通らない。実機では `STATUS_INVALID_IMAGE_HASH (0xC0000428)`。
  セキュアブート無効・HVCI 無効でも同じ (x64 のカーネル DSE は常時有効)。
- Generic モード (PID 0x0017) は標準 `usbaudio.sys` が当たるが、
  `STATUS_DEVICE_CONFIGURATION_ERROR (0xC0000182)` で起動失敗。
  AudioControl の `bInCollection` が MIDIStreaming インタフェース (IF3) を
  列挙しておらず、UAC1 仕様違反になっているのが原因と見られる。
- `winusb.sys` は Microsoft 署名済みなので、こちらのコードには
  カーネル署名が一切要らない。だから全部ユーザーモードでやる。

## 実機から読んだ記述子 (Vendor モード, PID 0x0016)

Full Speed (USB 1.1), 3 インタフェース, `bDeviceClass=0xFF` (非コンポジット)。

```
IF0  再生 (host -> SD-90)   class FF/02
     alt0 = エンドポイント無し
     alt1 = EP 0x01 OUT  iso  sync=None  wMaxPacketSize=320  bInterval=1
IF1  録音 (SD-90 -> host)   class FF/02
     alt0 = エンドポイント無し
     alt1 = EP 0x81 IN   iso  sync=None  wMaxPacketSize=320  bInterval=1
IF2  MIDI                   class FF/FF
     alt0 = EP 0x02 OUT / 0x82 IN  bulk 32B
     alt1 = EP 0x02 OUT bulk / 0x82 IN interrupt  32B
```

両ストリームとも `FORMAT_TYPE_I`, PCM, 2ch, 24bit, 48000Hz 固定 (離散1個)。
クラスコードは 0xFF だが、記述子の中身は UAC1 そのもの。

`sync type = None` なので**フィードバックエンドポイントは不要**。
帯域は 320B/frame × 2 方向で、フルスピードの iso 予算に対して十分余裕がある。

### 実測で確定したこと (3 秒 x 2 回, いずれもエラー 0 / 取りこぼし 0)

- **ペイロードは 24bit リトルエンディアン / 2ch インタリーブの生 PCM。独自ヘッダ無し。**
  記述子の `bSubframeSize=2` は単純に誤りで、実際は 3 バイト/サンプル。
  288 byte = 48 sample x 2ch x 3 byte がぴったり一致する。
- **実効レートは 48000 Hz ちょうど** (平均 48.000 サンプル/パケット)。
- **パケット長は 282 / 288 / 294 に揺れる** (47 / 48 / 49 サンプル)。
  sync=None の自走クロックなので、ホストの 1ms フレームとは同期していない。
  出現回数は 282 と 294 がほぼ同数で、均せば 48.000 に落ち着く。
  → **ASIO 化する際は「1ms = 48 サンプル固定」を前提にしてはいけない。**
  USB 側と ASIO バッファの間にリングバッファを挟むこと。
  なお 3 つとも 6 の倍数なので、フレーム境界は決して割れない。

- **録音バスには内蔵音源の音が乗る。** 音源を鳴らしながら取り込むとピークが
  -4 dBFS 前後まで上がる。本体側の設定だけで完結し、ベンダリクエストの
  解析は不要だった。

### 踏んだ罠: パケット記述子の Offset は転送相対

`USBD_ISO_PACKET_DESCRIPTOR.Offset` は**登録バッファ先頭からの絶対値ではなく、
その転送に渡した offset からの相対値**。どのスロットでも `0, 320, 640, ...` が返る。

```
let off = slot * slot_bytes + d.Offset as usize;   // 基点を足すこと
```

足し忘れると常にスロット 0 の領域を読み続ける。始末が悪いのは、これで

- パケット記述子の `Status` は全部 0
- パケット長も 288 で正常
- 平均サンプル数も 48.000 のまま

と、統計上はまったく正常に見えること。**波形の不連続としてしか現れない。**
転送境界 (8 パケット = 384 サンプル) の 13〜30% が飛ぶ形で発覚した。
修正後は 0.3% (音楽的な過渡音と区別がつかない水準) まで落ちた。

`SD90_DEBUG_DESC=1` を立てると最初の 6 転送分の記述子を出すので、
同種の疑いが出たらまずこれを見ること。

### 未解決点

- 録音バスに何が混ざっているか (音源出力のみか、アナログ入力も加算されるか) は
  本体のミキサー設定次第。無信号時に -75 dBFS 程度のノイズフロアと
  チャンネルごとの DC オフセットが乗るので、アナログ系が加算されていると思われる。

## MIDI (IF2)

**MIDI StartUp = USB でないと USB に出てこない。そしてその設定では背面の
MIDI IN は音源に届かない** (ホストに転送される)。つまり音源に MIDI を渡す
経路は USB しか無い。実際、EP 0x82 を読むと DIN に入れた演奏が cable 2 の
パケットとして流れてくる。

クラスコードは 0xFF だが中身は標準の USB-MIDI 1.0。Linux の
`sound/usb/quirks-table.h` の 0582:0016 の項がそう言っている:

```
QUIRK_DATA_MIDI_FIXED_ENDPOINT(2) {
        .out_cables = 0x000f,
        .in_cables  = 0x000f
}
```

cable の割り当ては実測で

| cable | 宛先 |
| --- | --- |
| 0 | PART A (内蔵音源) |
| 1 | PART B (内蔵音源) |
| 2 / 3 | 背面の MIDI OUT コネクタ |

Identity Request を投げると返事が来るので、鳴らさずに疎通確認できる:

```
送信 F0 7E 7F 06 01 F7
受信 F0 7E 10 06 02 41 48 01 00 00 00 00 00 00 F7   (41h = Roland)
```

### 踏んだ罠: 生の MIDI バイト列を書いてはいけない

エンドポイントに書けるのは **4 バイトのパケットだけ**。生のバイト列をそのまま
書くと先頭バイトが CIN として解釈され (`F0 7E 7F 06` なら cable 15 / CIN 0 の
未定義パケット)、SysEx の終端 F7 が届かないままパーサが待ち状態に入る。
**そうなると以後のノートは全部飲み込まれ、エラーも出ない。** 転送自体は
デバイスが ACK するので、WinUSB 側は成功したように見える。

戻すには Native On を送るか、本体の電源を入れ直す:

```
F0 41 10 00 48 12 00 00 00 00 00 00 F7
```

なお GM1 System On / GS リセット / Native On はどれを送っても音は出続ける
(同じ和音で -12.3 / -15.0 / -19.9 / -16.1 dBFS)。モード切り替え自体は
無音の原因にならない。

## MIDI の送出タイミング

DAW から音源を鳴らす以上、「いつ送ったか」が音楽的な精度を決める。基準にできる
のは**取り込み済みフレーム数** (`Shared::frames_captured`) だけで、ホストの
`Instant` は使えない。SD-90 の水晶とホストの時計は ppm でずれており、数分で
数十 ms たまるため。更新は 1 転送に 1 回しか来ないので、その間だけホストの
時計で補間する。位相はデバイスクロックで合わせ、間を埋めるのにホストを使う形。

### 転送の粒度がそのまま時計の粒度になる

`sd90-rs latency` は「このデバイスフレームに送る」と予約したノートが、実際に
取り込み側へ現れるまでのフレーム数 `d` を測る。取り込みの転送粒度だけを変えて
同じ測定をすると:

```
8 パケット/転送 (8ms):  d 中央値 7.33ms   ばらつき 3.60〜8.77ms   (8 回)
1 パケット/転送 (1ms):  d 中央値 3.38ms   ばらつき 3.33〜3.46ms   (16 回)
```

差の 4.0ms は 8ms 更新間隔の**平均ラグ (半分)** そのもの。時計が最後の更新から
の経過で外挿する以上、更新粒度は系統的な遅れとして丸ごと乗り、その中で
ランダムに揺れる。1ms に落とすと遅れが消えるだけでなく、ばらつきが 5.2ms から
**0.13ms** に落ちる。

つまり `d = 162 フレーム (3.38ms)` が音源そのものの応答で、8ms 構成で見えていた
残りは全部こちらの時計の粗さだった。MIDI を出す側は `StreamCfg::capture_only`
(1ms 粒度) を使うこと。転送本数は 125/s から 1000/s に増えるが、OUT 側は元から
1000/s で回しているので負荷の桁は変わらない。

ASIO ドライバは下の「ASIO ドライバ」節の実測値がその構成で取られているので、
既定は 8 のまま置いてある。

## 準備

1. 本体パネルの [SYSTEM] → [PAGE] で 5 ページ目を出し、V1 で **USB Driver** を
   **Vender** (マニュアル表記ママ) にして [ENTER]。同じ画面の 2 ページ目の
   **MIDI Start Up** も **USB** にしておく (下の MIDI 節を参照)。
   USB Driver の変更は次回電源投入から有効なので、電源を入れ直す (PID 0x0016)。
   (Owner's Manual p.69-70, p.74。背面にモードスイッチは無い)
2. [Zadig](https://zadig.akeo.ie/) で `EDIROL SD-90` に **WinUSB** を割り当てる。
   - Options → List All Devices
   - USB ID が `0582 0016` のものを選ぶ (`0017` は Generic モード側なので注意)
   - Replace Driver

Zadig を当てるまでは `list` に何も出ない。純正ドライバは起動に失敗しており
デバイスインタフェースが有効化されないため、SetupAPI から見えないのが正常。
なお純正ドライバはどのみち署名で動かないので、置き換えて失うものは無い。
戻したい場合はデバイスマネージャからドライバのロールバックで戻せる。

## ASIO ドライバ

`sd90_asio.dll` が ASIO ドライバ本体 (in-proc COM サーバ)。入出力とも 2ch。

当初は入力専用 (outputs=0) で作ったが、**SONAR (Cakewalk) は `getChannels` で
出力 0 を見た瞬間に撤収する**ことがログで判明したため、出力も実装した。
Cubase は outputs=0 でも問題なく動いていたので、ホスト依存の要求。

| 項目 | 値 |
| --- | --- |
| チャンネル | 入力 2 / 出力 2 |
| サンプル形式 | `ASIOSTInt32LSB` (24bit を 8bit 左シフトして格納) |
| サンプルレート | 48000Hz 固定 |
| バッファ | min 128 / preferred 512 / max 8192, 2 の冪のみ |
| レイテンシ申告 | 入力 bufferSize+384 / 出力 bufferSize+助走+USBキュー |
| クロック | SD-90 内蔵 (自走)。ホストではなくデバイスがマスタ |

x64 のみ。32bit のホストからは見えない。

### 登録

管理者権限のコンソールで、リポジトリのルートから:

```
regsvr32 target\release\sd90_asio.dll
regsvr32 /u target\release\sd90_asio.dll
```

DLL の**パスが登録時に記録される**ので、登録後に移動したら登録し直すこと。
`HKLM\SOFTWARE\ASIO\SD-90 (WinUSB)` と `HKCR\CLSID\{7E1A5C42-...}` に書く。

### 出力側の実装

`WinUsb_WriteIsochPipeAsap` は **パケット記述子を取らない**。WinUSB がバッファを
`wMaxPacketSize` (320B) 刻みで切ってしまうので、288B のパケットを作るには
**1 転送 = 1 パケット = 1 フレーム** にするしかない (転送は 1000 本/秒になる)。

代わりに 1ms 単位でパケット長を選べるので、ホストのフレームクロックと
デバイスの水晶の ppm 差を 47/48/49 サンプルの出し分けで吸収する。
SD-90 が入力側でこちらにやっているのと同じこと。

出力レートがデバイスのクロックに追従するのは、**入力で受けたフレーム数と同じだけ
出力に流している**から。入出力は同じ水晶なので、これだけで同期する。

#### サーボは必ず平滑値で回すこと

リングの瞬時滞留量は「ホストが bufferSize を一気に積む」「USB が 8ms ごとに
384 フレームまとめて吸う」で常時大きく上下しており、**その揺れ自体には情報が無い**。
生値で 47/49 を判断すると 6 割のフレームで長さが変わる暴れ方をして、
1 秒に 1 回ほど再生波形に振幅の飛びが出る。時定数 1 秒の EMA を噛ませると
調整は 5% 程度に落ち着き、飛びは消える。

実測 (440Hz -12dBFS を出し、SD-90 のモニタ経由で入力に戻して測定):

```
生値サーボ : servo47=3367 servo49=3056 / 10s   振幅の飛び 6 窓
EMA サーボ : servo47= 470 servo49= 225 / 15s   振幅の飛び 0 窓
              underrun=0 overrun=0
              包絡の中央値 0.2499 (送出値と一致), 基本波 440.4Hz,
              ゼロ交差間隔 109.09±0.29 (理論 109.09)
```

### ASIO の癖

* ホストは `CoCreateInstance(clsid, .., **clsid**, ..)` と、**IID に CLSID を渡す**。
  QueryInterface は CLSID も受け付けないと生成できない。
* vtable は IUnknown の 3 メソッド + ASIO の 20 メソッドが並ぶだけ。
  x64 は呼び出し規約が 1 つなので `extern "system"` で一致する
  (x86 の `__thiscall` 問題は考えなくてよい)。
* Windows の ASIO SDK は `NATIVE_INT64=0` なので `ASIOSamples` /
  `ASIOTimeStamp` は `{hi: u32, lo: u32}` の 2 ワード。u64 をそのまま
  書くと上下が入れ替わる。

## 使い方

```
sd90-rs list                     WinUSB から見える SD-90 を探す
sd90-rs desc                     デバイス/コンフィグ記述子をダンプ
sd90-rs capture [ms] [out.raw]   IF1 / EP 0x81 から iso IN を取り込む
sd90-rs asiotest [ms]            ASIO ドライバをレジストリ登録なしで検証
sd90-rs latency [回数] [ch] [note]  音源の応答 d を実測する
```

`latency` は既定で ch 10 (ドラム) の note 38 を 8 回鳴らし、送出フレームと
オンセットの差を並べる。プラグイン ([`sd90-vst`](../sd90-vst)) の申告レイテンシ `L = B + d` の `d` はこれで決める。
立ち上がりの遅い音色を指定するとアタックぶんが `d` に混ざるので、打楽器で測ること。

`asiotest` は DllGetClassObject → クラスファクトリ → IASIO の全経路を、
管理者権限も DAW も無しで叩く。vtable はホスト側の宣言を自前で書き下ろして
いるので、ドライバ側の定義とずれていれば即座に壊れる。

* `SD90_TONE=1` … 出力に 440Hz -12dBFS を流し、入力を `asiotest_in.i32`
  (i32 LE インタリーブ) に記録する。SD-90 のモニタ経由で戻ってくるので、
  出力の正しさを入力側で測定できる。
* `SD90_REC=1` … トーンを出さずに入力だけ記録する (切り分け用)。

`capture` は生のパケットペイロードをそのまま連結して書き出す。あわせて

- パケット長のヒストグラム (288 に揃うか)
- 取れたバイト数から逆算した実効レート (48000 付近に出るか)
- 24bit LE / 2ch と仮定したピークレベル (無音かどうか)
- 先頭パケットの hex

を出す。上の「実測で確定したこと」はこれで調べた。

## 構成

| ファイル | 役割 |
| --- | --- |
| `src/device.rs` | SetupAPI でのデバイス探索、WinUSB オープン、IF0/1/2 のハンドル |
| `src/descriptor.rs` | 記述子の取得と UAC1 として解釈した表示 |
| `src/iso.rs` | プローブ用の一括取り込みと統計 (`capture_with` で外から時計を読める) |
| `src/usbmidi.rs` | IF2 の bulk。USB-MIDI 4 バイトパケットの組み立てと送受信 |
| `src/midisched.rs` | デバイスフレーム基準の MIDI 送出。時計とキューと送出スレッド |
| `src/latency.rs` | 音源の応答 `d` の実測 |
| `src/ring.rs` | USB 側とASIO 側をつなぐ SPSC リングバッファ |
| `src/stream.rs` | iso IN / OUT を回し続けるワーカ |
| `src/asio.rs` | IASIO 実装と RT スレッド |
| `src/comserver.rs` | クラスファクトリ、DLL エクスポート、自己登録 |
| `src/asiohost.rs` | `asiotest` 用の最小 ASIO ホスト (bin 側のみ) |
| `src/log.rs` | 診断ログ (既定は無効) |

DAW の中では stdout が使えないので、診断は `%TEMP%\sd90_asio.log` に出す。
**空のファイルを自分で作ると有効になり、無ければ何もしない。**
ホストがどこまで呼んで諦めたかを見るのに使う (SONAR の件はこれで判明した)。

iso 転送は 8 スロット × 8 パケット (= 64ms 先まで) 積んで回す。
USB スレッドと ASIO の RT スレッドはどちらも MMCSS の "Pro Audio" を取る。

データの流れ:

```
USB スレッド                  リング                 RT スレッド              ホスト
iso IN 8ms 分を回収 ---> 24bit 詰めのまま ---> bufferSize 溜まったら ---> bufferSwitch()
(連結以外の仕事をしない)   (16384 フレーム)      deinterleave + <<8         |
                                                                            | 出力を埋める
iso OUT 1ms ずつ送出 <--- 24bit 詰めのまま <--- 同じフレーム数を畳む <-------+
(EMA サーボで 47/48/49)   (助走 = bufferSize x2,  >>8 して interleave
                           最低 1024)
```

パケット長が揺れる以上、USB の到着単位を ASIO のバッファサイズに直接
対応させることはできない。リングで両者を切り離すのが要。

## 商標

ASIO is a registered trademark of Steinberg Media Technologies GmbH.
Roland and EDIROL are trademarks of Roland Corporation.

非公式のプロジェクトで、Steinberg とも Roland とも関係は無い。
