//! HEAT's Bluetooth side on Windows: WinRT Bluetooth LE (Windows 10 1703+), with the same `Bus` / `Seen` /
//! `Ble` shape as the BlueZ module so the worker doesn't care which one it has. Runs on the HEAT worker
//! thread; WinRT calls block with `.get()`, and its event handlers (advertisements, notifications, the
//! link dropping) run on WinRT's own threads and only fill shared state or channels.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{IInspectable, GUID};
use windows::Devices::Bluetooth::Advertisement::{BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher, BluetoothLEScanningMode};
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristic, GattCharacteristicProperties, GattClientCharacteristicConfigurationDescriptorValue, GattCommunicationStatus, GattDeviceService,
    GattValueChangedEventArgs, GattWriteOption,
};
use windows::Devices::Bluetooth::{BluetoothAdapter, BluetoothCacheMode, BluetoothConnectionStatus, BluetoothLEDevice};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationCustomPairing, DevicePairingKinds, DevicePairingRequestedEventArgs, DevicePairingResultStatus};
use windows::Devices::Radios::RadioState;
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::{DataReader, DataWriter, IBuffer};

fn err(e: impl std::fmt::Display) -> String { e.to_string() }
fn addr_text(a: u64) -> String { (0..6).rev().map(|i| format!("{:02X}", (a >> (i * 8)) & 0xff)).collect::<Vec<_>>().join(":") }
fn guid(s: &str) -> GUID { GUID::try_from(s).unwrap_or_default() }
fn bytes(b: &IBuffer) -> Vec<u8> {
    let Ok(r) = DataReader::FromBuffer(b) else { return vec![] };
    let n = r.UnconsumedBufferLength().unwrap_or(0) as usize;
    let mut v = vec![0; n];
    if r.ReadBytes(&mut v).is_err() { v.clear(); }
    v
}
fn buffer(v: &[u8]) -> Result<IBuffer, String> {
    let w = DataWriter::new().map_err(err)?;
    w.WriteBytes(v).map_err(err)?;
    w.DetachBuffer().map_err(err)
}

#[derive(Clone)]
pub struct Seen { pub path: String, pub addr: String, pub name: String, pub puffco: bool, pub connected: bool }

fn is_puffco(name: &str, uuids: &[GUID], mfr: bool) -> bool {
    let ln = name.to_lowercase();
    uuids.iter().any(|u| *u == guid(lorax::SERVICE) || *u == guid(lorax::SERVICE_LEGACY)) || ["peak", "prox", "puff", "pivot"].iter().any(|k| ln.starts_with(k)) || mfr
}

/// the adapter, an advertisement watcher filling `seen`, and the paired devices (they're listed even when quiet)
pub struct Bus { watcher: BluetoothLEAdvertisementWatcher, seen: Arc<Mutex<HashMap<u64, Seen>>> }

impl Bus {
    pub fn new() -> Result<Bus, String> {
        // the worker thread joins the multithreaded apartment (fine if something already did)
        let _ = unsafe { windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED) };
        let adapter = BluetoothAdapter::GetDefaultAsync().and_then(|o| o.get()).map_err(|_| "no Bluetooth adapter found".to_string())?;
        if !adapter.IsLowEnergySupported().unwrap_or(false) { return Err("this Bluetooth adapter has no Bluetooth LE".into()); }
        if let Ok(radio) = adapter.GetRadioAsync().and_then(|o| o.get()) {
            if radio.State().map(|s| s != RadioState::On).unwrap_or(false) { return Err("Bluetooth is off (turn it on in Settings → Bluetooth & devices)".into()); }
        }
        let watcher = BluetoothLEAdvertisementWatcher::new().map_err(err)?;
        watcher.SetScanningMode(BluetoothLEScanningMode::Active).map_err(err)?;
        let seen: Arc<Mutex<HashMap<u64, Seen>>> = Arc::new(Mutex::new(HashMap::new()));
        let s2 = seen.clone();
        watcher.Received(&TypedEventHandler::new(move |_: &Option<BluetoothLEAdvertisementWatcher>, a: &Option<BluetoothLEAdvertisementReceivedEventArgs>| {
            let Some(a) = a else { return Ok(()) };
            let addr = a.BluetoothAddress()?;
            let ad = a.Advertisement()?;
            let name = ad.LocalName().map(|n| n.to_string()).unwrap_or_default();
            let uuids: Vec<GUID> = ad.ServiceUuids().map(|v| v.into_iter().collect()).unwrap_or_default();
            let mfr = ad.ManufacturerData().map(|v| v.into_iter().any(|m| m.CompanyId().ok() == Some(0x0c03))).unwrap_or(false);
            if let Ok(mut m) = s2.lock() {
                let e = m.entry(addr).or_insert_with(|| Seen { path: format!("{addr:012x}"), addr: addr_text(addr), name: String::new(), puffco: false, connected: false });
                // scan responses carry the name, other packets the service: keep what each one says
                if !name.is_empty() { e.name = name; }
                e.puffco = e.puffco || is_puffco(&e.name, &uuids, mfr);
            }
            Ok(())
        })).map_err(err)?;
        Ok(Bus { watcher, seen })
    }
    pub fn start_scan(&self) -> Result<(), String> {
        self.paired();
        self.watcher.Start().map_err(|e| format!("couldn't start a Bluetooth scan ({e})"))
    }
    pub fn stop_scan(&self) { let _ = self.watcher.Stop(); }
    /// devices Windows has paired before: they may not be advertising (or already connected)
    fn paired(&self) {
        let Ok(sel) = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true) else { return };
        let Ok(list) = DeviceInformation::FindAllAsyncAqsFilter(&sel).and_then(|o| o.get()) else { return };
        for di in list {
            let name = di.Name().map(|n| n.to_string()).unwrap_or_default();
            if !is_puffco(&name, &[], false) { continue; }
            let Ok(d) = di.Id().and_then(|id| BluetoothLEDevice::FromIdAsync(&id)).and_then(|o| o.get()) else { continue };
            let Ok(addr) = d.BluetoothAddress() else { continue };
            let connected = d.ConnectionStatus().map(|c| c == BluetoothConnectionStatus::Connected).unwrap_or(false);
            if let Ok(mut m) = self.seen.lock() {
                m.insert(addr, Seen { path: format!("{addr:012x}"), addr: addr_text(addr), name, puffco: true, connected });
            }
            let _ = d.Close();
        }
    }
    pub fn devices(&self) -> Vec<Seen> { self.seen.lock().map(|m| m.values().cloned().collect()).unwrap_or_default() }
}

/// one Puffco over GATT: writes to the command characteristic, replies come back as notifications
pub struct Ble {
    dev: BluetoothLEDevice, services: Vec<GattDeviceService>, cmd: GattCharacteristic, reply: GattCharacteristic,
    name: String, addr: String, is_proxy: bool,
    notes: Receiver<Vec<u8>>, lost: Arc<AtomicBool>,
    on: bool, seq: u16, with_resp: bool, fmt: Fmt, live_fmt: Option<Fmt>, temp_path: &'static str, soc: lorax::SocDecoder,
}

/// the device's Lorax characteristics: (service, command, reply, version)
fn find_chars(dev: &BluetoothLEDevice) -> Result<(GattDeviceService, GattCharacteristic, GattCharacteristic, Option<GattCharacteristic>), String> {
    let ch = |svc: &GattDeviceService, u: &str| -> Option<GattCharacteristic> {
        let r = svc.GetCharacteristicsForUuidWithCacheModeAsync(guid(u), BluetoothCacheMode::Uncached).and_then(|o| o.get()).ok()?;
        if r.Status().ok()? != GattCommunicationStatus::Success { return None; }
        r.Characteristics().ok()?.into_iter().next()
    };
    let mut answered = false;
    for want in [lorax::SERVICE, lorax::SERVICE_LEGACY] {
        let r = dev.GetGattServicesForUuidWithCacheModeAsync(guid(want), BluetoothCacheMode::Uncached).and_then(|o| o.get()).map_err(err)?;
        match r.Status().map_err(err)? {
            GattCommunicationStatus::Success => answered = true,
            GattCommunicationStatus::Unreachable => return Err("the device didn't answer".into()),
            GattCommunicationStatus::AccessDenied => return Err("access denied (Settings → Privacy & security → Bluetooth: let apps use it)".into()),
            s => return Err(format!("GATT error {}", s.0)),
        }
        let Some(svc) = r.Services().map_err(err)?.into_iter().next() else { continue };
        if let (Some(c), Some(rp)) = (ch(&svc, lorax::CMD), ch(&svc, lorax::REPLY)) {
            let v = ch(&svc, lorax::VERSION);
            return Ok((svc, c, rp, v));
        }
        return Err("the device's command characteristics are missing".into());
    }
    Err(if answered { "this isn't a Puffco (no Lorax service)".into() } else { "the device didn't answer".into() })
}

/// a failed connect, in words: the usual cause is another phone or app holding the Puffco
fn connect_hint(e: &str) -> String {
    let l = e.to_lowercase();
    if l.contains("didn't answer") || l.contains("unreachable") || l.contains("timeout") {
        format!("{e}. If your phone's Puffco app (or Cratebox HEAT) is connected to it, close that first, then wake the Puffco and try again")
    } else { e.to_string() }
}

/// a Puffco wants a bond: pair it "just works" style, answering Windows' question ourselves
fn pair(dev: &BluetoothLEDevice) -> Option<String> {
    let p = dev.DeviceInformation().and_then(|d| d.Pairing()).ok()?;
    if p.IsPaired().unwrap_or(false) || !p.CanPair().unwrap_or(false) { return None; }
    let custom = p.Custom().ok()?;
    let tok = custom.PairingRequested(&TypedEventHandler::new(|_: &Option<DeviceInformationCustomPairing>, a: &Option<DevicePairingRequestedEventArgs>| {
        if let Some(a) = a { a.Accept()?; }
        Ok(())
    })).ok();
    let kinds = DevicePairingKinds::ConfirmOnly | DevicePairingKinds::DisplayPin | DevicePairingKinds::ConfirmPinMatch;
    let r = custom.PairAsync(kinds).and_then(|o| o.get()).and_then(|r| r.Status());
    if let Some(t) = tok { let _ = custom.RemovePairingRequested(t); }
    match r {
        Ok(DevicePairingResultStatus::Paired) | Ok(DevicePairingResultStatus::AlreadyPaired) => None,
        Ok(s) => Some(format!("pairing status {}", s.0)),
        Err(e) => Some(e.to_string()),
    }
}

impl Ble {
    pub fn connect(_bus: Arc<Bus>, seen: &Seen, say: &mut dyn FnMut(&str)) -> Result<Ble, String> {
        let addr = u64::from_str_radix(&seen.path, 16).map_err(err)?;
        let is_proxy = { let n = seen.name.to_lowercase(); n.contains("prox") || n.contains("pivot") };
        // the link often drops right after connecting (worst on Proxy), so connect + discovery are retried together
        let mut got = None;
        for i in 1..=3u64 {
            let r = BluetoothLEDevice::FromBluetoothAddressAsync(addr).and_then(|o| o.get()).map_err(|_| "the device didn't answer".to_string())
                .and_then(|d| find_chars(&d).map(|c| (d, c)));
            match r {
                Ok(x) => { got = Some(x); break; }
                Err(e) => {
                    if i == 3 { return Err(connect_hint(&e)); }
                    say(&format!("Connecting… (try {} of 3: {e})", i + 1));
                    std::thread::sleep(Duration::from_millis(800 * i));
                }
            }
        }
        let (dev, (svc, cmd, reply, version)) = got.ok_or("no Lorax service")?;
        let pair_err = if dev.DeviceInformation().and_then(|d| d.Pairing()).and_then(|p| p.IsPaired()).unwrap_or(true) { None } else { say("Pairing…"); pair(&dev) };
        if let Some(v) = &version { let _ = v.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached).and_then(|o| o.get()); }
        // notifications: replies, and the link going away
        let (tx, notes) = channel();
        let tx = Mutex::new(tx);
        reply.ValueChanged(&TypedEventHandler::new(move |_: &Option<GattCharacteristic>, a: &Option<GattValueChangedEventArgs>| {
            if let Some(a) = a { if let Ok(tx) = tx.lock() { let _ = tx.send(bytes(&a.CharacteristicValue()?)); } }
            Ok(())
        })).map_err(err)?;
        let lost = Arc::new(AtomicBool::new(false));
        let l2 = lost.clone();
        dev.ConnectionStatusChanged(&TypedEventHandler::new(move |d: &Option<BluetoothLEDevice>, _: &Option<IInspectable>| {
            if let Some(d) = d { if d.ConnectionStatus()? == BluetoothConnectionStatus::Disconnected { l2.store(true, Ordering::Relaxed); } }
            Ok(())
        })).map_err(err)?;
        let notify = |c: &GattCharacteristic| c.WriteClientCharacteristicConfigurationDescriptorAsync(GattClientCharacteristicConfigurationDescriptorValue::Notify).and_then(|o| o.get());
        let mut subscribed = false;
        for _ in 0..3 {
            if notify(&reply).is_ok_and(|s| s == GattCommunicationStatus::Success) { subscribed = true; break; }
            std::thread::sleep(Duration::from_millis(1500));
        }
        if !subscribed { let _ = svc.Close(); let _ = dev.Close(); return Err("couldn't subscribe to the device; pair it in Settings → Bluetooth & devices and try again".into()); }
        std::thread::sleep(Duration::from_millis(150));
        let flags = cmd.CharacteristicProperties().map(|p| p.0).unwrap_or(0);
        let (w, wo) = (flags & GattCharacteristicProperties::Write.0 != 0, flags & GattCharacteristicProperties::WriteWithoutResponse.0 != 0);
        let both = w && wo;
        let with_resp = w || !wo;
        let mut me = Ble { dev, services: vec![svc], cmd, reply, name: seen.name.clone(), addr: seen.addr.clone(), is_proxy, notes, lost, on: true, seq: 0, with_resp,
            fmt: Fmt::F32, live_fmt: None, temp_path: lorax::TEMP, soc: Default::default() };
        // unauthenticated clients get dropped after ~10 s, so the handshake has to land
        let auth_problem = |s: &str| { let s = s.to_lowercase(); s.contains("not paired") || s.contains("insufficient auth") || s.contains("encrypt") || s.contains("access denied") || s.contains("accessdenied") };
        let mut why = String::new();
        for i in 1..=3 {
            why = me.auth();
            if why == "ok" || auth_problem(&why) { break; }
            if i == 1 && both { me.with_resp = !me.with_resp; }
            if i == 2 {
                let none = GattClientCharacteristicConfigurationDescriptorValue::None;
                let _ = me.reply.WriteClientCharacteristicConfigurationDescriptorAsync(none).and_then(|o| o.get()); std::thread::sleep(Duration::from_millis(300));
                let _ = notify(&me.reply); std::thread::sleep(Duration::from_millis(300));
            }
        }
        if why != "ok" {
            me.close();
            let pe = pair_err.map(|e| format!(" Pairing said: {e}.")).unwrap_or_default();
            return Err(if auth_problem(&why) { format!("it wouldn't pair ({why}).{pe} Remove it in Settings → Bluetooth & devices, wake it and try again") }
                else { format!("handshake failed ({why}).{pe} Close the Puffco app (and Cratebox HEAT on your phone): a Puffco talks to one thing at a time") });
        }
        let b = me.read(&lorax::p_temp(0), 4);
        me.fmt = lorax::detect_format(b.as_deref(), is_proxy);
        Ok(me)
    }
    fn send(&mut self, op: u8, payload: &[u8], timeout: f64) -> Result<(u8, Vec<u8>), String> {
        if !self.up() { return Err("Device is not connected".into()); }
        self.seq = self.seq.wrapping_add(1);
        if self.seq == 0 { self.seq = 1; }
        let s = self.seq;
        while self.notes.try_recv().is_ok() {}
        let opt = if self.with_resp { GattWriteOption::WriteWithResponse } else { GattWriteOption::WriteWithoutResponse };
        let st = self.cmd.WriteValueWithOptionAsync(&buffer(&lorax::frame(s, op, payload))?, opt).and_then(|o| o.get()).map_err(err)?;
        match st {
            GattCommunicationStatus::Success => {}
            GattCommunicationStatus::Unreachable => { self.on = false; return Err("Device disconnected".into()); }
            GattCommunicationStatus::AccessDenied => return Err("access denied (not paired?)".into()),
            s => return Err(format!("write failed (GATT error {})", s.0)),
        }
        let until = Instant::now() + Duration::from_secs_f64(timeout);
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() { return Err("BLE reply timed out".into()); }
            match self.notes.recv_timeout(left.min(Duration::from_millis(250))) {
                Ok(b) if b.len() >= 3 && u16::from_le_bytes([b[0], b[1]]) == s => return Ok((b[2], b[3..].to_vec())),
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => { self.on = false; return Err("Device disconnected".into()); }
            }
            if self.lost.load(Ordering::Relaxed) { self.on = false; return Err("Device disconnected".into()); }
        }
    }
    fn up(&mut self) -> bool {
        if self.lost.load(Ordering::Relaxed) { self.on = false; }
        self.on
    }
    /// Windows drops the link once nothing holds the device or its services open
    fn close(&mut self) {
        self.on = false;
        let _ = self.reply.WriteClientCharacteristicConfigurationDescriptorAsync(GattClientCharacteristicConfigurationDescriptorValue::None).and_then(|o| o.get());
        for s in self.services.drain(..) { let _ = s.Close(); }
        let _ = self.dev.Close();
    }
}

lorax_device!(Ble);
