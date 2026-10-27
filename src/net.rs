//! Wi-Fi and Bluetooth for Settings → Wi-Fi / Bluetooth: NetworkManager's `nmcli` and BlueZ's `bluetoothctl`,
//! run on a worker thread of their own ("cratebox-net") so a slow scan or pairing never holds up a frame.
//! The UI thread sends `Cmd`s and reads the latest `NetView`. Scans only happen when asked for (a Wi-Fi scan
//! can upset audio timing on some cards); while the panel is open the lists refresh every few seconds from
//! what the system already knows.

use std::process::Command;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wifi { pub ssid: String, pub signal: u32, pub secure: bool, pub active: bool, pub saved: bool }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BtDev { pub mac: String, pub name: String, pub paired: bool, pub connected: bool }

#[derive(Clone, Debug, Default)]
pub struct NetView {
    pub wifi_on: bool,
    pub wifi: Vec<Wifi>,
    pub bt_on: bool,
    pub bt: Vec<BtDev>,
    /// a job in progress ("Scanning…", "Connecting to …"), shown with a spinner
    pub busy: Option<String>,
    /// the last result, good or bad, in plain words
    pub note: String,
    /// nmcli / bluetoothctl missing (not Cratebox OS)
    pub no_nmcli: bool,
    pub no_bluetoothctl: bool,
}

pub enum Cmd {
    Refresh,
    WifiScan,
    WifiRadio(bool),
    WifiConnect { ssid: String, password: Option<String> },
    WifiDisconnect(String),
    WifiForget(String),
    BtPower(bool),
    BtScan,
    BtPair(String),
    BtConnect(String),
    BtDisconnect(String),
    BtForget(String),
}

pub struct Net { tx: Sender<Cmd>, pub view: Arc<Mutex<NetView>>, last: Option<Instant> }

impl Default for Net { fn default() -> Self { Net::new() } }

impl Net {
    pub fn new() -> Net {
        let (tx, rx) = channel();
        let view = Arc::new(Mutex::new(NetView::default()));
        let v = view.clone();
        let _ = std::thread::Builder::new().name("cratebox-net".into()).spawn(move || worker(rx, v));
        Net { tx, view, last: None }
    }
    pub fn send(&self, c: Cmd) { let _ = self.tx.send(c); }
    /// call each frame while the panel is open: refreshes every 4 s
    pub fn tick(&mut self) {
        if self.last.is_none_or(|t| t.elapsed() > Duration::from_secs(4)) { self.last = Some(Instant::now()); self.send(Cmd::Refresh); }
    }
    pub fn snapshot(&self) -> NetView { self.view.lock().map(|v| v.clone()).unwrap_or_default() }
}

fn run(prog: &str, args: &[&str]) -> Result<String, String> {
    match Command::new(prog).args(args).output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => {
            let e = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let e = if e.is_empty() { String::from_utf8_lossy(&o.stdout).trim().to_string() } else { e };
            Err(e.lines().last().unwrap_or("failed").trim_start_matches("Error: ").to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

fn worker(rx: Receiver<Cmd>, view: Arc<Mutex<NetView>>) {
    let set = |f: &dyn Fn(&mut NetView)| { if let Ok(mut v) = view.lock() { f(&mut v); } };
    let busy = |s: &str| { let s = s.to_string(); set(&move |v| { v.busy = Some(s.clone()); }) };
    let done = |r: Result<String, String>, ok: &str| {
        let note = match r { Ok(_) => ok.to_string(), Err(e) => e };
        set(&move |v| { v.busy = None; v.note = note.clone(); });
    };
    let have = |p: &str| Command::new(p).arg("--version").output().is_ok();
    let (nm, btc) = (have("nmcli"), have("bluetoothctl"));
    set(&|v| { v.no_nmcli = !nm; v.no_bluetoothctl = !btc; });
    let refresh = |rescan: bool| {
        if nm {
            let on = run("nmcli", &["radio", "wifi"]).map(|s| s.trim() == "enabled").unwrap_or(false);
            let saved = run("nmcli", &["-t", "-f", "NAME,TYPE", "connection", "show"]).map(|s| parse_saved(&s)).unwrap_or_default();
            let list = if on { run("nmcli", &["-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list", "--rescan", if rescan { "yes" } else { "no" }]).map(|s| parse_wifi(&s, &saved)).unwrap_or_default() } else { vec![] };
            set(&move |v| { v.wifi_on = on; v.wifi = list.clone(); });
        }
        if btc {
            let on = run("bluetoothctl", &["show"]).map(|s| s.contains("Powered: yes")).unwrap_or(false);
            let all = run("bluetoothctl", &["devices"]).unwrap_or_default();
            let paired = run("bluetoothctl", &["devices", "Paired"]).unwrap_or_default();
            let conn = run("bluetoothctl", &["devices", "Connected"]).unwrap_or_default();
            let list = parse_bt(&all, &paired, &conn);
            set(&move |v| { v.bt_on = on; v.bt = list.clone(); });
        }
    };
    for c in rx {
        match c {
            Cmd::Refresh => refresh(false),
            Cmd::WifiScan => { busy("Looking for Wi-Fi networks…"); refresh(true); set(&|v| v.busy = None); }
            Cmd::WifiRadio(on) => { if on { let _ = run("rfkill", &["unblock", "wifi"]); } done(run("nmcli", &["radio", "wifi", if on { "on" } else { "off" }]), if on { "Wi-Fi on" } else { "Wi-Fi off" }); refresh(false); }
            Cmd::WifiConnect { ssid, password } => {
                busy(&format!("Connecting to {ssid}…"));
                let r = match &password {
                    Some(p) => run("nmcli", &["device", "wifi", "connect", &ssid, "password", p]),
                    None => run("nmcli", &["device", "wifi", "connect", &ssid]),
                };
                done(r, &format!("Connected to {ssid}"));
                refresh(false);
            }
            Cmd::WifiDisconnect(ssid) => { done(run("nmcli", &["connection", "down", "id", &ssid]), &format!("Disconnected from {ssid}")); refresh(false); }
            Cmd::WifiForget(ssid) => { done(run("nmcli", &["connection", "delete", "id", &ssid]), &format!("Forgot {ssid}")); refresh(false); }
            Cmd::BtPower(on) => { if on { let _ = run("rfkill", &["unblock", "bluetooth"]); } done(run("bluetoothctl", &["power", if on { "on" } else { "off" }]), if on { "Bluetooth on" } else { "Bluetooth off" }); refresh(false); }
            Cmd::BtScan => { busy("Looking for Bluetooth devices (10 s)…"); let r = run("bluetoothctl", &["--timeout", "10", "scan", "on"]); refresh(false); done(r.map(|_| String::new()), "Scan finished: pick a device to pair"); }
            Cmd::BtPair(mac) => {
                busy("Pairing… (put the device in pairing mode)");
                let r = run("bluetoothctl", &["--timeout", "20", "pair", &mac]).and_then(|_| run("bluetoothctl", &["trust", &mac])).and_then(|_| run("bluetoothctl", &["--timeout", "10", "connect", &mac]));
                done(r, "Paired and connected");
                refresh(false);
            }
            Cmd::BtConnect(mac) => { busy("Connecting…"); done(run("bluetoothctl", &["--timeout", "10", "connect", &mac]), "Connected"); refresh(false); }
            Cmd::BtDisconnect(mac) => { done(run("bluetoothctl", &["disconnect", &mac]), "Disconnected"); refresh(false); }
            Cmd::BtForget(mac) => { done(run("bluetoothctl", &["remove", &mac]), "Forgotten"); refresh(false); }
        }
    }
}

/// nmcli -t splits fields with ':' and escapes ':' and '\' inside them with '\'
fn split_terse(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut esc = false;
    for ch in line.chars() {
        if esc { out.last_mut().unwrap().push(ch); esc = false; }
        else if ch == '\\' { esc = true; }
        else if ch == ':' { out.push(String::new()); }
        else { out.last_mut().unwrap().push(ch); }
    }
    out
}

/// `nmcli -t -f NAME,TYPE connection show` → the saved Wi-Fi network names
pub fn parse_saved(s: &str) -> Vec<String> {
    s.lines().filter_map(|l| { let f = split_terse(l); (f.len() >= 2 && f[1].contains("wireless")).then(|| f[0].clone()) }).collect()
}

/// `nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list` → one entry per network name (strongest), the
/// connected one first, then by signal; hidden networks (no name) left out
pub fn parse_wifi(s: &str, saved: &[String]) -> Vec<Wifi> {
    let mut out: Vec<Wifi> = vec![];
    for l in s.lines() {
        let f = split_terse(l);
        if f.len() < 4 || f[1].is_empty() { continue; }
        let w = Wifi { active: f[0].trim() == "*", ssid: f[1].clone(), signal: f[2].trim().parse().unwrap_or(0), secure: !f[3].trim().is_empty() && f[3].trim() != "--", saved: saved.contains(&f[1]) };
        match out.iter_mut().find(|x| x.ssid == w.ssid) {
            Some(x) => { x.active |= w.active; if w.signal > x.signal { x.signal = w.signal; } }
            None => out.push(w),
        }
    }
    out.sort_by(|a, b| b.active.cmp(&a.active).then(b.signal.cmp(&a.signal)));
    out
}

/// `bluetoothctl devices` (+ the Paired / Connected lists) → devices, connected then paired first;
/// unnamed devices (the name is just the address) left out
pub fn parse_bt(all: &str, paired: &str, connected: &str) -> Vec<BtDev> {
    let macs = |s: &str| -> Vec<String> { s.lines().filter_map(|l| l.strip_prefix("Device ").and_then(|r| r.split_whitespace().next()).map(String::from)).collect() };
    let (p, c) = (macs(paired), macs(connected));
    let mut out: Vec<BtDev> = all.lines().filter_map(|l| {
        let r = l.strip_prefix("Device ")?;
        let (mac, name) = r.split_once(' ').unwrap_or((r, ""));
        if name.is_empty() || name.replace('-', ":") == mac { return None; }
        Some(BtDev { mac: mac.to_string(), name: name.to_string(), paired: p.iter().any(|x| x == mac), connected: c.iter().any(|x| x == mac) })
    }).collect();
    out.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.paired.cmp(&a.paired)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_nmcli_and_bluetoothctl() {
        let saved = parse_saved("Home:802-11-wireless\nWired connection 1:802-3-ethernet\nCafe\\: Free:802-11-wireless\n");
        assert_eq!(saved, vec!["Home".to_string(), "Cafe: Free".to_string()]);
        let w = parse_wifi(" :Cafe\\: Free:40:\n*:Home:72:WPA2\n :Home:80:WPA2\n :Neighbour:55:WPA1 WPA2\n ::30:WPA2\n", &saved);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0], Wifi { ssid: "Home".into(), signal: 80, secure: true, active: true, saved: true });
        assert_eq!(w[1].ssid, "Neighbour");
        assert!(!w[2].secure && w[2].saved);
        let b = parse_bt("Device 11:22:33:44:55:66 AirPods\nDevice AA:BB:CC:DD:EE:FF Speaker\nDevice 01:02:03:04:05:06 01-02-03-04-05-06\n",
            "Device AA:BB:CC:DD:EE:FF Speaker\nDevice 11:22:33:44:55:66 AirPods\n", "Device 11:22:33:44:55:66 AirPods\n");
        assert_eq!(b.len(), 2);
        assert!(b[0].connected && b[0].name == "AirPods");
        assert!(b[1].paired && !b[1].connected);
    }
}
