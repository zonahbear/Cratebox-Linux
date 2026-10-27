//! USB sticks and SD cards: mounts plugged-in drives through udisks (the Cratebox session has no desktop
//! automounter) and reports which mounted drives hold a rekordbox export. Its own thread, every 3 s;
//! started from `App::new`, results come back as `Job::Drives`.

use std::path::PathBuf;
use std::process::Command;

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
    for (name, hot, fs, mut mp) in parts {
        if !hot || !fs { continue; }
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
