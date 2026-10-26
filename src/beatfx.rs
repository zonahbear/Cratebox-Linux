//! Beat FX / Pad FX state on the UI thread (the effects themselves run in the engine's FX units). The
//! BEAT FX unit: an effect, a BEAT per effect, LEVEL/DEPTH, ON and the channel it's on (a deck, or the
//! master). PAD FX: hold a pad = that effect on that deck, let go = off. Port of the iOS FxController.
//! Each unit in use gets the beat clock of its deck (the master's: the deck the mix follows) every
//! 100 ms, so echoes and rolls stay on the grid.

use crate::engine::{Cmd, MASTER_FX};
use crate::fx::{spec, FxClock, FxKind, SlotSet, FX_LIST, PAD_FX};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

pub struct BeatFx {
    tx: Sender<Cmd>,
    pub fx: FxKind,
    pub beats: HashMap<FxKind, f64>,
    pub depth: f64,
    pub on: bool,
    /// 0-3 = deck channels, 4 = master
    pub ch: usize,
    /// per deck: the PAD FX pad held (pad, bank)
    pub pad: [Option<(usize, usize)>; 4],
    /// HEAT: a Puffco is connected, so PUFF is on offer
    pub puff_ready: bool,
    last_active: HashMap<usize, Instant>,
    last_clock: Instant,
    /// settings changed (the app saves them)
    pub dirty: bool,
}

impl BeatFx {
    pub fn new(tx: Sender<Cmd>, fx: &str, depth: f64, ch: usize, saved: &HashMap<String, f64>) -> BeatFx {
        let mut beats: HashMap<FxKind, f64> = FX_LIST.iter().map(|(k, s)| (*k, s.beat)).collect();
        for (k, v) in saved { if let Some(f) = FxKind::from_name(k) { if *v > 0.0 { beats.insert(f, *v); } } }
        let fx = FxKind::from_name(fx).filter(|f| *f != FxKind::Puff).unwrap_or(FxKind::Echo);
        BeatFx { tx, fx, beats, depth: depth.clamp(0.0, 1.0), on: false, ch: ch.min(MASTER_FX), pad: [None; 4], puff_ready: false,
            last_active: HashMap::new(), last_clock: Instant::now(), dirty: false }
    }
    /// the effects on offer right now (PUFF only with a Puffco)
    pub fn kinds(&self) -> Vec<FxKind> { FX_LIST.iter().map(|x| x.0).filter(|k| *k != FxKind::Puff || self.puff_ready).collect() }
    pub fn beat(&self) -> f64 { self.beats.get(&self.fx).copied().unwrap_or(spec(self.fx).beat) }
    pub fn saved_beats(&self) -> HashMap<String, f64> { self.beats.iter().map(|(k, v)| (k.name().to_string(), *v)).collect() }

    fn post(&self, unit: usize, pad: bool, set: SlotSet) { let _ = self.tx.send(Cmd::Fx { unit, pad, set }); }
    fn send_clock(&self, u: usize, clock: &dyn Fn(usize) -> FxClock) { let _ = self.tx.send(Cmd::Clock { unit: u, clock: clock(u) }); }
    fn send(&mut self, clock: &dyn Fn(usize) -> FxClock) {
        if self.on { self.last_active.insert(self.ch, Instant::now()); self.send_clock(self.ch, clock); }
        self.post(self.ch, false, SlotSet { fx: Some(self.fx), beats: Some(self.beat()), depth: Some(self.depth), on: Some(self.on) });
    }
    /// every frame: refresh the clocks of the units in use (and for 15 s after, while tails ring out)
    pub fn clock_tick(&mut self, clock: &dyn Fn(usize) -> FxClock) {
        if self.last_clock.elapsed() < Duration::from_millis(100) { return; }
        self.last_clock = Instant::now();
        let units: Vec<(usize, Instant)> = self.last_active.iter().map(|(u, t)| (*u, *t)).collect();
        for (u, t) in units {
            let busy = (u == self.ch && self.on) || (u < 4 && self.pad[u].is_some());
            if busy { self.last_active.insert(u, Instant::now()); }
            if busy || t.elapsed() < Duration::from_secs(15) { self.send_clock(u, clock); } else { self.last_active.remove(&u); }
        }
    }
    pub fn select_step(&mut self, step: i32, clock: &dyn Fn(usize) -> FxClock) -> Option<String> {
        let names = self.kinds();
        let k = names.iter().position(|x| *x == self.fx).unwrap_or(0) as i32;
        self.fx = names[(k + step).rem_euclid(names.len() as i32) as usize];
        self.send(clock);
        self.dirty = true;
        self.on.then(|| format!("Beat FX: {}", self.fx.name()))
    }
    pub fn select(&mut self, k: FxKind, clock: &dyn Fn(usize) -> FxClock) { self.fx = k; self.send(clock); self.dirty = true; }
    pub fn beat_step(&mut self, step: i32, clock: &dyn Fn(usize) -> FxClock) {
        let s = spec(self.fx);
        let Some(list) = s.beats else { return };
        let cur = self.beat();
        let k = list.iter().position(|b| (b - cur).abs() < 1e-6).or_else(|| list.iter().position(|b| *b == s.beat)).unwrap_or(0) as i32;
        let k = (k + step).clamp(0, list.len() as i32 - 1) as usize;
        self.beats.insert(self.fx, list[k]);
        self.send(clock);
        self.dirty = true;
    }
    pub fn channel(&mut self, c: usize, clock: &dyn Fn(usize) -> FxClock) {
        let c = c.min(MASTER_FX);
        if c == self.ch { return; }
        // the old insert stops taking input and rings out; the new one picks up
        self.post(self.ch, false, SlotSet { on: Some(false), ..Default::default() });
        self.ch = c;
        self.send(clock);
        self.dirty = true;
    }
    pub fn set_on(&mut self, v: bool, clock: &dyn Fn(usize) -> FxClock) { self.on = v; self.send(clock); }
    pub fn set_depth(&mut self, v: f64, clock: &dyn Fn(usize) -> FxClock) { self.depth = v.clamp(0.0, 1.0); self.send(clock); self.dirty = true; }
    /// PUFF: the hit strength from the Puffco (0..1), straight to the BEAT FX insert
    pub fn set_puff(&self, v: f64) { let _ = self.tx.send(Cmd::FxMod { unit: self.ch, v }); }
    pub fn pad_down(&mut self, deck: usize, bank: usize, p: usize, clock: &dyn Fn(usize) -> FxClock) {
        let Some(&(fx, beats, depth)) = PAD_FX.get(bank).and_then(|b| b.get(p)) else { return };
        self.pad[deck] = Some((p, bank));
        self.last_active.insert(deck, Instant::now());
        self.send_clock(deck, clock);
        self.post(deck, true, SlotSet { fx: Some(fx), beats: Some(beats), depth: Some(depth), on: Some(true) });
    }
    pub fn pad_up(&mut self, deck: usize, bank: usize, p: usize) {
        if self.pad[deck] != Some((p, bank)) { return; }
        self.pad[deck] = None;
        self.post(deck, true, SlotSet { on: Some(false), ..Default::default() });
    }
    /// a deck went away (4-deck mode off): let go of its PAD FX
    pub fn release_deck(&mut self, deck: usize) {
        if let Some((p, b)) = self.pad[deck] { self.pad_up(deck, b, p); }
    }
}
