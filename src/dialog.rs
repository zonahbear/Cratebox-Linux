//! A small file/folder browser drawn with egui (no GTK or portal needed): pick a music folder, a USB
//! stick, a rekordbox XML file, or where to save one. Mounted drives are listed as places.

use eframe::egui;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose { AddFolder, OpenUsb, ImportXml, ExportXml, ImportMapping, ExportMapping }

pub struct FileDialog {
    pub purpose: Purpose,
    pub dir: PathBuf,
    pub name: String,
    pick_dir: bool,
    save: bool,
    ext: Option<&'static str>,
    entries: Vec<(String, bool)>,
    path_text: String,
    sel: Option<String>,
    err: String,
}

pub fn home() -> PathBuf { dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")) }

/// mounted drives (USB sticks, SD cards, other disks)
pub fn mounts() -> Vec<PathBuf> {
    let user = std::env::var("USER").unwrap_or_default();
    let mut out = vec![];
    for base in [format!("/media/{user}"), format!("/run/media/{user}"), "/media".into(), "/mnt".into()] {
        if let Ok(rd) = std::fs::read_dir(&base) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() && !out.contains(&p) && p.file_name().map(|n| n.to_string_lossy() != user).unwrap_or(true) { out.push(p); }
            }
        }
    }
    out
}

impl FileDialog {
    pub fn new(purpose: Purpose, start: Option<&Path>) -> FileDialog {
        let (pick_dir, save, ext, name) = match purpose {
            Purpose::AddFolder | Purpose::OpenUsb => (true, false, None, String::new()),
            Purpose::ImportXml => (false, false, Some("xml"), String::new()),
            Purpose::ExportXml => (false, true, Some("xml"), "cratebox-rekordbox.xml".into()),
            Purpose::ImportMapping => (false, false, Some("json"), String::new()),
            Purpose::ExportMapping => (false, true, Some("json"), "cratebox-mapping.json".into()),
        };
        let dir = start.map(|p| p.to_path_buf()).filter(|p| p.is_dir()).unwrap_or_else(|| {
            if purpose == Purpose::OpenUsb { mounts().into_iter().next().map(|p| p.parent().map(|x| x.to_path_buf()).unwrap_or(p)).unwrap_or_else(home) }
            else if purpose == Purpose::AddFolder { let m = home().join("Music"); if m.is_dir() { m } else { home() } }
            else { home() }
        });
        let mut d = FileDialog { purpose, dir: dir.clone(), name, pick_dir, save, ext, entries: vec![], path_text: String::new(), sel: None, err: String::new() };
        d.go(dir);
        d
    }
    fn title(&self) -> &'static str {
        match self.purpose {
            Purpose::AddFolder => "Add a music folder", Purpose::OpenUsb => "Open a rekordbox USB stick or SD card",
            Purpose::ImportXml => "Import rekordbox XML", Purpose::ExportXml => "Export rekordbox XML",
            Purpose::ImportMapping => "Import a controller mapping", Purpose::ExportMapping => "Save the controller mapping",
        }
    }
    fn go(&mut self, d: PathBuf) {
        match std::fs::read_dir(&d) {
            Ok(rd) => {
                let mut v: Vec<(String, bool)> = rd.flatten().filter_map(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    if n.starts_with('.') { return None; }
                    let is_dir = e.path().is_dir();
                    if !is_dir && (self.pick_dir || self.ext.map(|x| !n.to_lowercase().ends_with(&format!(".{x}"))).unwrap_or(false)) { return None; }
                    Some((n, is_dir))
                }).collect();
                v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
                self.entries = v;
                self.dir = d;
                self.path_text = self.dir.display().to_string();
                self.sel = None;
                self.err.clear();
            }
            Err(e) => self.err = format!("Can't open {}: {e}", d.display()),
        }
    }
    /// Some(Some(path)) = chosen, Some(None) = cancelled, None = still open
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Option<PathBuf>> {
        let mut result = None;
        let mut open = true;
        egui::Window::new(self.title()).open(&mut open).collapsible(false).resizable(true).default_size([620.0, 460.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("⬆ Up").clicked() { if let Some(p) = self.dir.parent() { let p = p.to_path_buf(); self.go(p); } }
                if ui.button("Home").clicked() { self.go(home()); }
                let m = home().join("Music");
                if m.is_dir() && ui.button("Music").clicked() { self.go(m); }
                for mnt in mounts() {
                    let label = format!("⏏ {}", mnt.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
                    if ui.button(label).on_hover_text(mnt.display().to_string()).clicked() { self.go(mnt); }
                }
            });
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut self.path_text).desired_width(f32::INFINITY));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) { let p = PathBuf::from(self.path_text.trim()); if p.is_dir() { self.go(p); } }
            });
            if !self.err.is_empty() { ui.colored_label(egui::Color32::from_rgb(255, 110, 110), &self.err); }
            let list_h = (ui.available_height() - 64.0).max(120.0);
            egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
                let mut go_to = None;
                let mut pick_file = None;
                for (n, is_dir) in &self.entries {
                    let label = if *is_dir { format!("📁 {n}") } else { format!("   {n}") };
                    let r = ui.selectable_label(self.sel.as_deref() == Some(n.as_str()), label);
                    if r.clicked() { self.sel = Some(n.clone()); if !*is_dir && self.save { self.name = n.clone(); } }
                    if r.double_clicked() { if *is_dir { go_to = Some(self.dir.join(n)); } else { pick_file = Some(self.dir.join(n)); } }
                }
                if let Some(p) = go_to { self.go(p); }
                if let Some(p) = pick_file { if !self.save { result = Some(Some(p)); } }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if self.save { ui.label("Name:"); ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(260.0)); }
                let hint = if self.purpose == Purpose::OpenUsb { "Pick the drive itself (the one with the PIONEER folder)" } else if self.pick_dir { "Open the folder, or select it, then Choose" } else { "" };
                if !hint.is_empty() { ui.weak(hint); }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok_label = if self.save { "Save" } else if self.pick_dir { "Choose" } else { "Open" };
                    if ui.button(ok_label).clicked() {
                        let target = self.sel.as_ref().map(|s| self.dir.join(s));
                        if self.pick_dir {
                            result = Some(Some(target.filter(|p| p.is_dir()).unwrap_or_else(|| self.dir.clone())));
                        } else if self.save {
                            if !self.name.trim().is_empty() { result = Some(Some(self.dir.join(self.name.trim()))); }
                        } else if let Some(t) = target.filter(|p| p.is_file()) { result = Some(Some(t)); }
                    }
                    if ui.button("Cancel").clicked() { result = Some(None); }
                });
            });
        });
        if !open { result = Some(None); }
        result
    }
}
