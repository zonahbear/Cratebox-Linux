//! USB sticks and SD cards: mounts plugged-in drives through udisks (the Cratebox session has no desktop
//! automounter) and reports which mounted drives hold a rekordbox export. Its own thread, every 3 s;
//! started from `App::new`, results come back as `Job::Drives`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// drives ejected from Cratebox: not mounted again until they're unplugged
static EJECTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn walk(v: &serde_json::Value, out: &mut Vec<(String, bool, bool, PathBuf)>, hot: bool) {
    let Some(a) = v.as_array() else { return };
    for d in a {
        let hot = hot || d["rm"].as_bool().unwrap_or(false) || d["hotplug"].as_bool().unwrap_or(false);
        let name = d["name"].as_str().unwrap_or("").to_string();
        let fs = d["fstype"].as_str().is_some_and(|s| !s.is_empty() && s != "swap");
        let mp = d["mountpoint"].as_str().map(PathBuf::from).unwrap_or_default();
        out.push((name, hot, fs, mp));
        walk(&d["children"], out, hot);
    }
}

/// one pass: mount any removable filesystem that isn't mounted yet, return the mount points of
/// removable drives with PIONEER/rekordbox/export.pdb
pub fn scan() -> Vec<PathBuf> {
    let Ok(o) = Command::new("lsblk").args(["-J", "-p", "-o", "NAME,RM,HOTPLUG,FSTYPE,MOUNTPOINT"]).output() else { return vec![] };
    let Ok(j) = serde_json::from_slice::<serde_json::Value>(&o.stdout) else { return vec![] };
    let mut parts = vec![];
    walk(&j["blockdevices"], &mut parts, false);
    let mut out = vec![];
    if let Ok(mut e) = EJECTED.lock() {
        if let Some(set) = e.as_mut() { set.retain(|n| parts.iter().any(|p| &p.0 == n)); }
    }
    let ejected = EJECTED.lock().ok().and_then(|e| e.clone()).unwrap_or_default();
    for (name, hot, fs, mut mp) in parts {
        if !hot || !fs || ejected.contains(&name) { continue; }
        if mp.as_os_str().is_empty() {
            let Ok(m) = Command::new("udisksctl").args(["mount", "-b", &name, "--no-user-interaction"]).output() else { continue };
            // "Mounted /dev/sdb1 at /media/jonah/STICK"
            let s = String::from_utf8_lossy(&m.stdout);
            let Some(at) = s.split(" at ").nth(1) else { continue };
            mp = PathBuf::from(at.trim().trim_end_matches('.'));
        }
        if crate::playlists::sub(&mp, "/PIONEER/rekordbox/export.pdb").exists() { out.push(mp); }
    }
    out
}

/// unmount the drive mounted at `root` and power it off so it can be pulled out; Err says why not
pub fn eject(root: &Path) -> Result<(), String> {
    let o = Command::new("findmnt").args(["-n", "-o", "SOURCE", "--target"]).arg(root).output().map_err(|e| e.to_string())?;
    let dev = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if dev.is_empty() { return Err("That drive isn't mounted".into()); }
    if let Ok(mut e) = EJECTED.lock() { e.get_or_insert_with(HashSet::new).insert(dev.clone()); }
    let u = Command::new("udisksctl").args(["unmount", "-b", &dev, "--no-user-interaction"]).output().map_err(|e| e.to_string())?;
    if !u.status.success() {
        if let Ok(mut e) = EJECTED.lock() { if let Some(s) = e.as_mut() { s.remove(&dev); } }
        return Err(format!("Couldn't eject: {}", String::from_utf8_lossy(&u.stderr).trim()));
    }
    // power-off is best-effort (some card readers refuse); unmounted is already safe to pull
    let _ = Command::new("udisksctl").args(["power-off", "-b", &dev, "--no-user-interaction"]).output();
    Ok(())
}
