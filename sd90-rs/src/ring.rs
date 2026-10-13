//! USB 側 (単一プロデューサ) と ASIO コールバック側 (単一コンシューマ) を
//! つなぐリングバッファ。
//!
//! SD-90 のパケット長は 282/288/294 と揺れる (sync=None の自走クロック) ため、
//! USB の到着単位を ASIO のバッファサイズにそのまま対応させることはできない。
//! ここで両者を切り離す。

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Ring {
    buf: UnsafeCell<Box<[u8]>>,
    /// 容量 (フレーム数)。2 の冪であること。
    frames: usize,
    frame_bytes: usize,
    /// 単調増加のフレームカウンタ。差分が滞留量。
    write: AtomicUsize,
    read: AtomicUsize,
    overruns: AtomicUsize,
    underruns: AtomicUsize,
}

// 生ポインタを持つが、プロデューサ 1 / コンシューマ 1 の規約下でのみ使う。
unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

impl Ring {
    pub fn new(frames: usize, frame_bytes: usize) -> Self {
        assert!(frames.is_power_of_two(), "容量は 2 の冪であること");
        Self {
            buf: UnsafeCell::new(vec![0u8; frames * frame_bytes].into_boxed_slice()),
            frames,
            frame_bytes,
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            overruns: AtomicUsize::new(0),
            underruns: AtomicUsize::new(0),
        }
    }

    pub fn available(&self) -> usize {
        self.write
            .load(Ordering::Acquire)
            .wrapping_sub(self.read.load(Ordering::Relaxed))
    }

    pub fn overruns(&self) -> usize {
        self.overruns.load(Ordering::Relaxed)
    }

    pub fn underruns(&self) -> usize {
        self.underruns.load(Ordering::Relaxed)
    }

    pub fn note_underrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
    }

    /// プロデューサ (USB スレッド) 専用。`data` はフレーム境界に揃っていること。
    ///
    /// 空きが足りない場合は入ってきた分を捨てる。読み側の index には触らない
    /// (SPSC を崩さないため)。捨てるのは ASIO 側が止まっている異常時だけ。
    pub fn push(&self, data: &[u8]) {
        let n = data.len() / self.frame_bytes;
        if n == 0 {
            return;
        }
        let w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        if w.wrapping_sub(r) + n > self.frames {
            self.overruns.fetch_add(1, Ordering::Relaxed);
            return;
        }

        let buf = unsafe { &mut *self.buf.get() };
        let mask = self.frames - 1;
        let head = (w & mask) * self.frame_bytes;
        let to_end = (self.frames - (w & mask)).min(n) * self.frame_bytes;
        buf[head..head + to_end].copy_from_slice(&data[..to_end]);
        let total = n * self.frame_bytes;
        if to_end < total {
            buf[..total - to_end].copy_from_slice(&data[to_end..total]);
        }
        self.write.store(w.wrapping_add(n), Ordering::Release);
    }

    /// コンシューマ (ASIO スレッド) 専用。`n` フレーム分を 1〜2 個の連続スライス
    /// として渡す (リングの折り返しがあるため 2 回呼ばれうる)。
    /// 溜まっていなければ何もせず false。
    pub fn consume<F: FnMut(&[u8])>(&self, n: usize, mut f: F) -> bool {
        if self.available() < n {
            return false;
        }
        let r = self.read.load(Ordering::Relaxed);
        let buf = unsafe { &*self.buf.get() };
        let mask = self.frames - 1;
        let head = (r & mask) * self.frame_bytes;
        let to_end = (self.frames - (r & mask)).min(n) * self.frame_bytes;
        f(&buf[head..head + to_end]);
        let total = n * self.frame_bytes;
        if to_end < total {
            f(&buf[..total - to_end]);
        }
        self.read.store(r.wrapping_add(n), Ordering::Release);
        true
    }
}
