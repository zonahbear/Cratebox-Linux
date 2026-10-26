//! Windows-only bits: the audio thread's MMCSS "Pro Audio" class, message boxes for startup problems
//! (a GUI app has no terminal), attaching to the terminal for --version, and the drive letters the file
//! picker lists. Called from the UI thread, except `pro_audio_thread` (the audio callback, once).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{w, HSTRING};
use windows::Win32::System::Threading::{AvSetMmThreadCharacteristicsW, AvSetMmThreadPriority, AVRT_PRIORITY_HIGH};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK};

/// Joins the calling (audio callback) thread to the "Pro Audio" MMCSS task, the way pro audio apps on
/// Windows get their callbacks run ahead of everything else. Needs no rights; if it fails the thread
/// stays at normal priority. Windows undoes it when the thread ends.
pub fn pro_audio_thread() {
    static WARNED: AtomicBool = AtomicBool::new(false);
    let mut task = 0u32;
    match unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task) } {
        Ok(h) => { let _ = unsafe { AvSetMmThreadPriority(h, AVRT_PRIORITY_HIGH) }; }
        Err(e) => if !WARNED.swap(true, Ordering::Relaxed) { eprintln!("cratebox: no Pro Audio priority for the audio thread ({e}), running at normal priority"); },
    }
}

pub fn message_box(text: &str) {
    unsafe { MessageBoxW(None, &HSTRING::from(text), w!("Cratebox"), MB_OK | MB_ICONWARNING); }
}

/// started from PowerShell or cmd: print there (--version, warnings); started from Explorer: nothing happens
pub fn attach_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

/// every drive letter in use (C:\, USB sticks, SD cards), for the file picker's places
pub fn drives() -> Vec<PathBuf> {
    use windows::Win32::Storage::FileSystem::GetLogicalDrives;
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8).filter(|i| mask & (1 << i) != 0).map(|i| PathBuf::from(format!("{}:\\", (b'A' + i) as char))).collect()
}
