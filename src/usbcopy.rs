//! Copy a rekordbox USB stick (or SD card) to this computer, and later bring the copy up to date from
//! the stick: so the stick only has to come out when the library changes on the main computer.
//! Runs on its own thread (started from `App::copy_device`); never writes to the stick.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// where local copies live: ~/Music/Cratebox USB/<stick name> local
pub fn copy_root(name: &str) -> PathBuf {
    dirs::audio_dir().or_else(|| dirs::home_dir().map(|h| h.join("Music"))).unwrap_or_else(|| PathBuf::from("."))
        .join("Cratebox USB").join(format!("{name} local"))
}

/// is this device folder one of our local copies?
pub fn is_copy(root: &Path) -> bool {
    root.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "Cratebox USB")
}

fn skip(name: &str) -> bool {
    name.starts_with('.') || name == "System Volume Information" || name == "$RECYCLE.BIN"
}

fn walk(dir: &Path, rel: &Path, out: &mut Vec<(PathBuf, u64, Option<SystemTime>)>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_string();
        if skip(&name) { continue; }
        let md = e.metadata()?;
        let r = rel.join(&name);
        if md.is_dir() { walk(&e.path(), &r, out)?; } else if md.is_file() { out.push((r, md.len(), md.modified().ok())); }
    }
    Ok(())
}

/// what a copy or update did
pub struct Copied { pub files: usize, pub bytes: u64, pub same: usize }

/// copy every file on `src` that `dst` doesn't have yet, or has at a different size or date; `progress`
/// gets (bytes done, bytes to copy). Each file goes in under a temporary name first, so a stick pulled
/// out halfway leaves the old copy of that file, never half of one. Files gone from the stick stay.
pub fn copy_tree(src: &Path, dst: &Path, mut progress: impl FnMut(u64, u64)) -> Result<Copied, String> {
    let mut files = vec![];
    walk(src, Path::new(""), &mut files).map_err(|e| format!("Couldn't read the stick: {e}"))?;
    let todo: Vec<_> = files.iter().filter(|(r, len, mt)| {
        match std::fs::metadata(dst.join(r)) {
            Ok(m) => m.len() != *len || m.modified().ok() != *mt,
            Err(_) => true,
        }
    }).collect();
    let total: u64 = todo.iter().map(|f| f.1).sum();
    let mut done = 0u64;
    progress(0, total);
    for (r, len, mt) in &todo {
        let (from, to) = (src.join(r), dst.join(r));
        if let Some(p) = to.parent() { std::fs::create_dir_all(p).map_err(|e| format!("Couldn't make {}: {e}", p.display()))?; }
        let tmp = to.with_file_name(format!(".{}.part", to.file_name().unwrap_or_default().to_string_lossy()));
        std::fs::copy(&from, &tmp).map_err(|e| format!("Couldn't copy {}: {e}", r.display()))?;
        if let Some(t) = mt { if let Ok(f) = std::fs::File::options().write(true).open(&tmp) { let _ = f.set_modified(*t); } }
        std::fs::rename(&tmp, &to).map_err(|e| format!("Couldn't copy {}: {e}", r.display()))?;
        done += len;
        progress(done, total);
    }
    Ok(Copied { files: todo.len(), bytes: total, same: files.len() - todo.len() })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copies_then_updates_only_changes() {
        let base = std::env::temp_dir().join(format!("cratebox-usbcopy-{}", std::process::id()));
        let (src, dst) = (base.join("STICK"), base.join("Cratebox USB").join("STICK local"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(src.join("PIONEER/rekordbox")).unwrap();
        std::fs::create_dir_all(src.join("Contents/A")).unwrap();
        std::fs::create_dir_all(src.join(".Trashes")).unwrap();
        std::fs::write(src.join("PIONEER/rekordbox/export.pdb"), b"db1").unwrap();
        std::fs::write(src.join("Contents/A/t.mp3"), b"music").unwrap();
        std::fs::write(src.join(".Trashes/x"), b"no").unwrap();
        let c = copy_tree(&src, &dst, |_, _| {}).unwrap();
        assert_eq!((c.files, c.same), (2, 0));
        assert!(!dst.join(".Trashes").exists() && is_copy(&dst));
        let c = copy_tree(&src, &dst, |_, _| {}).unwrap();
        assert_eq!((c.files, c.same), (0, 2), "nothing changed, nothing copied");
        std::fs::write(src.join("PIONEER/rekordbox/export.pdb"), b"db2!").unwrap();
        let c = copy_tree(&src, &dst, |_, _| {}).unwrap();
        assert_eq!(c.files, 1);
        assert_eq!(std::fs::read(dst.join("PIONEER/rekordbox/export.pdb")).unwrap(), b"db2!");
        let _ = std::fs::remove_dir_all(&base);
    }
}
