//! プラグイン本体とエディタの間で共有するもの。
//!
//! エディタは UI スレッド、`process` は RT スレッドで動く。本体に送る
//! メッセージは**必ず `process` から**積む。`MidiSched` は単一生産者の
//! リングで、しかも「フレームの昇順に積まれる」前提で送出スレッドが
//! 先頭から待つので、UI スレッドが勝手に積むと DAW のノートを追い越したり
//! 塞いだりする。
//!
//! なのでエディタは `outbox` に置くだけにし、`process` が毎ブロック
//! `try_lock` で少しずつ取り出して、そのブロックの先頭フレームに積む。
//! 取り出す量は MIDI の DIN 速度相当に絞る (下の `Pacer`)。SysEx を USB の
//! 速度で一気に流し込むと、音源側の受信処理が追いつかずに取りこぼす。

use crate::engine::Engine;
use crate::sd90::{Change, Model, Out};
use sd90_rs::device::SAMPLE_RATE;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, Weak};

/// 1 バイトあたりのフレーム数。31250 bps / 10 bit = 3125 B/s。
const FRAMES_PER_BYTE: u32 = SAMPLE_RATE / 3125;
/// 貯められる送信枠の上限。この分までは一気に出してよい。
const MAX_CREDIT_BYTES: i64 = 256;

pub struct Control {
    pub model: Mutex<Model>,
    outbox: Mutex<VecDeque<Out>>,
    /// 直近の出力ピーク (f32 のビット列)。エディタが読んで 0 に戻す。
    peak: [AtomicU32; 2],
    /// 本体を掴めているか。
    pub online: AtomicBool,
    /// 掴めなかった理由など。
    pub status: Mutex<String>,
    /// 診断表示用。プラグインが持っている間だけ upgrade できる。
    pub engine: Mutex<Weak<Engine>>,
    /// 申告中のレイテンシ (フレーム)。
    pub latency: AtomicU32,
    /// 状態が読み込まれた / 掴み直したので一式を送り直す必要がある。
    pub resend: AtomicBool,
    /// エディタの外 (setState や DAW からの CC) でモデルが変わった。
    pub model_rev: AtomicU32,
    /// 診断表示用。読み出しが底を打った / 溜まりすぎて助走からやり直した回数。
    pub reader_underruns: AtomicU32,
    pub reader_overfulls: AtomicU32,
    /// 診断表示用。ホストが `process` で要求したフレームの累計。
    /// デバイスの取り込み量と比べれば、ホストの実際のレートがわかる。
    pub host_frames: AtomicU64,
    /// ホストが `setupProcessing` で伝えてきたサンプリングレート。
    pub host_rate: AtomicU32,
    /// 診断表示用。読み出し比 (f32 のビット列) と平滑した水位 (フレーム)。
    pub reader_ratio: AtomicU32,
    pub reader_level: AtomicU32,
}

impl Default for Control {
    fn default() -> Self {
        Self {
            model: Mutex::new(Model::default()),
            outbox: Mutex::new(VecDeque::new()),
            peak: [AtomicU32::new(0), AtomicU32::new(0)],
            online: AtomicBool::new(false),
            status: Mutex::new(String::new()),
            engine: Mutex::new(Weak::new()),
            latency: AtomicU32::new(0),
            resend: AtomicBool::new(false),
            model_rev: AtomicU32::new(0),
            reader_underruns: AtomicU32::new(0),
            reader_overfulls: AtomicU32::new(0),
            host_frames: AtomicU64::new(0),
            host_rate: AtomicU32::new(SAMPLE_RATE),
            reader_ratio: AtomicU32::new(1f32.to_bits()),
            reader_level: AtomicU32::new(0),
        }
    }
}

impl Control {
    /// エディタから。変更 1 回ぶんを送る。
    pub fn send_change(&self, model: &Model, change: Change) {
        self.post(model.messages(change));
    }

    pub fn post(&self, msgs: impl IntoIterator<Item = Out>) {
        if let Ok(mut q) = self.outbox.lock() {
            q.extend(msgs);
        }
    }

    /// 今の状態一式を送り直す。送りかけのものは捨てる (どうせ上書きされる)。
    pub fn send_all(&self) {
        let dump = match self.model.lock() {
            Ok(m) => m.dump(),
            Err(_) => return,
        };
        if let Ok(mut q) = self.outbox.lock() {
            q.clear();
            q.extend(dump);
        }
    }

    pub fn pending(&self) -> usize {
        self.outbox.lock().map(|q| q.len()).unwrap_or(0)
    }

    /// RT スレッドから。出力ピークを記録する。
    pub fn note_peak(&self, ch: usize, v: f32) {
        let a = &self.peak[ch.min(1)];
        if v > f32::from_bits(a.load(Relaxed)) {
            a.store(v.to_bits(), Relaxed);
        }
    }

    /// エディタから。前回読んでからのピークを取り出す。
    pub fn take_peak(&self) -> [f32; 2] {
        [
            f32::from_bits(self.peak[0].swap(0, Relaxed)),
            f32::from_bits(self.peak[1].swap(0, Relaxed)),
        ]
    }

    /// RT スレッドから。送ってよい分だけ取り出して `emit` に渡す。
    /// ロックが取れなければ今回は何もしない (次のブロックでやる)。
    pub fn drain(&self, pacer: &mut Pacer, frames: usize, mut emit: impl FnMut(u8, &[u8])) -> bool {
        pacer.credit = (pacer.credit + (frames as u32 / FRAMES_PER_BYTE.max(1)) as i64)
            .min(MAX_CREDIT_BYTES);
        if pacer.credit <= 0 {
            return false;
        }
        let Ok(mut q) = self.outbox.try_lock() else {
            return false;
        };
        let mut sent = false;
        while pacer.credit > 0 {
            let Some(o) = q.pop_front() else { break };
            match o {
                Out::Wait(ms) => {
                    pacer.credit -= ms as i64 * 3125 / 1000;
                }
                Out::Msg { cable, .. } => {
                    let b = o.bytes();
                    pacer.credit -= b.len() as i64;
                    emit(cable, b);
                    sent = true;
                }
            }
        }
        sent
    }
}

/// 送信枠。RT スレッド側の持ち物。
#[derive(Default)]
pub struct Pacer {
    credit: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(n: u8) -> Out {
        // 13 バイト = DT1 1 個ぶん。
        let mut b = [0u8; 13];
        b[0] = n;
        Out::msg(0, &b)
    }

    #[test]
    fn drain_keeps_order_and_din_rate() {
        let c = Control::default();
        c.post((0..10).map(msg));
        let mut pacer = Pacer::default();
        let mut got = Vec::new();
        // 512 フレーム = 34 バイトぶんの枠。13 バイトを 3 個 (最後の 1 個ははみ出してよい)。
        c.drain(&mut pacer, 512, |_, b| got.push(b[0]));
        assert_eq!(got, vec![0, 1, 2]);
        // 1 秒ぶん回せば全部出る (3125 B/s)。順番は崩れない。
        for _ in 0..94 {
            c.drain(&mut pacer, 512, |_, b| got.push(b[0]));
        }
        assert_eq!(got, (0..10).collect::<Vec<_>>());
        assert_eq!(c.pending(), 0);
    }

    #[test]
    fn wait_holds_back_what_follows() {
        let c = Control::default();
        c.post([msg(1), Out::Wait(300), msg(2)]);
        let mut pacer = Pacer::default();
        let mut got = Vec::new();
        let mut frames = 0usize;
        while got.len() < 2 && frames < 48_000 {
            c.drain(&mut pacer, 256, |_, b| got.push((b[0], frames)));
            frames += 256;
        }
        assert_eq!(got[0].0, 1);
        // 300ms 待ってから 2 個目。
        let gap = got[1].1 - got[0].1;
        assert!((14_000..=16_000).contains(&gap), "gap = {gap}");
    }
}
