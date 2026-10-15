//! MIDI ports through midir (the ALSA sequencer on Linux): connect to the controller, collect its
//! messages for the UI thread, send LEDs back. Ports are rescanned every couple of seconds so a
//! controller plugged in later is picked up.

use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

pub struct MidiIo {
    input: Option<MidiInputConnection<()>>,
    output: Option<MidiOutputConnection>,
    pub name: String,
    pub ports: Vec<String>,
    tx: Sender<Vec<u8>>,
    pub rx: Receiver<Vec<u8>>,
    last_scan: Instant,
    pub error: Option<String>,
    wake: Option<Box<dyn Fn() + Send + Sync>>,
    wake_arc: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    scanner: Option<MidiInput>,
}

/// the ALSA client part of a port name: "DDJ-FLX4:DDJ-FLX4 MIDI 1 24:0" -> "DDJ-FLX4"
fn client(n: &str) -> &str { n.split(':').next().unwrap_or(n) }
/// "DDJ-FLX4 MIDI 1" -> "DDJ-FLX4", for matching an output to its input
fn base(n: &str) -> String {
    let mut w: Vec<&str> = client(n).split_whitespace().collect();
    while w.len() > 1 {
        let last = w[w.len() - 1];
        if ["MIDI", "IN", "OUT"].contains(&last.to_uppercase().as_str()) || last.chars().all(|c| c.is_ascii_digit()) { w.pop(); } else { break; }
    }
    w.join(" ")
}

impl MidiIo {
    pub fn new() -> MidiIo {
        let (tx, rx) = channel();
        let mut m = MidiIo { input: None, output: None, name: String::new(), ports: vec![], tx, rx, last_scan: Instant::now() - Duration::from_secs(10), error: None, wake: None, wake_arc: None, scanner: None };
        m.scan();
        m
    }
    /// called when a message arrives (to wake the UI)
    pub fn set_wake(&mut self, f: std::sync::Arc<dyn Fn() + Send + Sync>) { self.wake_arc = Some(f); let _ = &self.wake; }

    pub fn scan(&mut self) -> bool {
        self.last_scan = Instant::now();
        if self.scanner.is_none() { self.scanner = MidiInput::new("Cratebox scan").map_err(|e| { self.error = Some(format!("MIDI unavailable ({e})")); }).ok(); }
        match self.scanner.as_ref().ok_or(()) {
            Ok(mi) => {
                let ports: Vec<String> = mi.ports().iter().filter_map(|p| mi.port_name(p).ok())
                    .filter(|n| !n.starts_with("Midi Through") && !n.contains("Cratebox")).collect();
                let changed = ports != self.ports;
                self.ports = ports;
                self.error = None;
                changed
            }
            Err(_) => false,
        }
    }
    pub fn maybe_rescan(&mut self) -> bool {
        // no ALSA sequencer at all: don't keep asking
        let every = if self.error.is_some() { 15 } else { 2 };
        if self.last_scan.elapsed() < Duration::from_secs(every) { return false; }
        let changed = self.scan();
        // the connected port went away
        if !self.name.is_empty() && !self.ports.contains(&self.name) { self.disconnect(); return true; }
        changed
    }
    pub fn disconnect(&mut self) {
        if let Some(c) = self.input.take() { c.close(); }
        if let Some(c) = self.output.take() { c.close(); }
        self.name.clear();
    }
    /// connect to `name`'s input and the matching output
    pub fn connect(&mut self, name: &str) -> Result<(), String> {
        self.disconnect();
        let mut mi = MidiInput::new("Cratebox").map_err(|e| e.to_string())?;
        mi.ignore(midir::Ignore::ActiveSense);
        let port = mi.ports().into_iter().find(|p| mi.port_name(p).map(|n| n == name).unwrap_or(false)).ok_or("controller not found")?;
        let tx = self.tx.clone();
        let wake = self.wake_arc.clone();
        let conn = mi.connect(&port, "Cratebox in", move |_, msg, _| {
            let _ = tx.send(msg.to_vec());
            if let Some(w) = &wake { w(); }
        }, ()).map_err(|e| e.to_string())?;
        self.input = Some(conn);
        self.name = name.to_string();
        if let Ok(mo) = MidiOutput::new("Cratebox") {
            let b = base(name);
            let ports = mo.ports();
            let pick = ports.iter().find(|p| mo.port_name(p).map(|n| n == name).unwrap_or(false))
                .or_else(|| ports.iter().find(|p| mo.port_name(p).map(|n| client(&n) == client(name)).unwrap_or(false)))
                .or_else(|| ports.iter().find(|p| mo.port_name(p).map(|n| !b.is_empty() && base(&n) == b).unwrap_or(false)));
            if let Some(p) = pick.cloned() { self.output = mo.connect(&p, "Cratebox out").ok(); }
        }
        Ok(())
    }
    pub fn has_output(&self) -> bool { self.output.is_some() }
    pub fn send(&mut self, bytes: &[u8]) { if let Some(o) = self.output.as_mut() { let _ = o.send(bytes); } }
}

impl Default for MidiIo { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn port_names() {
        assert_eq!(client("DDJ-FLX4:DDJ-FLX4 MIDI 1 24:0"), "DDJ-FLX4");
        assert_eq!(base("DDJ-FLX4 MIDI 1"), "DDJ-FLX4");
    }
}
