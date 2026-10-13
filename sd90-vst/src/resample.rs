//! リングの 24bit を f32 に開きながら、ホストのクロックとデバイスの水晶の
//! ずれを吸収して読み出す。
//!
//! ホストが 48kHz でも、それは**ホストの水晶の 48kHz** であって SD-90 の
//! 48kHz ではない。ppm 単位のずれを放っておくと滞留量が片方向に育ち、
//! いずれ溢れるか枯れる。そこで読み出し位置を小数で持ち、滞留量を見ながら
//! 比を公称比の周りで微調整する。
//!
//! ホストが 48kHz 以外 (44.1k / 96k / 192k など) のときは、公称比
//! `48000 / ホストのレート` (ホスト 1 フレームあたりのデバイスフレーム数) を
//! 中心に回す。水位・目標はすべて**デバイスフレーム**で数える。
//!
//! 補間は 4 点 Catmull-Rom (Hermite)。アップサンプル (96k / 192k) と 48k 同士は
//! これで足りる。ダウンサンプル (44.1k) は前段のローパスを持たないので、
//! 22.05k〜24k の成分が 20.1k〜22.05k に折り返す。SD-90 の出力はそこに
//! ほとんど成分を持たないので割り切っている。
//!
//! サーボを生の滞留量で回してはいけないのは stream.rs の OUT 側と同じ理由。
//! 瞬時値は「ホストが 1 ブロックまとめて吸う」「USB が 1ms ごとに 48 フレーム
//! 積む」で常に上下していて、その揺れ自体には情報が無い。拾いたいのは
//! ppm 単位のクロック差だけなので、必ず平滑してから判断する。

use sd90_rs::device::{BYTES_PER_SAMPLE, CHANNELS, FRAME_BYTES};
use sd90_rs::ring::Ring;

/// 滞留量の平滑化係数。1 ブロック (5ms 前後) ごとに呼ばれるので、
/// 0.01 で時定数はおよそ 0.5 秒。
const LEVEL_ALPHA: f64 = 0.01;
/// 比の調整幅。水晶のずれは高々 ±100ppm 程度なので、その 20 倍見ておけば
/// 起動直後の水位ずれも含めて吸収できる。これ以上振ると可聴になる。
const MAX_TRIM: f64 = 0.002;
/// 目標水位からのずれを比に変換する係数。滞留量の 1% のずれを
/// 比の 0.01 * P で返す形。強すぎると振動するので弱めに置く。
const SERVO_P: f64 = 0.05;
/// 目標水位 + 1 ブロックをこれ以上超えたら、サーボに任せず助走からやり直す。20ms。
///
/// ホストが `process` をしばらく呼ばなかった (エンジンの再起動、設定変更、
/// 重い処理での引っかかり) 間も USB 側は積み続ける。サーボの引き戻しは
/// 高々 0.2% なので、0.3 秒溜まれば戻るまでに 150 秒かかる。そのあいだ
/// 実際の遅れが申告値より大きく (= 発音タイミングが遅れ)、リングが満杯に
/// 張り付いて溢れた転送ごとに波形が欠ける。
const OVERFULL_SLACK_FRAMES: usize = 960;

fn sample_i24_le(b: &[u8]) -> i32 {
    let v = (b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16);
    (v << 8) >> 8
}

/// 24bit フルスケール。
const SCALE: f32 = 1.0 / 8_388_608.0;

/// 4 点 Catmull-Rom。`t` は y1 と y2 の間の位相 0..1。
fn hermite(y: [f32; 4], t: f32) -> f32 {
    let c0 = y[1];
    let c1 = 0.5 * (y[2] - y[0]);
    let c2 = y[0] - 2.5 * y[1] + 2.0 * y[2] - 0.5 * y[3];
    let c3 = 0.5 * (y[3] - y[0]) + 1.5 * (y[1] - y[2]);
    ((c3 * t + c2) * t + c1) * t + c0
}

pub struct Stats {
    /// 出力できずに無音を返したブロック数。
    pub underruns: usize,
    /// 溜まりすぎで助走からやり直した回数。
    pub overfulls: usize,
    /// 直近の滞留量の平滑値 (フレーム)。
    pub level: f64,
    /// 直近の読み出し比。
    pub ratio: f64,
    /// これまでにリングから吸い出したフレーム数。
    pub consumed: u64,
}

pub struct Reader {
    /// 補間に使う直近 4 フレーム。`hist[1]` と `hist[2]` の間を出力する。
    hist: [[f32; CHANNELS]; 4],
    /// `hist[1]` からの位相 0..1。
    phase: f64,
    ratio: f64,
    level: f64,
    /// 目標滞留量 (フレーム)。申告レイテンシの B。
    target: usize,
    /// ホストの最大ブロック長をデバイスフレームに直したもの。溜まりすぎの判定に使う。
    max_block: usize,
    /// 公称比。ホスト 1 フレームあたりのデバイスフレーム数 (= 48000 / ホストのレート)。
    nominal: f64,
    /// 助走が済んだか。済むまでは無音を返す。
    primed: bool,
    underruns: usize,
    /// 溜まりすぎで助走からやり直した回数。
    overfulls: usize,
    consumed: u64,
    /// リングから引く 1 回分の置き場。ブロック長 + 補間の余白で足りる。
    scratch: Vec<u8>,
}

impl Reader {
    /// `target` はデバイスフレーム、`max_block` はホストのフレーム。
    /// `nominal` は `48000 / ホストのレート`。
    pub fn new(target: usize, max_block: usize, nominal: f64) -> Self {
        // 1 ブロックで吸い出す最大量。比は公称比の高々 +0.2%。
        let dev_block = (max_block as f64 * nominal * (1.0 + MAX_TRIM)).ceil() as usize;
        Self {
            hist: [[0.0; CHANNELS]; 4],
            phase: 0.0,
            ratio: nominal,
            level: target as f64,
            target,
            max_block: dev_block,
            nominal,
            primed: false,
            underruns: 0,
            overfulls: 0,
            consumed: 0,
            // 比は高々公称比の 1.002 倍なので、それで換算したブロック長 + 少しで足りる。
            scratch: vec![0u8; (dev_block + 16) * FRAME_BYTES],
        }
    }

    pub fn stats(&self) -> Stats {
        Stats {
            underruns: self.underruns,
            overfulls: self.overfulls,
            level: self.level,
            ratio: self.ratio,
            consumed: self.consumed,
        }
    }

    /// 目標水位を変える。申告レイテンシも一緒に変わるので、ホストには
    /// `restartComponent(kLatencyChanged)` を投げること。
    pub fn set_target(&mut self, target: usize) {
        self.target = target;
        self.primed = false;
    }

    /// 助走からやり直す。ホストが処理を止めていた後に呼ぶ。
    pub fn reset(&mut self) {
        self.primed = false;
    }

    /// `out` は ch ごとの出力バッファ (VST3 は非インタリーブ)。
    /// 長さはすべて等しいこと。戻り値は音を書いたか (false = 無音で埋めた)。
    pub fn pull(&mut self, ring: &Ring, out: &mut [&mut [f32]]) -> bool {
        let n = out.first().map_or(0, |b| b.len());
        if n == 0 {
            return true;
        }
        let mut avail = ring.available();

        // 溜まりすぎ。ホストが止まっていた分はサーボでは戻しきれないので捨てる。
        if self.primed && avail > self.target + self.max_block + OVERFULL_SLACK_FRAMES {
            self.overfulls += 1;
            self.primed = false;
        }

        // 助走。規定量が溜まるまでは吸い出さない。
        if !self.primed {
            if avail < self.target {
                self.level = avail as f64;
                silence(out);
                return false;
            }
            // 溜まりすぎている分はここで捨てて、水位を目標ちょうどから始める。
            //
            // サーボの引き戻しは高々 0.2%/s なので、目標から数百フレーム
            // ずれた状態から始めると数秒間ずれたままになる。そのあいだ
            // **申告レイテンシが実際の遅れと合わない** — ホストの PDC が
            // その分そのままずれる。音の連続性より申告値の正しさを取る。
            // どのみち助走に入るのは起動直後かアンダーラン直後で、
            // すでに波形は切れている。
            if avail > self.target {
                ring.consume(avail - self.target, |_| {});
                avail = self.target;
            }
            self.level = self.target as f64;
            self.primed = true;
            // 補間には出力位置の前後 2 点ずつ要る。位相を 3 から始めると、
            // 最初の 1 サンプルを出す前に hist[1..4] が実データで埋まる。
            self.phase = 3.0;
            self.hist = [[0.0; CHANNELS]; 4];
        } else {
            self.level += (avail as f64 - self.level) * LEVEL_ALPHA;
        }

        // 平滑した水位が目標からずれているぶんだけ、比を 1.0 から動かす。
        // 溜まりすぎなら速く読む (比 > 1)、枯れかけなら遅く読む。
        let err = (self.level - self.target as f64) / self.target.max(1) as f64;
        self.ratio = self.nominal * (1.0 + (err * SERVO_P).clamp(-MAX_TRIM, MAX_TRIM));

        // このブロックで読み込む入力フレーム数。最後のサンプルを出すまでに
        // hist をずらす回数そのもの = floor(最終サンプルの絶対位置)。
        //
        // ここを floor(phase + ratio * n) にしてはいけない。1 フレーム多く
        // 吸い出して使わないまま捨てることになり、ブロックごとに 1 サンプル
        // 飛ぶ。サーボは比を緩めて水位だけ辻褄を合わせるので、統計上は
        // 正常に見えたまま波形にだけ不連続が出る。
        let need = (self.phase + self.ratio * (n - 1) as f64).floor() as usize;
        if need * FRAME_BYTES > self.scratch.len() {
            // 比の暴走。ここに来るなら設計が壊れている。
            silence(out);
            self.underruns += 1;
            self.primed = false;
            return false;
        }
        if avail < need {
            // 底を打った。無音で埋めて助走からやり直す。
            silence(out);
            self.underruns += 1;
            self.primed = false;
            return false;
        }

        let mut off = 0usize;
        ring.consume(need, |part| {
            self.scratch[off..off + part.len()].copy_from_slice(part);
            off += part.len();
        });
        self.consumed += need as u64;

        // 出力を 1 サンプルずつ組み立てる。位相が 1 を超えるたびに hist を
        // ずらして新しいフレームを 1 つ読み込む。
        let mut src = 0usize;
        for i in 0..n {
            while self.phase >= 1.0 {
                self.hist.rotate_left(1);
                let base = src * FRAME_BYTES;
                for ch in 0..CHANNELS {
                    self.hist[3][ch] =
                        sample_i24_le(&self.scratch[base + ch * BYTES_PER_SAMPLE..]) as f32 * SCALE;
                }
                src += 1;
                self.phase -= 1.0;
            }
            let t = self.phase as f32;
            for (ch, buf) in out.iter_mut().enumerate() {
                let c = ch.min(CHANNELS - 1);
                buf[i] = hermite(
                    [
                        self.hist[0][c],
                        self.hist[1][c],
                        self.hist[2][c],
                        self.hist[3][c],
                    ],
                    t,
                );
            }
            self.phase += self.ratio;
        }
        // 吸い出した分はすべて hist を通っていること。端数の位相は次に持ち越す。
        debug_assert_eq!(src, need);
        true
    }
}

fn silence(out: &mut [&mut [f32]]) {
    for b in out.iter_mut() {
        b.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// USB 側 1ms ごとに 48 フレーム、ホスト側 10ms ごとに 480 フレームで回し、
    /// 途中で `process` が `stall_ms` だけ呼ばれない時間を作る。
    /// 止まった後 `after_ms` のあいだに増えたリング溢れの回数を返す。
    fn run_with_stall(stall_ms: usize, after_ms: usize) -> (usize, usize) {
        const BLOCK: usize = 480;
        let ring = Ring::new(16384, FRAME_BYTES);
        let target = BLOCK + 96;
        let mut r = Reader::new(target, BLOCK, 1.0);
        let pkt = vec![0u8; 48 * FRAME_BYTES];
        let mut l = vec![0f32; BLOCK];
        let mut rr = vec![0f32; BLOCK];
        let warm = 2000;
        let mut over_at_resume = 0;
        for ms in 0..warm + stall_ms + after_ms {
            ring.push(&pkt);
            let stalled = ms >= warm && ms < warm + stall_ms;
            // 再開後の最初の pull が済んでから数える。
            if ms == warm + stall_ms + 10 {
                over_at_resume = ring.overruns();
            }
            if !stalled && ms % 10 == 9 {
                let mut bufs: [&mut [f32]; 2] = [&mut l, &mut rr];
                r.pull(&ring, &mut bufs);
            }
        }
        (ring.overruns() - over_at_resume, ring.available())
    }

    /// SD-90 側に 1kHz の正弦波を 48kHz で積み、ホストは `host_rate` で
    /// 10ms ごとに 1 ブロック吸う。最後の 1 秒の出力の上向きゼロ交差を数えて
    /// 周波数を返す。あわせて (リング溢れ, 底打ち, 溜まりすぎ, 平滑水位)。
    fn run_at_rate(host_rate: usize) -> (f64, usize, usize, usize, f64) {
        let block = host_rate / 100;
        let nominal = 48_000.0 / host_rate as f64;
        let dev_block = (block as f64 * nominal).ceil() as usize;
        let target = dev_block + 96;
        let ring = Ring::new(16384, FRAME_BYTES);
        let mut r = Reader::new(target, block, nominal);
        let mut l = vec![0f32; block];
        let mut rr = vec![0f32; block];
        let mut pkt = vec![0u8; 48 * FRAME_BYTES];
        let mut t = 0u64;
        let secs = 10;
        let mut crossings = 0usize;
        let mut prev = 0f32;
        for ms in 0..secs * 1000 {
            for f in 0..48 {
                let v = ((t as f64 * 2.0 * std::f64::consts::PI * 1000.0 / 48_000.0).sin() * 4_000_000.0) as i32;
                for ch in 0..CHANNELS {
                    let o = f * FRAME_BYTES + ch * BYTES_PER_SAMPLE;
                    pkt[o..o + 3].copy_from_slice(&v.to_le_bytes()[..3]);
                }
                t += 1;
            }
            ring.push(&pkt);
            if ms % 10 == 9 {
                let mut bufs: [&mut [f32]; 2] = [&mut l, &mut rr];
                r.pull(&ring, &mut bufs);
                if ms >= (secs - 1) * 1000 {
                    for &x in l.iter() {
                        if prev < 0.0 && x >= 0.0 {
                            crossings += 1;
                        }
                        prev = x;
                    }
                }
            }
        }
        let st = r.stats();
        (crossings as f64, ring.overruns(), st.underruns, st.overfulls, st.level - target as f64)
    }

    #[test]
    fn runs_at_other_host_rates() {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let (hz, over, under, overfull, dev) = run_at_rate(rate);
            assert!((hz - 1000.0).abs() <= 2.0, "{rate}: ピッチがずれる ({hz} Hz)");
            assert_eq!((over, under, overfull), (0, 0, 0), "{rate}: 溢れ / 底打ち / 溜まりすぎ");
            assert!(dev.abs() < 100.0, "{rate}: 水位が目標から外れる ({dev:+.0})");
        }
    }

    #[test]
    fn recovers_after_host_stall() {
        // 0.5 秒止まったあと 10 秒回す。溢れが止まり、水位が目標付近に戻ること。
        let (over, avail) = run_with_stall(500, 10_000);
        assert!(over == 0, "再開後もリング溢れが続く: {over} 回 / 水位 {avail}");
        assert!(avail < 480 + 96 + 480, "水位が戻らない: {avail}");
    }
}
