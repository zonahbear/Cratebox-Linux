//! Lock-free single-producer/single-consumer ring of stereo frames, for feeding a second output
//! device (headphones) from the main audio callback. If the two devices' clocks drift, the reader
//! drops or repeats audio at the edges instead of blocking.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Inner {
    buf: Box<[UnsafeCell<f32>]>,
    cap: usize,
    head: AtomicUsize, // written by the producer
    tail: AtomicUsize, // written by the consumer
}
unsafe impl Sync for Inner {}
unsafe impl Send for Inner {}

pub struct Producer(Arc<Inner>);
pub struct Consumer(Arc<Inner>, bool);

pub fn ring(frames: usize) -> (Producer, Consumer) {
    let cap = (frames * 2).next_power_of_two();
    let buf: Vec<UnsafeCell<f32>> = (0..cap).map(|_| UnsafeCell::new(0.0)).collect();
    let inner = Arc::new(Inner { buf: buf.into_boxed_slice(), cap, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) });
    (Producer(inner.clone()), Consumer(inner, false))
}

impl Producer {
    pub fn push2(&mut self, l: f32, r: f32) {
        let i = &self.0;
        let h = i.head.load(Ordering::Relaxed);
        let t = i.tail.load(Ordering::Acquire);
        if h.wrapping_sub(t) + 2 > i.cap { return; } // full: drop (the reader is behind)
        unsafe {
            *i.buf[h & (i.cap - 1)].get() = l;
            *i.buf[(h + 1) & (i.cap - 1)].get() = r;
        }
        i.head.store(h.wrapping_add(2), Ordering::Release);
    }
}

impl Consumer {
    /// fill interleaved stereo; waits for a little audio to build up before starting
    pub fn pull(&mut self, out: &mut [f32], channels: usize) {
        let i = &self.0;
        let frames = out.len() / channels;
        let h = i.head.load(Ordering::Acquire);
        let mut t = i.tail.load(Ordering::Relaxed);
        let avail = h.wrapping_sub(t) / 2;
        if !self.1 && avail < frames * 2 { out.fill(0.0); return; }
        self.1 = true;
        // too far behind (clock drift): skip ahead to keep latency low
        if avail > frames * 8 { t = h.wrapping_sub(frames * 4); }
        for k in 0..frames {
            let (l, r) = if h.wrapping_sub(t) >= 2 {
                let v = unsafe { (*i.buf[t & (i.cap - 1)].get(), *i.buf[(t + 1) & (i.cap - 1)].get()) };
                t = t.wrapping_add(2);
                v
            } else { (0.0, 0.0) };
            let f = &mut out[k * channels..(k + 1) * channels];
            f.fill(0.0);
            f[0] = l;
            if channels > 1 { f[1] = r; }
        }
        i.tail.store(t, Ordering::Release);
    }
}
