//! The window, laid out like an XDJ's screen: a slim header, the scrolling waveforms (2 or 4 decks) with
//! each deck's numbers above its waveform and the whole track below it, then the decks' controls either
//! side of the mixer (channels 3 1 2 4 in 4-deck mode, the BEAT FX unit underneath), then the library.
//! WAVE mode shows just the waveforms. Sized for 1152×720 points (a MacBook10,1 at 2x) and up.

use crate::app::{level, App};
use crate::deck::{PadMode, JUMPS, LOOP_SIZES, RANGES, JogKind};
use crate::dialog::{FileDialog, Purpose};
use crate::fx::{frac_text, spec, FxKind, PAD_FX};
use crate::keys::{camelot, keys_match};
use crate::library::{SortKey, Track};
use crate::midi;
use crate::mixer::DECKS;
use crate::playlists::{row_is_view, Lists, RowKind, View};
use crate::widgets::*;
use eframe::egui::{self, pos2, vec2, Align, Color32, Key, Layout, Rect, Response, Sense, Stroke, StrokeKind, Ui, UiBuilder};
use std::path::PathBuf;

/// what's being dragged out of the track list
#[derive(Clone)]
pub struct DragRows { pub ids: Vec<String>, pub positions: Option<Vec<usize>> }
/// a playlist/folder being dragged in the tree
#[derive(Clone)]
pub struct DragList(pub String);

pub struct Gui {
    pub app: App,
    deck_rects: [Rect; DECKS],
    jog_last: [Option<f32>; DECKS],
    lane_drag: [Option<(f32, f32, std::time::Instant, bool, bool)>; DECKS],
    /// each lane's waveform tiles on the GPU (widgets::WaveCache)
    lane_cache: [WaveCache; DECKS],
    /// a PAD FX pad held with the mouse (deck, pad)
    pad_held: Option<(usize, usize)>,
    pub frames: u64,
    pub test_shot: Option<(PathBuf, u64)>,
    /// named on-screen controls (for the scripted UI tests)
    pub rects: std::collections::HashMap<String, Rect>,
    pub uitest: Option<crate::uitest::UiTest>,
    heat_open: bool,
    /// the OFF menu (exit to the login screen, restart, shut down) and the action waiting for its confirm
    power_open: bool,
    sys: SysStats,
    /// last battery read (every 15 s, 5 s when low): percent, charging
    battery: (std::time::Instant, Option<(u32, bool)>),
    /// battery alerts given since the last charge: 5 = low (OK), 3 = critical (stopped + ejected)
    bat_alerted: u32,
    /// the battery alert on screen (5 or 3)
    bat_dialog: Option<u32>,
    /// WAVE mode: the pop-up library (opened by scrolling, last activity) and the selection it last saw
    wave_browse: Option<std::time::Instant>,
    last_sel: usize,
    power_confirm: Option<&'static str>,
    /// HEAT panel: the paste box, and the simulator's "hold to hit" being held
    heat_paste: String,
    hitting: bool,
    /// Ctrl+F: focus the search box next frame; F12: the shortcuts window
    focus_search: bool,
    keys_open: bool,
    /// `cratebox --cmd …` lines (the OS's Command-key shortcuts)
    remote: Option<std::sync::mpsc::Receiver<String>>,
    /// `app.loads` when last looked at
    seen_loads: u64,
    /// the PLAYLISTS column: track id → playlist names, and the lists' fingerprint it was built from
    pl_names: (u64, std::collections::HashMap<String, String>),
}

const ROW: f32 = 22.0;
const GAP: f32 = 4.0;

/// a child Ui laid out left to right inside `r`
fn row_ui(ui: &mut Ui, r: Rect) -> Ui {
    let mut c = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::left_to_right(Align::Center)));
    c.spacing_mut().item_spacing = vec2(GAP, GAP);
    c
}
/// the channel order on the mixer: 3 1 2 4 with four decks, like a four-channel DJM
pub fn mixer_order(n: usize) -> Vec<usize> { match n { 4 => vec![2, 0, 1, 3], 3 => vec![2, 0, 1], _ => vec![0, 1] } }

impl Gui {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Gui {
        let mut app = App::new();
        set_theme(&app.store.settings.theme);
        style(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let wake: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || ctx.request_repaint());
        app.repaint = Some(wake.clone());
        app.midi_io.set_wake(wake.clone());
        app.watch_drives();
        let remote = crate::remote::listen(move || wake());
        Gui { app, deck_rects: [Rect::NOTHING; DECKS], jog_last: [None; DECKS], lane_drag: [None; DECKS], lane_cache: Default::default(), pad_held: None, frames: 0, test_shot: None,
            rects: Default::default(), uitest: crate::uitest::UiTest::from_env(), heat_open: false, power_open: false, sys: SysStats::new(), battery: (std::time::Instant::now().checked_sub(std::time::Duration::from_secs(60)).unwrap_or_else(std::time::Instant::now), read_battery()), power_confirm: None, bat_alerted: 100, bat_dialog: None, wave_browse: None, last_sel: usize::MAX, heat_paste: String::new(), hitting: false, focus_search: false, keys_open: false, remote, seen_loads: 0, pl_names: (0, Default::default()) }
    }

    /// in learn mode a click on a mappable control picks it; returns true if it was used up that way
    fn reg(&mut self, name: impl Into<String>, r: Rect) { self.rects.insert(name.into(), r); }
    fn learn(&mut self, ui: &Ui, r: &Response, action: &str, deck: i8) -> bool {
        let key = if deck >= 0 { format!("deck{deck}.{action}") } else { action.to_string() };
        self.rects.insert(key, r.rect);
        if !self.app.learning() { return false; }
        let picked = self.app.ui.learn_pick.as_ref().is_some_and(|(a, d)| a == action && *d == deck);
        ui.painter().rect_stroke(r.rect.expand(2.0), 3.0, Stroke::new(if picked { 2.0_f32 } else { 1.0 }, if picked { LOOP } else { Color32::from_rgba_unmultiplied(245, 214, 74, 70) }), StrokeKind::Outside);
        if r.clicked() || r.drag_started() { self.app.learn_pick(action, deck); }
        true
    }
    /// a pulsing outline in the deck's colour around a deck the controller's hands are on (4 decks only)
    fn select_glow(&self, ui: &Ui, i: usize, r: Rect) {
        if self.side_of(i).is_none() { return; }
        let col = deck_color(i);
        let a = (strobe(ui.ctx()) * 255.0) as u8;
        ui.painter().rect_stroke(r, 4.0, Stroke::new(2.0_f32, Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), a)), StrokeKind::Inside);
    }
    /// the controller side (0 left / 1 right) playing deck `i` in 4-deck mode
    fn side_of(&self, i: usize) -> Option<usize> {
        if !self.app.four() || self.app.midi_io.name.is_empty() { return None; }
        self.app.midi.side.iter().position(|&d| d == i)
    }

    // ===================================================================== header
    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(egui::RichText::new("CRATE").font(bold(17.0)).color(FG()));
            ui.label(egui::RichText::new("BOX").font(bold(17.0)).color(deck_color(0)));
            ui.spacing_mut().item_spacing.x = 5.0;
            ui.add_space(8.0);
            // narrow window (the MacBook's 1152 pt): short pills, and the status line floats under the header
            let compact = ui.available_width() < 1240.0;
            // controller pill
            let on = !self.app.midi_io.name.is_empty();
            let name = if on { self.app.midi_io.name.split(':').next().unwrap_or("").to_string() } else { self.app.midi_io.error.clone().map(|_| "MIDI unavailable".to_string()).unwrap_or_else(|| "No controller".into()) };
            let name = if compact { if on { name.chars().take(10).collect() } else { "MIDI".to_string() } } else { name };
            pill(ui, &name, on).on_hover_text(self.app.midi_io.error.clone().unwrap_or_default());
            let ch = self.app.audio.channels;
            let out = if self.app.audio.running() { if self.app.audio.device.is_empty() { "Default output".to_string() } else { short_dev(&self.app.audio.device) } } else { "No sound output".into() };
            let out = if compact { if self.app.audio.running() { out.chars().take(10).collect() } else { "NO SOUND".to_string() } } else { out };
            pill(ui, &format!("{out} · {ch} ch"), self.app.audio.running() && ch >= 4).on_hover_text(&self.app.hp_note);
            ui.add_space(4.0);
            let st = self.app.status_text().to_string();
            let right_w = 470.0 + 40.0 + 130.0 + 110.0 + 40.0 + 140.0 + if self.battery.1.is_some() { 130.0 } else { 0.0 };
            if compact {
                if !st.is_empty() {
                    let top = ui.max_rect().bottom() + 6.0;
                    egui::Area::new(egui::Id::new("status_toast")).order(egui::Order::Foreground).interactable(false)
                        .anchor(egui::Align2::CENTER_TOP, vec2(0.0, top)).show(ui.ctx(), |ui| {
                            egui::Frame::new().fill(PANEL_D()).corner_radius(4.0).inner_margin(egui::Margin::symmetric(10, 4))
                                .show(ui, |ui| ui.label(egui::RichText::new(&st).color(FG()).size(11.5)));
                        });
                }
                ui.add_space((ui.available_width() - right_w).max(0.0));
            } else {
                ui.add_sized(vec2((ui.available_width() - right_w).max(40.0), 20.0), egui::Label::new(egui::RichText::new(st).color(MUTED()).size(11.5)).truncate());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // power, in the far right corner
                let r = button(ui, "", vec2(26.0, 22.0), self.power_open, RED, 11.0).on_hover_text("Exit to the login screen, restart or shut down");
                paint_power(ui.painter(), r.rect, if self.power_open { BG() } else { RED });
                self.reg("power", r.rect);
                if r.clicked() { self.power_open = !self.power_open; self.power_confirm = None; }
                ui.add_space(8.0);
                let lv = [level(self.app.status_snap.master_meter[0]), level(self.app.status_snap.master_meter[1])];
                meter(ui, vec2(5.0, 24.0), lv[1]);
                meter(ui, vec2(5.0, 24.0), lv[0]);
                let mut m = self.app.mixer.master;
                let frozen = self.app.learning();
                let r = knob(ui, &mut m, "", FG(), 0.8, 22.0, frozen);
                if !self.learn(ui, &r, "master", -1) && r.changed() { self.app.mixer.master = m; self.app.send_mixer(); }
                ui.label(egui::RichText::new(db_text(crate::engine::master_level_gain(self.app.mixer.master as f64))).font(mono(10.0)).color(FG()));
                ui.label(egui::RichText::new("MASTER").size(9.5).color(MUTED()));
                ui.add_space(6.0);
                let r = button(ui, "⚙", vec2(26.0, 22.0), self.app.ui.settings_open, deck_color(0), 13.0).on_hover_text("Settings");
                self.reg("settings", r.rect);
                if r.clicked() { self.app.ui.settings_open = !self.app.ui.settings_open; }
                let hl = self.app.heat.label();
                let lit = self.app.heat.lit();
                let w = ui.painter().layout_no_wrap(hl.clone(), prop(11.5), FG()).size().x + 26.0;
                let r = button(ui, &format!("{hl} ▾"), vec2(w, 22.0), lit, CUE, 11.5).on_hover_text("TrueCurve heat curves for a Puffco Peak Pro / Proxy");
                self.reg("heat", r.rect);
                if r.clicked() { self.heat_open = !self.heat_open; }
                let n = self.app.deck_count();
                let r = button(ui, &format!("{n} DECKS"), vec2(62.0, 22.0), n > 2, FG(), 11.5)
                    .on_hover_text("2 → 3 → 4 decks. 3: decks 1 and 3 share the left side (layer button / SMART CFX), 2 on the right. 4: 3 and 4 on the outer channels");
                self.reg("four", r.rect);
                if r.clicked() { self.app.set_decks(if n == 4 { 2 } else { n + 1 }); }
                let wave = self.app.store.settings.wave;
                let r = icon_button(ui, Icon::Wave, wave, deck_color(2)).on_hover_text("WAVE: just the waveforms (F1)");
                self.reg("wave", r.rect);
                if r.clicked() { self.app.store.settings.wave = !wave; self.app.store.dirty_settings = true; }
                let lib = !self.app.store.settings.lib_hidden;
                let r = icon_button(ui, Icon::List, lib, FG()).on_hover_text("Library under the decks: show or hide (hidden: scroll to pop it up)");
                self.reg("lib", r.rect);
                if r.clicked() { self.app.store.settings.lib_hidden = lib; self.app.store.dirty_settings = true; }
                // quick launchers: Chrome (cratebox-web, from os-files) and the file manager
                let r = icon_button(ui, Icon::Globe, false, FG()).on_hover_text("WEB: Chrome, downloads go straight into the collection");
                self.reg("web", r.rect);
                if r.clicked() {
                    // nothing launched: say why (the helper comes with the app update)
                    if let Err(e) = std::process::Command::new("cratebox-web").spawn() {
                        self.app.say(&format!("Chrome didn't start ({e}). Run Update Cratebox from the Cratebox OS menu to get the helper."), 10.0);
                    }
                }
                let r = icon_button(ui, Icon::Folder, false, FG()).on_hover_text("FILES: open the file manager");
                self.reg("files", r.rect);
                if r.clicked() {
                    let home = dirs::home_dir().unwrap_or_default();
                    let _ = std::process::Command::new("pcmanfm-qt").arg(&home).spawn().or_else(|_| std::process::Command::new("xdg-open").arg(&home).spawn());
                }
                let rec = self.app.rec.as_ref().map(|r| r.seconds());
                let armed = self.app.rec_armed;
                let label = match rec { Some(s) => format!("● {}:{:02}", s as u64 / 60, s as u64 % 60), None if armed => "● ARMED".into(), None => "● REC".into() };
                if armed { ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)); }
                let lit = rec.is_some() || (armed && (ui.ctx().input(|i| i.time) * 2.0).fract() < 0.5);
                let r = button(ui, &label, vec2(62.0, 22.0), lit, RED, 11.5).on_hover_text("Record the master to a WAV file (Settings → Sound says where)");
                self.reg("rec", r.rect);
                if r.clicked() { self.app.toggle_rec(); }
                // battery, left of REC: red "PLUG IN" at 20 %, blinking at 10 % (on battery only; alerts at 5 and 3 %)
                if let Some((pc, charging)) = self.battery.1 {
                    // ▲ charging, ▼ on battery
                    let (txt, c) = if charging { (format!("{pc}% ▲"), Color32::from_rgb(0x5c, 0xd6, 0x7a)) }
                        else if pc <= 20 { (format!("PLUG IN {pc}% ▼"), RED) }
                        else { (format!("{pc}% ▼"), MUTED()) };
                    let blink = !charging && pc <= 10;
                    let t = ui.ctx().input(|i| i.time);
                    if blink { ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)); }
                    let c = if blink && (t * 2.0).fract() > 0.5 { Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 40) } else { c };
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(txt).font(bold(11.5)).color(c)).on_hover_text(if charging { "Charging" } else { "On battery" });
                }
                // CPU (whole machine, %) and memory in use (MB), every 2 s
                if self.sys.at.elapsed().as_secs_f32() >= 2.0 { self.sys.sample(); }
                ui.add_space(4.0);
                // two small lines: CPU + memory over network rates, Wi-Fi signal and Bluetooth
                let (wt, wc) = match self.sys.wifi { Some(q) => (format!("WI-FI {q}%"), if q < 30 { RED } else { MUTED() }), None => ("WI-FI OFF".into(), RED) };
                let mut l1 = format!("CPU {:.0}% · {} MB · BT {}", self.sys.cpu, self.sys.mem_mb, if self.sys.bt { "ON" } else { "OFF" });
                if self.app.store.settings.show_fps { if let Ok(f) = FRAME_STATS.lock() { if !f.is_empty() { l1 = format!("{f} · {l1}"); } } }
                let l2 = format!("↓{} ↑{} · {wt}", rate_text(self.sys.down), rate_text(self.sys.up));
                let (c1, c2) = (if self.sys.cpu >= 85.0 || !self.sys.bt { RED } else { MUTED() }, wc);
                ui.with_layout(Layout::top_down(Align::Max), |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add(egui::Label::new(egui::RichText::new(l1).font(mono(9.5)).color(c1)).extend()).on_hover_text("CPU use of the whole computer, memory in use, Bluetooth (HEAT needs it)");
                    ui.add(egui::Label::new(egui::RichText::new(l2).font(mono(9.5)).color(c2)).extend()).on_hover_text("Network download / upload (all connections) and Wi-Fi signal");
                });
            });
        });
    }

    // ===================================================================== waveforms
    /// one deck's waveform block: the numbers strip, the scrolling waveform, the whole-track overview
    fn lane(&mut self, ui: &mut Ui, i: usize, rect: Rect) {
        let info_h = if rect.height() > 90.0 { 18.0 } else { 15.0 };
        let ov_h = (rect.height() * 0.12).clamp(7.0, 16.0);
        let info = Rect::from_min_size(rect.min, vec2(rect.width(), info_h));
        let wave = Rect::from_min_max(pos2(rect.left(), info.bottom()), pos2(rect.right(), rect.bottom() - ov_h - 1.0));
        let ov = Rect::from_min_max(pos2(rect.left(), wave.bottom() + 1.0), rect.max);
        self.info_strip(ui, i, info);
        self.select_glow(ui, i, rect);
        // ---- the scrolling waveform: drag to move through the track (audio follows the hand while playing), click to jump
        let r = ui.interact(wave, ui.id().with(("lane", i)), Sense::click_and_drag());
        self.reg(format!("lane{i}"), wave);
        let zoom = self.app.store.settings.zoom;
        let rgb = self.app.store.settings.wave_style != "3band";
        paint_lane(ui.painter(), wave, &self.app.decks[i], i, zoom, rgb, &mut self.lane_cache[i]);
        if let Some(s) = self.side_of(i) {
            let b = Rect::from_min_size(pos2(wave.left() + 8.0, wave.top() + 4.0), vec2(16.0, 14.0));
            ui.painter().rect_filled(b, 3.0, deck_color(i));
            ui.painter().text(b.center(), egui::Align2::CENTER_CENTER, if s == 0 { "L" } else { "R" }, bold(10.0), BG());
        }
        // the whole track: click to jump, drag to scrub
        let or = ui.interact(ov, ui.id().with(("ov", i)), Sense::click_and_drag());
        self.reg(format!("deck{i}.overview"), ov);
        paint_overview(ui.painter(), ov, &self.app.decks[i], rgb);
        let frozen = self.app.learning();
        if !frozen && (or.dragged() || or.clicked() || or.drag_started()) {
            if let Some(p) = or.interact_pointer_pos() {
                let d = &mut self.app.decks[i];
                if d.loaded() { let f = ((p.x - ov.left()) / ov.width()).clamp(0.0, 1.0) as f64; let dur = d.duration; d.seek_to(f * dur); }
            }
        }
        let d = &mut self.app.decks[i];
        if !d.loaded() || frozen { return; }
        let pps = wave.width() as f64 / zoom / d.rate().max(0.05);
        if r.hovered() {
            // two-finger scroll: up/down zooms (smoothly: a trackpad sends many small steps), sideways scrubs while paused
            let sd = ui.input(|i| i.smooth_scroll_delta);
            if sd.y != 0.0 && sd.y.abs() >= sd.x.abs() { let z = &mut self.app.store.settings.zoom; *z = (*z * (-sd.y as f64 / 200.0).exp()).clamp(1.0, 32.0); }
            let d = &mut self.app.decks[i];
            if sd.x != 0.0 && sd.x.abs() > sd.y.abs() && !d.playing { let t = (d.now() - sd.x as f64 / pps).clamp(0.0, d.duration); d.seek(t); }
        }
        let d = &mut self.app.decks[i];
        if r.drag_started() {
            if let Some(p) = r.interact_pointer_pos() { self.lane_drag[i] = Some((p.x, p.x, std::time::Instant::now(), false, d.playing)); }
        }
        if let Some((x0, last, t0, mut moved, scratch)) = self.lane_drag[i] {
            if let Some(p) = r.interact_pointer_pos() {
                let dx = p.x - last;
                if !moved && (p.x - x0).abs() >= 4.0 { moved = true; if scratch { d.jog_touch(true, false); } }
                if moved && dx != 0.0 {
                    let dt = -(dx as f64) / pps; // drag left = later in the track
                    if scratch { d.jog_ticks(dt / crate::deck::TICK_SEC, JogKind::Scratch); } else { let t = (d.now() + dt).clamp(0.0, d.duration); d.seek(t); }
                }
                self.lane_drag[i] = Some((x0, p.x, t0, moved, scratch));
            }
            if r.drag_stopped() || !r.is_pointer_button_down_on() {
                if moved && scratch { d.jog_touch(false, true); }
                self.lane_drag[i] = None;
            }
        }
        if r.clicked() {
            if let Some(p) = r.interact_pointer_pos() { let t = (d.now() + (p.x - wave.center().x) as f64 / pps).clamp(0.0, d.duration); d.seek_to(t); }
        }
    }

    /// XDJ-style numbers over a waveform: deck, title, then phrase, beat, key, BPM, tempo, time left
    fn info_strip(&mut self, ui: &mut Ui, i: usize, r: Rect) {
        let p = ui.painter().with_clip_rect(r);
        let col = deck_color(i);
        let d = &self.app.decks[i];
        let active = self.app.deck_active(i);
        p.rect_filled(r, 0.0, PANEL_D());
        let fs = (r.height() - 4.0).min(13.0);
        let badge = Rect::from_min_size(r.min + vec2(2.0, 1.0), vec2(r.height() + 2.0, r.height() - 2.0));
        p.rect_filled(badge, 3.0, if active { col } else { DIM() });
        p.text(badge.center(), egui::Align2::CENTER_CENTER, format!("{}", i + 1), bold(fs), BG());
        // right side, laid out right to left
        let mut x = r.right() - 6.0;
        let cy = r.center().y;
        let put = |x: &mut f32, p: &egui::Painter, text: String, font: egui::FontId, c: Color32, gap: f32| {
            let g = p.layout_no_wrap(text, font, c);
            let w = g.size().x;
            p.galley(pos2(*x - w, cy - g.size().y / 2.0), g, c);
            *x -= w + gap;
        };
        let t = d.now();
        if d.loaded() {
            put(&mut x, &p, format!("-{}", fmt_time(d.duration - t)), mono(fs), FG(), 10.0);
            let rg = d.range();
            put(&mut x, &p, format!("{:+.2}% {}", (d.rate() - 1.0) * 100.0, if rg == 100.0 { "W".into() } else { format!("±{rg}") }), mono(fs - 1.5), MUTED(), 8.0);
            put(&mut x, &p, if d.bpm() > 0.0 { format!("{:.2}", d.eff_bpm()) } else { "--.--".into() }, mono(fs + 1.0), FG(), 8.0);
            let k = d.key_now();
            if let Some(c) = camelot(k) { put(&mut x, &p, c.label(), bold(fs), rgb(c.color()), 8.0); }
            let master = self.app.tempo_master() == Some(i);
            for (on, txt, c) in [(d.mt, "MT", col), (master, "MASTER", CUE), (d.sync, "SYNC", col)] {
                if on { put(&mut x, &p, txt.to_string(), bold(fs - 3.0), c, 5.0); }
            }
            if let Some((txt, c)) = phase_tag(d) { put(&mut x, &p, txt, mono(fs - 2.0), c, 6.0); }
            if d.ana.is_some() {
                let beat = d.beat_at(t).floor().rem_euclid(4.0) as i32;
                x -= 3.0;
                for k in (0..4).rev() {
                    p.circle_filled(pos2(x - 3.0, cy), 2.6, if k == beat { if k == 0 { RED } else { col } } else { LINE2() });
                    x -= 8.0;
                }
                x -= 4.0;
                if let Some((s, _e, bars, label, bar)) = d.phrase_spans().into_iter().find(|ps| t >= ps.0 && t < ps.1) {
                    let n = (((t - s) / bar).floor() as u32 + 1).min(bars);
                    put(&mut x, &p, format!("{} {}/{}", label.to_uppercase(), n, bars), bold(fs - 2.5), rgb(crate::analysis::phrase_color(&label)), 8.0);
                }
            }
        }
        let (title, artist) = match &d.track {
            Some(t) => (format!("{}{}", if d.loading { "Loading… " } else { "" }, t.title), t.artist.clone()),
            None if !active => ("Hidden: turn on 4 DECKS".into(), String::new()),
            None => ("No track".into(), String::new()),
        };
        let left = badge.right() + 6.0;
        let tp = p.with_clip_rect(Rect::from_min_max(pos2(left, r.top()), pos2(x.max(left), r.bottom())));
        let g = tp.layout_no_wrap(title, bold(fs), if d.track.is_some() { FG() } else { DIM() });
        let tw = g.size().x;
        tp.galley(pos2(left, cy - g.size().y / 2.0), g, FG());
        if !artist.is_empty() { tp.text(pos2(left + tw + 8.0, cy), egui::Align2::LEFT_CENTER, artist, prop(fs - 1.5), MUTED()); }
    }

    // ===================================================================== a deck's controls
    fn deck(&mut self, ui: &mut Ui, i: usize, rect: Rect) {
        let col = deck_color(i);
        let p = ui.painter();
        p.rect_filled(rect, 6.0, PANEL_D());
        p.rect_stroke(rect, 6.0, Stroke::new(1.0_f32, LINE()), StrokeKind::Inside);
        // the deck the controller is on (4-deck mode) gets an outline
        if self.side_of(i).is_some() { p.rect_stroke(rect, 6.0, Stroke::new(1.5_f32, col), StrokeKind::Inside); }
        // drop a track on the deck
        let resp = ui.interact(rect, ui.id().with(("deckdrop", i)), Sense::hover());
        if let Some(pl) = resp.dnd_release_payload::<DragRows>() { if let Some(id) = pl.ids.first() { let id = id.clone(); self.app.load_id(i, &id); } }
        if resp.dnd_hover_payload::<DragRows>().is_some() { ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0_f32, col), StrokeKind::Inside); }
        self.deck_rects[i] = rect;
        self.reg(format!("deck{i}.area"), rect);
        let inner = rect.shrink2(vec2(8.0, 7.0));
        if self.screens() { self.deck_display(ui, i, inner) }
        else if inner.height() >= 200.0 { self.deck_tall(ui, i, inner) } else { self.deck_compact(ui, i, inner) }
    }

    /// two decks, XDJ style: tempo fader on the outside edge; a screen (platter, time, key, BPM, beat counter);
    /// SYNC MT Q TAP GRID; wide CUE and PLAY; the loop row; the pad-mode tabs and two rows of pads
    fn deck_tall(&mut self, ui: &mut Ui, i: usize, inner: Rect) {
        let h = inner.height();
        let (rh, font) = (24.0, 11.5);
        let tempo_w = 24.0;
        let outer_left = i % 2 == 0;
        let tempo_r = if outer_left { Rect::from_min_size(inner.min, vec2(tempo_w, h)) } else { Rect::from_min_size(pos2(inner.right() - tempo_w, inner.top()), vec2(tempo_w, h)) };
        let area = if outer_left { Rect::from_min_max(pos2(tempo_r.right() + 8.0, inner.top()), inner.max) } else { Rect::from_min_max(inner.min, pos2(tempo_r.left() - 8.0, inner.bottom())) };
        self.tempo(ui, i, tempo_r, rh, font);
        let w = area.width();
        let cb = 34.0;
        let pad_h = ((h - 4.0 * rh - cb) * 0.2).clamp(24.0, 38.0);
        let pads_h = rh + 2.0 * pad_h + 2.0 * GAP;
        let screen_h = (h - pads_h - 2.0 * rh - cb - 4.0 * GAP).max(60.0);
        let screen = Rect::from_min_size(area.min, vec2(w, screen_h));
        self.deck_screen(ui, i, screen, outer_left);
        let r_ctl = Rect::from_min_size(pos2(area.left(), screen.bottom() + GAP), vec2(w, rh));
        self.ctl_row(ui, i, r_ctl, rh, font);
        // CUE and PLAY side by side, full width (the play side towards the mixer, like a CDJ's)
        let r_cp = Rect::from_min_size(pos2(area.left(), r_ctl.bottom() + GAP), vec2(w, cb));
        let half = (w - GAP) / 2.0;
        let cue_r = Rect::from_min_size(r_cp.min, vec2(half, cb));
        let play_r = Rect::from_min_size(pos2(r_cp.left() + half + GAP, r_cp.top()), vec2(half, cb));
        self.cue_play(ui, i, cue_r, play_r, cb);
        let r_loop = Rect::from_min_size(pos2(area.left(), r_cp.bottom() + GAP), vec2(w, rh));
        self.loop_row(ui, i, r_loop, rh, font);
        let rest = Rect::from_min_max(pos2(area.left(), r_loop.bottom() + GAP), area.max);
        self.pads(ui, i, rest, rh, font);
    }

    /// SYNC MT Q TAP GRID and the loop length, in one row
    fn ctl_row(&mut self, ui: &mut Ui, i: usize, r_ctl: Rect, rh: f32, font: f32) {
        let (col, w) = (deck_color(i), r_ctl.width());
        {
            let ui = &mut row_ui(ui, r_ctl);
            let d = &self.app.decks[i];
            let (sync, mt, q, g) = (d.sync, d.mt, d.quant, self.app.ui.grid_open[i]);
            let u = (w - 5.0 * GAP) / 6.4;
            let r = button(ui, "SYNC", vec2(u * 1.4, rh), sync, col, font).on_hover_text("Sync to the master. Right-click (or hold SYNC on the controller): make this deck the master");
            if !self.learn(ui, &r, "sync", i as i8) { if r.clicked() { self.app.set_sync(i, !sync); } else if r.secondary_clicked() { self.app.make_master(i); } }
            let r = button(ui, "MT", vec2(u, rh), mt, col, font).on_hover_text("Master tempo (keylock): change tempo without changing the key");
            if !self.learn(ui, &r, "keylock", i as i8) && r.clicked() { self.app.set_mt(i, !mt); }
            let r = button(ui, "Q", vec2(u, rh), q, col, font).on_hover_text("Quantize: snap cues and loops to the beat grid");
            if !self.learn(ui, &r, "quant", i as i8) && r.clicked() { self.app.decks[i].quant = !q; }
            if button(ui, "TAP", vec2(u, rh), false, col, font).on_hover_text("Tap along with the beat (4+ taps) to set the BPM").clicked() { self.app.decks[i].tap(); }
            let r = button(ui, "GRID", vec2(u, rh), g, col, font).on_hover_text("Edit the beat grid");
            self.reg(format!("deck{i}.grid"), r.rect);
            if r.clicked() { self.app.ui.grid_open[i] = !g; }
            self.loop_len(ui, i, vec2(u, rh), font);
        }
    }

    /// on-screen buttons or only screens: Settings → Decks (auto = screens while a controller is connected)
    fn screens(&self) -> bool {
        match self.app.store.settings.deck_view.as_str() { "screens" => true, "controls" => false, _ => !self.app.midi_io.name.is_empty() }
    }

    /// a deck played from the controller: the screen fills the deck, with the hot cues (click to jump) and
    /// the pad mode and loop under it
    fn deck_display(&mut self, ui: &mut Ui, i: usize, inner: Rect) {
        let col = deck_color(i);
        let rh = if inner.height() > 200.0 { 24.0 } else { 18.0 };
        let font = if rh > 20.0 { 11.5 } else { 10.0 };
        // two decks: room for a SYNC / Q / GRID row; the grid editor takes the hot cue row while it's open
        let tall = inner.height() > 200.0;
        let rows = if tall { 3.0 } else { 2.0 };
        let screen = Rect::from_min_max(inner.min, pos2(inner.right(), inner.bottom() - rows * (rh + GAP)));
        self.deck_screen(ui, i, screen, i % 2 == 0);
        let mut y = screen.bottom() + GAP;
        if tall {
            self.ctl_row(ui, i, Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), rh)), rh, font);
            y += rh + GAP;
        }
        let r_hot = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), rh));
        if self.app.ui.grid_open[i] {
            // the editor's buttons wrap onto two rows: it takes the status line too
            self.grid_bar(ui, i, Rect::from_min_size(r_hot.min, vec2(inner.width(), 2.0 * rh + GAP)), rh, font);
            return;
        }
        {
            let ui = &mut row_ui(ui, r_hot);
            let u = (inner.width() - 7.0 * GAP) / 8.0;
            for k in 0..8 {
                let set = self.app.decks[i].hot[k].is_some();
                let r = button(ui, &((b'A' + k as u8) as char).to_string(), vec2(u, rh), set, col, font).on_hover_text("Hot cue: click to jump (or set it here)");
                if r.clicked() { self.app.decks[i].hotcue(k); }
            }
        }
        let r_st = Rect::from_min_size(pos2(inner.left(), r_hot.bottom() + GAP), vec2(inner.width(), rh));
        let p = ui.painter();
        p.rect_filled(r_st, 4.0, Color32::from_rgb(0x07, 0x09, 0x0c));
        let d = &self.app.decks[i];
        let mode = match d.pad_mode { PadMode::HotCue => "HOT CUE", PadMode::BeatLoop => "BEAT LOOP", PadMode::BeatJump => "BEAT JUMP", PadMode::PadFx => "PAD FX 1", PadMode::PadFx2 => "PAD FX 2" };
        p.text(pos2(r_st.left() + 8.0, r_st.center().y), egui::Align2::LEFT_CENTER, mode, bold(font), col);
        let lp = match (d.lp.a, d.lp.b) { (None, _) => String::new(), (Some(_), None) => "LOOP IN…".into(), _ => format!("LOOP {}{}", frac_text(d.loop_beats()), if d.lp.on { "" } else { " (off)" }) };
        if !lp.is_empty() { p.text(r_st.center(), egui::Align2::CENTER_CENTER, lp, bold(font), if d.lp.on { LOOP } else { MUTED() }); }
        let q = format!("{}{}", if d.quant { "Q " } else { "" }, if self.app.ui.grid_open[i] { "GRID" } else { "" });
        p.text(pos2(r_st.right() - 8.0, r_st.center().y), egui::Align2::RIGHT_CENTER, q, bold(font), MUTED());
    }

    /// the deck's screen: a small platter (drag to scratch) on the outside, then time, key and BPM in big
    /// numbers, with the tempo, the sync state and a beat / phrase counter under them
    fn deck_screen(&mut self, ui: &mut Ui, i: usize, r: Rect, platter_left: bool) {
        let col = deck_color(i);
        ui.painter().rect_filled(r, 5.0, Color32::from_rgb(0x07, 0x09, 0x0c));
        ui.painter().rect_stroke(r, 5.0, Stroke::new(1.0_f32, LINE()), StrokeKind::Inside);
        self.select_glow(ui, i, r);
        let jd = (r.height() - 12.0).min(r.width() * 0.3).max(40.0);
        let jr = if platter_left { Rect::from_min_size(pos2(r.left() + 6.0, r.center().y - jd / 2.0), vec2(jd, jd)) }
            else { Rect::from_min_size(pos2(r.right() - 6.0 - jd, r.center().y - jd / 2.0), vec2(jd, jd)) };
        self.jog(ui, i, jr);
        // LOAD LOCK: a padlock in the screen's corner above the platter
        let lr = if platter_left { Rect::from_min_size(r.left_top() + vec2(4.0, 4.0), vec2(18.0, 18.0)) } else { Rect::from_min_size(pos2(r.right() - 22.0, r.top() + 4.0), vec2(18.0, 18.0)) };
        let resp = ui.interact(lr, ui.id().with(("lock", i)), Sense::click());
        self.reg(format!("lock{i}"), lr);
        if resp.clicked() { let d = &mut self.app.decks[i]; d.locked = !d.locked; let on = d.locked; self.app.say(&format!("Deck {} {}", i + 1, if on { "locked: nothing loads onto it" } else { "unlocked" }), 3.0); }
        let locked = self.app.decks[i].locked;
        paint_lock(ui.painter(), lr, locked, if locked { col } else if resp.hovered() { FG() } else { DIM() });
        resp.on_hover_text(if locked { "Load lock on: click to unlock" } else { "Load lock: stop anything loading onto this deck" });
        let text = if platter_left { Rect::from_min_max(pos2(jr.right() + 10.0, r.top() + 6.0), pos2(r.right() - 10.0, r.bottom() - 6.0)) }
            else { Rect::from_min_max(pos2(r.left() + 10.0, r.top() + 6.0), pos2(jr.left() - 10.0, r.bottom() - 6.0)) };
        let p = ui.painter().with_clip_rect(text);
        let d = &self.app.decks[i];
        let loaded = d.loaded();
        let t = d.now();
        let big = (text.height() * 0.3).min(text.width() * 0.4 / 5.0).clamp(14.0, 30.0);
        let small = (big * 0.45).clamp(9.5, 12.0);
        let lab = |x: f32, y: f32, s: &str, a: egui::Align2| { p.text(pos2(x, y), a, s, bold(small - 1.0), DIM()); };
        // three columns: TIME | KEY | BPM
        let cw = text.width() * 0.2;
        let (c0, c1, c2) = (text.left(), text.left() + text.width() * 0.4, text.right());
        let y_lab = text.top();
        let y_big = y_lab + small + 2.0;
        lab(c0, y_lab, "REMAIN", egui::Align2::LEFT_TOP);
        lab(c1 + cw / 2.0, y_lab, "KEY", egui::Align2::CENTER_TOP);
        lab(c2, y_lab, "BPM", egui::Align2::RIGHT_TOP);
        let fg = if loaded { FG() } else { DIM() };
        let remain = if loaded { format!("-{}", fmt_time(d.duration - t)) } else { "-:--.-".into() };
        let near_end = loaded && d.duration - t < 30.0;
        p.text(pos2(c0, y_big), egui::Align2::LEFT_TOP, remain, mono(big), if near_end { RED } else { fg });
        let key = if loaded { camelot(d.key_now()) } else { None };
        match key {
            Some(c) => { p.text(pos2(c1 + cw / 2.0, y_big), egui::Align2::CENTER_TOP, c.label(), bold(big), rgb(c.color())); }
            None => { p.text(pos2(c1 + cw / 2.0, y_big), egui::Align2::CENTER_TOP, "--", bold(big), DIM()); }
        }
        let bpm = if loaded && d.bpm() > 0.0 { format!("{:.1}", d.eff_bpm()) } else { "---.-".into() };
        p.text(pos2(c2, y_big), egui::Align2::RIGHT_TOP, bpm, mono(big), fg);
        // second line: elapsed | flags | tempo
        let y2 = y_big + big + 4.0;
        if loaded { p.text(pos2(c0, y2), egui::Align2::LEFT_TOP, fmt_time(t), mono(small), MUTED()); }
        let rg = d.range();
        let tempo = format!("{:+.2}% {}", (d.rate() - 1.0) * 100.0, if rg == 100.0 { "W".into() } else { format!("±{rg}") });
        p.text(pos2(c2, y2), egui::Align2::RIGHT_TOP, tempo, mono(small), if d.pitch.abs() < 1e-6 { MUTED() } else { FG() });
        let master = self.app.tempo_master() == Some(i);
        let mut flags: Vec<(String, Color32)> = [(master, "MASTER", CUE), (d.sync, "SYNC", col), (d.mt, "MT", col), (d.locked, "LOCK", col)].into_iter().filter(|f| f.0).map(|f| (f.1.to_string(), f.2)).collect();
        if let Some(t) = phase_tag(d) { flags.insert(flags.len().min(2), t); }
        let mut fx = c1 + cw / 2.0 - flags.iter().map(|f| f.0.len() as f32 * small * 0.62 + 6.0).sum::<f32>() / 2.0;
        for (s, c) in flags {
            let g = p.layout_no_wrap(s, bold(small - 1.0), c);
            let gw = g.size().x;
            p.galley(pos2(fx, y2), g, c);
            fx += gw + 6.0;
        }
        let y3 = text.bottom() - small;
        // the space between (big on two decks): title and artist, then the whole track's waveform
        let mid = Rect::from_min_max(pos2(c0, y2 + small + 8.0), pos2(c2, y3 - 6.0));
        if loaded && mid.height() >= 26.0 {
            let (title, artist) = d.track.as_ref().map(|t| (t.title.clone(), t.artist.clone())).unwrap_or_default();
            p.text(mid.left_top(), egui::Align2::LEFT_TOP, title, bold(small + 1.0), FG());
            let mut top = mid.top() + small + 4.0;
            if !artist.is_empty() && mid.height() >= 60.0 {
                p.text(pos2(mid.left(), top), egui::Align2::LEFT_TOP, artist, prop(small), MUTED());
                top += small + 4.0;
            }
            let ov = Rect::from_min_max(pos2(mid.left(), top + 2.0), pos2(mid.right(), mid.bottom().min(top + 2.0 + 56.0)));
            if ov.height() >= 12.0 { paint_overview(&p, ov, d, self.app.store.settings.wave_style == "rgb"); }
        }
        // bottom line: beat dots, bar.beat, phrase
        if loaded && d.ana.is_some() && y3 > y2 + small + 2.0 {
            let b = d.beat_at(t).floor();
            let beat = b.rem_euclid(4.0) as i32;
            let mut x = c0 + 4.0;
            for k in 0..4 {
                let on = k == beat;
                p.rect_filled(Rect::from_center_size(pos2(x + 6.0, y3 + small / 2.0), vec2(12.0, 6.0)), 1.5, if on { if k == 0 { RED } else { col } } else { LINE2() });
                x += 16.0;
            }
            let bar = (b / 4.0).floor() as i64 + 1;
            p.text(pos2(x + 4.0, y3 + small / 2.0), egui::Align2::LEFT_CENTER, format!("{}.{}", bar.max(0), beat + 1), mono(small), MUTED());
            if let Some((s, _e, bars, label, barlen)) = d.phrase_spans().into_iter().find(|ps| t >= ps.0 && t < ps.1) {
                let n = (((t - s) / barlen).floor() as u32 + 1).min(bars);
                p.text(pos2(c2, y3 + small / 2.0), egui::Align2::RIGHT_CENTER, format!("{} {}/{}", label.to_uppercase(), n, bars), bold(small), rgb(crate::analysis::phrase_color(&label)));
            }
        }
    }

    /// four decks: the same controls squeezed into half the height; jog on the outside, buttons and pads inside
    fn deck_compact(&mut self, ui: &mut Ui, i: usize, inner: Rect) {
        let h = inner.height();
        let col = deck_color(i);
        let rh = ((h - 4.0 * GAP) / 5.0).clamp(17.0, 26.0);
        let font = (rh * 0.46).clamp(9.5, 12.0);
        let cb = (rh + 6.0).clamp(24.0, 34.0);
        let jog_d = (h - cb - GAP).min(inner.width() * 0.27).max(50.0);
        let tempo_w = 22.0;
        let mirror = i % 2 == 1;
        let (tempo_r, jog_r, ctrl_r) = if !mirror {
            let t = Rect::from_min_size(inner.min, vec2(tempo_w, h));
            let j = Rect::from_min_size(pos2(t.right() + 6.0, inner.top()), vec2(jog_d, h));
            (t, j, Rect::from_min_max(pos2(j.right() + 8.0, inner.top()), inner.max))
        } else {
            let t = Rect::from_min_size(pos2(inner.right() - tempo_w, inner.top()), vec2(tempo_w, h));
            let j = Rect::from_min_size(pos2(t.left() - 6.0 - jog_d, inner.top()), vec2(jog_d, h));
            (t, j, Rect::from_min_max(inner.min, pos2(j.left() - 8.0, inner.bottom())))
        };
        self.tempo(ui, i, tempo_r, rh, font);
        self.jog(ui, i, Rect::from_min_size(pos2(jog_r.left(), jog_r.top()), vec2(jog_d, jog_d)));
        let y = jog_r.bottom() - cb;
        let gap = ((jog_d - 2.0 * cb) / 3.0).max(4.0);
        self.cue_play(ui, i, Rect::from_min_size(pos2(jog_r.left() + gap, y), vec2(cb, cb)), Rect::from_min_size(pos2(jog_r.left() + jog_d - gap - cb, y), vec2(cb, cb)), cb);
        let w = ctrl_r.width();
        let r0 = Rect::from_min_size(ctrl_r.min, vec2(w, rh));
        let r1 = Rect::from_min_size(pos2(ctrl_r.left(), r0.bottom() + GAP), vec2(w, rh));
        {
            let ui = &mut row_ui(ui, r0);
            let d = &self.app.decks[i];
            let (sync, mt, q, grid_open) = (d.sync, d.mt, d.quant, self.app.ui.grid_open[i]);
            let u = (w - 4.0 * GAP) / 5.4;
            let r = button(ui, "SYNC", vec2(u * 1.4, rh), sync, col, font).on_hover_text("Sync to the master. Right-click (or hold SYNC on the controller): make this deck the master");
            if !self.learn(ui, &r, "sync", i as i8) { if r.clicked() { self.app.set_sync(i, !sync); } else if r.secondary_clicked() { self.app.make_master(i); } }
            let r = button(ui, "MT", vec2(u, rh), mt, col, font).on_hover_text("Master tempo (keylock)");
            if !self.learn(ui, &r, "keylock", i as i8) && r.clicked() { self.app.set_mt(i, !mt); }
            let r = button(ui, "Q", vec2(u, rh), q, col, font).on_hover_text("Quantize");
            if !self.learn(ui, &r, "quant", i as i8) && r.clicked() { self.app.decks[i].quant = !q; }
            let r = button(ui, "GRID", vec2(u, rh), grid_open, col, font).on_hover_text("Edit the beat grid");
            self.reg(format!("deck{i}.grid"), r.rect);
            if r.clicked() { self.app.ui.grid_open[i] = !grid_open; }
            self.loop_len(ui, i, vec2(u, rh), font);
        }
        self.loop_row(ui, i, r1, rh, font);
        let rest = Rect::from_min_max(pos2(ctrl_r.left(), r1.bottom() + GAP), ctrl_r.max);
        self.pads(ui, i, rest, rh, font);
    }

    /// tempo fader (top = slower, like a CDJ) and its range under it
    fn tempo(&mut self, ui: &mut Ui, i: usize, r: Rect, rh: f32, font: f32) {
        let frozen = self.app.learning();
        let ui = &mut ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::top_down(Align::Center)));
        ui.spacing_mut().item_spacing = vec2(0.0, GAP);
        let mut v = (0.5 - self.app.decks[i].pitch / 2.0) as f32;
        let rs = vslider(ui, &mut v, vec2(r.width(), (r.height() - rh - GAP).max(40.0)), true, frozen).on_hover_text("Tempo (double-click: 0%)");
        if !self.learn(ui, &rs, "tempo", i as i8) && rs.changed() { self.app.decks[i].set_pitch((0.5 - v as f64) * 2.0); }
        let rl = RANGES[self.app.decks[i].range_idx];
        let label = if rl == 100.0 { "W".to_string() } else { format!("±{rl}") };
        if button(ui, &label, vec2(r.width(), rh), false, deck_color(i), font - 2.0).on_hover_text("Tempo range").clicked() { self.app.decks[i].cycle_range(); }
    }
    fn jog(&mut self, ui: &mut Ui, i: usize, jr: Rect) {
        let resp = ui.interact(jr, ui.id().with(("jog", i)), Sense::click_and_drag());
        paint_jog(ui.painter(), jr, &self.app.decks[i], i);
        if self.learn(ui, &resp, "jog", i as i8) { return; }
        let ang = |p: egui::Pos2| (p.y - jr.center().y).atan2(p.x - jr.center().x);
        if resp.drag_started() || (resp.is_pointer_button_down_on() && self.jog_last[i].is_none()) {
            if let Some(p) = resp.interact_pointer_pos() { self.jog_last[i] = Some(ang(p)); self.app.decks[i].jog_touch(true, false); }
        }
        if let (Some(last), Some(p)) = (self.jog_last[i], resp.interact_pointer_pos()) {
            let a = ang(p);
            let mut da = a - last;
            if da > std::f32::consts::PI { da -= std::f32::consts::TAU; }
            if da < -std::f32::consts::PI { da += std::f32::consts::TAU; }
            if da != 0.0 { self.app.decks[i].jog_ticks(da as f64 / std::f64::consts::TAU * 720.0, JogKind::Scratch); }
            self.jog_last[i] = Some(a);
        }
        if self.jog_last[i].is_some() && !resp.is_pointer_button_down_on() { self.jog_last[i] = None; self.app.decks[i].jog_touch(false, false); }
    }
    fn cue_play(&mut self, ui: &mut Ui, i: usize, cue_r: Rect, play_r: Rect, cb: f32) {
        let d = &self.app.decks[i];
        let cue_lit = d.loaded() && (d.previewing || (d.now() - d.cue).abs() < 0.03);
        let playing = d.playing;
        let wide = cue_r.width() > cue_r.height() * 1.5;
        let r = { let u = &mut ui.new_child(UiBuilder::new().max_rect(cue_r)); if wide { button(u, "CUE", cue_r.size(), cue_lit, CUE, 15.0) } else { round_button(u, "CUE", cb, cue_lit, CUE) } };
        if !self.learn(ui, &r, "cue", i as i8) {
            if r.is_pointer_button_down_on() && ui.input(|x| x.pointer.any_pressed()) { self.app.decks[i].cue_down(); }
            if r.drag_stopped() || r.clicked() { self.app.decks[i].cue_up(); }
        }
        let r = { let u = &mut ui.new_child(UiBuilder::new().max_rect(play_r)); let l = if playing { "❚❚" } else { "▶" }; if wide { button(u, l, play_r.size(), playing, GREEN, 17.0) } else { round_button(u, l, cb, playing, GREEN) } };
        if !self.learn(ui, &r, "play", i as i8) && r.clicked() { self.app.toggle_play(i); }
    }
    /// the loop's length in beats (outlined while it's on)
    fn loop_len(&mut self, ui: &mut Ui, i: usize, size: egui::Vec2, font: f32) {
        let d = &self.app.decks[i];
        let info = match (d.lp.a, d.lp.b) { (None, _) => "—".to_string(), (Some(_), None) => "IN…".into(), _ => frac_text(d.loop_beats()) };
        let on = d.lp.on;
        let lr = button(ui, &info, size, false, LOOP, font).on_hover_text("Loop length in beats");
        ui.painter().rect_stroke(lr.rect, 4.0, Stroke::new(1.0_f32, if on { LOOP } else { LINE2() }), StrokeKind::Inside);
    }
    fn loop_row(&mut self, ui: &mut Ui, i: usize, r: Rect, rh: f32, font: f32) {
        let col = deck_color(i);
        let w = r.width();
        let ui = &mut row_ui(ui, r);
        let d = &self.app.decks[i];
        let (lp_on, a_set, b_set) = (d.lp.on, d.lp.a.is_some(), d.lp.b.is_some());
        let u = (w - 6.0 * GAP) / 7.2;
        let rr = button(ui, "IN", vec2(u, rh), lp_on || (a_set && !b_set), LOOP, font);
        if !self.learn(ui, &rr, "loopIn", i as i8) && rr.clicked() { self.app.decks[i].loop_in(); }
        let rr = button(ui, "OUT", vec2(u, rh), lp_on, LOOP, font);
        if !self.learn(ui, &rr, "loopOut", i as i8) && rr.clicked() { self.app.decks[i].loop_out(); }
        let rr = button(ui, "RELOOP", vec2(u * 1.6, rh), b_set, LOOP, font - 1.0);
        if !self.learn(ui, &rr, "reloop", i as i8) && rr.clicked() { self.app.decks[i].reloop_exit(); }
        if button(ui, "½", vec2(u * 0.8, rh), false, LOOP, font).on_hover_text("Halve loop").clicked() { self.app.decks[i].loop_scale(0.5); }
        if button(ui, "×2", vec2(u * 0.8, rh), false, LOOP, font).on_hover_text("Double loop").clicked() { self.app.decks[i].loop_scale(2.0); }
        if button(ui, "◀4", vec2(u, rh), false, col, font).on_hover_text("Jump back 4 beats").clicked() { self.app.decks[i].beat_jump(-4.0); }
        if button(ui, "4▶", vec2(u, rh), false, col, font).on_hover_text("Jump ahead 4 beats").clicked() { self.app.decks[i].beat_jump(4.0); }
    }
    /// the pad-mode tabs and 8 pads (or the grid editor while it's open)
    fn pads(&mut self, ui: &mut Ui, i: usize, rest: Rect, rh: f32, font: f32) {
        if self.app.ui.grid_open[i] { self.grid_bar(ui, i, rest, rh, font); return; }
        let col = deck_color(i);
        let w = rest.width();
        let r2 = Rect::from_min_size(rest.min, vec2(w, rh));
        {
            let ui = &mut row_ui(ui, r2);
            let u = (w - 4.0 * GAP) / 5.0;
            for (m, label, act) in [(PadMode::HotCue, "CUES", "mode:hotcue"), (PadMode::BeatLoop, "LOOP", "mode:beatloop"), (PadMode::BeatJump, "JUMP", "mode:beatjump"),
                (PadMode::PadFx, "FX1", "mode:padfx"), (PadMode::PadFx2, "FX2", "mode:padfx2")] {
                let r = tab(ui, label, vec2(u, rh), self.app.decks[i].pad_mode == m, col, font);
                if !self.learn(ui, &r, act, i as i8) && r.clicked() { self.app.decks[i].pad_mode = m; }
            }
        }
        let pads_top = r2.bottom() + GAP;
        let pad_h = ((rest.bottom() - pads_top - GAP) / 2.0).clamp(14.0, 44.0);
        let pw = (w - 3.0 * GAP) / 4.0;
        for row in 0..2 {
            for c in 0..4 {
                let pidx = row * 4 + c;
                let pr = Rect::from_min_size(pos2(rest.left() + c as f32 * (pw + GAP), pads_top + row as f32 * (pad_h + GAP)), vec2(pw, pad_h));
                let d = &self.app.decks[i];
                let fx_mode = d.pad_mode.fx_bank().is_some();
                let (label, lit, accent) = match d.pad_mode {
                    PadMode::HotCue => match d.hot[pidx] { Some(t) => (format!("{}  {}", &"ABCDEFGH"[pidx..pidx + 1], fmt_time(t).trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('.')), true, GREEN), None => ("ABCDEFGH"[pidx..pidx + 1].to_string(), false, GREEN) },
                    PadMode::BeatLoop => { let s = LOOP_SIZES[pidx]; (frac_text(s), d.lp.on && (d.loop_beats() - s).abs() < 0.01, LOOP) }
                    PadMode::BeatJump => { let j = JUMPS[pidx]; (format!("{}{}{}", if j < 0.0 { "◀ " } else { "" }, j.abs(), if j > 0.0 { " ▶" } else { "" }), false, col) }
                    PadMode::PadFx | PadMode::PadFx2 => {
                        let bank = if d.pad_mode == PadMode::PadFx { 0 } else { 1 };
                        let (fx, b, _) = PAD_FX[bank][pidx];
                        let label = if spec(fx).beats.is_some() { format!("{} {}", short_fx(fx), frac_text(b)) } else { short_fx(fx).to_string() };
                        (label, self.app.fx.pad[i] == Some((pidx, bank)), Color32::from_rgb(0xb4, 0x7c, 0xff))
                    }
                };
                let r = { let u = &mut ui.new_child(UiBuilder::new().max_rect(pr)); pad(u, &label, pr.size(), lit, accent, (pad_h * 0.42).clamp(9.5, 12.5)) };
                self.reg(format!("deck{i}.pad{pidx}"), r.rect);
                let act = format!("hot:{pidx}");
                if self.learn(ui, &r, &act, i as i8) { continue; }
                if fx_mode {
                    // PAD FX: on while held
                    let down = r.is_pointer_button_down_on();
                    if down && self.pad_held != Some((i, pidx)) { self.pad_held = Some((i, pidx)); self.app.pad_down(i, pidx, false); }
                    if !down && self.pad_held == Some((i, pidx)) { self.pad_held = None; self.app.pad_up(i, pidx); }
                } else {
                    if r.clicked() { let shift = ui.input(|x| x.modifiers.shift); self.app.pad_down(i, pidx, shift); }
                    if r.secondary_clicked() { self.app.pad_down(i, pidx, true); }
                }
            }
        }
    }

    fn grid_bar(&mut self, ui: &mut Ui, i: usize, rect: Rect, rh: f32, font: f32) {
        ui.painter().rect_filled(rect, 4.0, Color32::from_rgb(0x0c, 0x0e, 0x12));
        let ui = &mut ui.new_child(UiBuilder::new().max_rect(rect.shrink(3.0)).layout(Layout::left_to_right(Align::Min).with_main_wrap(true)));
        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
        let bh = rh.min(22.0);
        let id = ui.id().with(("bpm", i));
        let te = egui::TextEdit::singleline(&mut self.app.ui.bpm_text[i]).id(id).desired_width(58.0).font(mono(12.0)).hint_text(format!("{:.3}", self.app.decks[i].bpm()));
        let r = ui.add_sized(vec2(64.0, bh), te);
        if r.gained_focus() { self.app.ui.bpm_text[i] = format!("{}", (self.app.decks[i].bpm() * 1000.0).round() / 1000.0); }
        if r.lost_focus() {
            if ui.input(|x| x.key_pressed(Key::Enter)) {
                if let Ok(v) = self.app.ui.bpm_text[i].replace(',', ".").trim().parse::<f64>() { if v > 20.0 && v < 400.0 { self.app.decks[i].set_bpm(v, "BPM typed in"); } }
            }
            self.app.ui.bpm_text[i].clear();
        }
        let shift = ui.input(|x| x.modifiers.shift);
        let b = self.app.decks[i].bpm();
        let c = deck_color(i);
        let gb = |ui: &mut Ui, t: &str, tip: &str| { let w = ui.painter().layout_no_wrap(t.to_string(), prop(font), FG()).size().x + 12.0; button(ui, t, vec2(w, bh), false, c, font).on_hover_text(tip).clicked() };
        let d = &mut self.app.decks[i];
        if gb(ui, "−.01", "BPM −0.01 (Shift: −0.1)") { d.set_bpm(b - if shift { 0.1 } else { 0.01 }, ""); }
        if gb(ui, "+.01", "BPM +0.01 (Shift: +0.1)") { d.set_bpm(b + if shift { 0.1 } else { 0.01 }, ""); }
        if gb(ui, "÷2", "Halve the BPM") { d.set_bpm(b / 2.0, "halved"); }
        if gb(ui, "×2", "Double the BPM") { d.set_bpm(b * 2.0, "doubled"); }
        if gb(ui, "TAP", "Tap along with the beat (4+ taps)") { d.tap(); }
        if gb(ui, "◀ 5ms", "Slide the grid 5 ms earlier (Shift: 1 ms)") { d.nudge_grid(if shift { -0.001 } else { -0.005 }); }
        if gb(ui, "5ms ▶", "Slide the grid 5 ms later (Shift: 1 ms)") { d.nudge_grid(if shift { 0.001 } else { 0.005 }); }
        if gb(ui, "BEAT HERE", "Put the nearest beat line on the playhead (pause on a kick first)") { d.beat_here(); }
        if gb(ui, "BAR 1 HERE", "Make the playhead the first beat of a bar") { d.downbeat_here(); }
        if gb(ui, "RESET", "Back to the analysed grid") { d.reset_grid(); }
        if gb(ui, "DONE", "Close the grid editor") { self.app.ui.grid_open[i] = false; }
    }

    // ===================================================================== mixer + BEAT FX
    fn mixer(&mut self, ui: &mut Ui, rect: Rect) {
        ui.painter().rect_filled(rect, 5.0, PANEL());
        let frozen = self.app.learning();
        let inner = rect.shrink2(vec2(6.0, 5.0));
        let chans = mixer_order(self.app.deck_count());
        let n = chans.len();
        let rh = 20.0;
        let fx_h = 2.0 * rh + GAP;
        let xf_h = 22.0;
        let strips = Rect::from_min_max(inner.min, pos2(inner.right(), inner.bottom() - fx_h - xf_h - 2.0 * GAP));
        let sw = (strips.width() - (n - 1) as f32 * GAP) / n as f32;
        // one column per channel, top to bottom like the FLX4: TRIM, HI, MID, LOW, CFX, CUE, then the fader + meter
        let fader_min = 60.0;
        let kh = ((strips.height() - 16.0 - rh - fader_min - 3.0 * GAP) / 5.0).clamp(18.0, 34.0);
        let ks = (kh - 4.0).min(sw - 34.0).clamp(12.0, 26.0);
        for (k, &i) in chans.iter().enumerate() {
            let col = deck_color(i);
            let sr = Rect::from_min_size(pos2(strips.left() + k as f32 * (sw + GAP), strips.top()), vec2(sw, strips.height()));
            ui.painter().rect_filled(sr, 3.0, PANEL_D());
            let tab_r = Rect::from_min_size(sr.min, vec2(sw, 14.0));
            // 4 decks + controller: only the decks the FLX4's two sides play are lit, with a medium strobe
            // (1.5 Hz) so you can see where the hands are; the others stay dim. 2 decks: lit when loaded.
            let tab_c = if self.app.four() && !self.app.midi_io.name.is_empty() {
                if self.side_of(i).is_some() {
                    let a = strobe(ui.ctx());
                    Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), (a * 255.0) as u8)
                } else { Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 40) }
            } else if self.app.decks[i].loaded() { col } else { Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 60) };
            ui.painter().rect_filled(tab_r, 3.0, tab_c);
            ui.painter().text(tab_r.center(), egui::Align2::CENTER_CENTER, format!("{}", i + 1), bold(11.0), BG());
            let names: [(&str, &str); 5] = [("TRIM", "trim"), ("HI", "hi"), ("MID", "mid"), ("LOW", "low"), ("CFX", "cfx")];
            // knob and its label side by side, centred in the strip
            let kw = ks + 8.0 + 26.0;
            let kx = sr.center().x - kw / 2.0;
            for (q, (label, key)) in names.iter().enumerate() {
                let y = tab_r.bottom() + 2.0 + q as f32 * kh;
                if *key == "trim" {
                    // TRIM with its gain in dB under it
                    let g = crate::mixer::trim_gain(self.app.mixer.trim[i] as f64);
                    ui.painter().text(pos2(kx + 24.0, y + kh / 2.0 - 4.5), egui::Align2::RIGHT_CENTER, *label, prop(8.5), MUTED());
                    ui.painter().text(pos2(kx + 24.0, y + kh / 2.0 + 5.0), egui::Align2::RIGHT_CENTER, db_text(g), mono(8.0), FG());
                } else {
                    ui.painter().text(pos2(kx + 24.0, y + kh / 2.0), egui::Align2::RIGHT_CENTER, *label, prop(8.5), MUTED());
                }
                let kr = Rect::from_min_size(pos2(kx + 26.0, y + (kh - ks - 4.0) / 2.0), vec2(ks + 8.0, ks + 4.0));
                let ui = &mut ui.new_child(UiBuilder::new().max_rect(kr).layout(Layout::top_down(Align::Center)));
                let m = &self.app.mixer;
                let mut v = match *key { "trim" => m.trim[i], "hi" => m.hi[i], "mid" => m.mid[i], "low" => m.low[i], _ => m.cfx[i] };
                let r = knob(ui, &mut v, "", if *key == "cfx" { LOOP } else { col }, 0.5, ks, frozen).on_hover_text(*label);
                if !self.learn(ui, &r, key, i as i8) && r.changed() {
                    let m = &mut self.app.mixer;
                    match *key { "trim" => m.trim[i] = v, "hi" => m.hi[i] = v, "mid" => m.mid[i] = v, "low" => m.low[i] = v, _ => m.cfx[i] = v }
                    self.app.send_mixer();
                }
            }
            let cue_top = tab_r.bottom() + 2.0 + 5.0 * kh + GAP;
            let cw = (sw - 8.0).min(48.0);
            let cue_r = Rect::from_min_size(pos2(sr.center().x - cw / 2.0, cue_top), vec2(cw, rh));
            let lit = self.app.mixer.pfl[i];
            let r = { let u = &mut ui.new_child(UiBuilder::new().max_rect(cue_r)); button(u, "CUE", cue_r.size(), lit, CUE, 10.5) };
            if !self.learn(ui, &r, "pfl", i as i8) && r.clicked() { self.app.mixer.pfl[i] = !lit; self.app.send_mixer(); }
            let fh = sr.bottom() - cue_r.bottom() - 2.0 * GAP;
            let fr = Rect::from_min_size(pos2(sr.center().x - 15.0, cue_r.bottom() + GAP), vec2(22.0, fh));
            let mr = Rect::from_min_size(pos2(fr.right() + 4.0, fr.top() + 4.0), vec2(5.0, fh - 8.0));
            paint_meter(ui.painter(), mr, level(self.app.status_snap.meters[i]));
            let mut v = self.app.mixer.fader[i];
            let r = { let u = &mut ui.new_child(UiBuilder::new().max_rect(fr)); vslider(u, &mut v, fr.size(), false, frozen) };
            if !self.learn(ui, &r, "fader", i as i8) && r.changed() { self.app.mixer.fader[i] = v; self.app.send_mixer(); }
        }
        // ---- crossfader, CUE/MASTER, MST
        let xr = Rect::from_min_size(pos2(inner.left(), strips.bottom() + GAP), vec2(inner.width(), xf_h));
        {
            let ui = &mut row_ui(ui, xr);
            let mut v = self.app.mixer.head_mix;
            let r = knob(ui, &mut v, "", FG(), 0.0, 14.0, frozen).on_hover_text("Headphones: CUE ⇄ MASTER");
            if !self.learn(ui, &r, "headMix", -1) && r.changed() { self.app.mixer.head_mix = v; self.app.send_mixer(); }
            let mc = self.app.mixer.master_cue;
            let r = button(ui, "MST", vec2(32.0, 18.0), mc, CUE, 9.5).on_hover_text("Master in the headphones");
            if !self.learn(ui, &r, "masterCue", -1) && r.clicked() { self.app.mixer.master_cue = !mc; self.app.send_mixer(); }
            let mut x = self.app.mixer.xfader;
            // as wide on the right of the knob + MST as they take on the left, so its middle is the mixer's middle
            let used = ui.min_rect().right() - xr.left() + ui.spacing().item_spacing.x;
            let w = (xr.width() - 2.0 * used).max(60.0);
            let r = hslider(ui, &mut x, vec2(w, 18.0), frozen).on_hover_text("Crossfader: 1 and 3 left, 2 and 4 right (double-click: centre)");
            if !self.learn(ui, &r, "xfader", -1) && r.changed() { self.app.mixer.xfader = x; self.app.send_mixer(); }
        }
        // ---- BEAT FX: effect ◀ ▶, beat ◀ ▶, channel, level, ON
        let fx_r = Rect::from_min_max(pos2(inner.left(), xr.bottom() + GAP), inner.max);
        ui.painter().rect_filled(fx_r.expand(2.0), 3.0, PANEL_D());
        let r1 = Rect::from_min_size(fx_r.min, vec2(fx_r.width(), rh));
        let r2 = Rect::from_min_size(pos2(fx_r.left(), r1.bottom() + GAP), vec2(fx_r.width(), rh));
        let fxc = Color32::from_rgb(0xb4, 0x7c, 0xff);
        {
            let ui = &mut row_ui(ui, r1);
            let w = fx_r.width();
            let r = button(ui, "◀", vec2(18.0, rh), false, fxc, 10.0).on_hover_text("Previous effect");
            if !self.learn(ui, &r, "fxSelectPrev", -1) && r.clicked() { self.app.fx_select(-1); }
            let name = self.app.fx.fx.name();
            let nw = (w - 4.0 * 18.0 - 46.0 - 5.0 * GAP).max(50.0);
            let kinds = self.app.fx.kinds();
            let resp = ui.add_sized(vec2(nw, rh), egui::Button::new(egui::RichText::new(name).font(bold(11.0)).color(fxc)).fill(BG()));
            let resp = self.reg_resp("fxSelect", resp);
            egui::Popup::menu(&resp).show(|ui| {
                for k in kinds { if ui.selectable_label(self.app.fx.fx == k, k.name()).clicked() { self.app.fx_pick(k); } }
            });
            let r = button(ui, "▶", vec2(18.0, rh), false, fxc, 10.0).on_hover_text("Next effect");
            if !self.learn(ui, &r, "fxSelect", -1) && r.clicked() { self.app.fx_select(1); }
            let has_beats = spec(self.app.fx.fx).beats.is_some();
            let r = button(ui, "◀", vec2(18.0, rh), false, fxc, 10.0).on_hover_text("Fewer beats");
            if !self.learn(ui, &r, "fxBeatDown", -1) && r.clicked() { self.app.fx_beat(-1); }
            let bt = if has_beats { frac_text(self.app.fx.beat()) } else { "—".into() };
            ui.add_sized(vec2(46.0, rh), egui::Label::new(egui::RichText::new(bt).font(mono(11.5)).color(FG())));
            let r = button(ui, "▶", vec2(18.0, rh), false, fxc, 10.0).on_hover_text("More beats");
            if !self.learn(ui, &r, "fxBeatUp", -1) && r.clicked() { self.app.fx_beat(1); }
        }
        {
            let ui = &mut row_ui(ui, r2);
            let w = fx_r.width();
            let units: Vec<usize> = (0..self.app.deck_count()).chain([DECKS]).collect();
            for u in units {
                let label = if u == DECKS { "M".to_string() } else { format!("{}", u + 1) };
                let c = if u == DECKS { FG() } else { deck_color(u) };
                if button(ui, &label, vec2(18.0, rh), self.app.fx.ch == u, c, 10.5).on_hover_text(if u == DECKS { "BEAT FX on the master" } else { "BEAT FX on this channel" }).clicked() { self.app.fx_channel(u); }
            }
            let used = (self.app.deck_count() + 1) as f32 * (18.0 + GAP);
            let lw = (w - used - 54.0 - GAP).max(30.0);
            let mut dv = self.app.fx.depth as f32;
            let r = hslider(ui, &mut dv, vec2(lw, rh - 2.0), frozen).on_hover_text(format!("LEVEL/DEPTH {:.0}%", dv * 100.0));
            if !self.learn(ui, &r, "fxDepth", -1) && r.changed() { self.app.fx_depth(dv as f64); }
            let on = self.app.fx.on;
            let r = button(ui, "FX ON", vec2(52.0, rh), on, fxc, 10.5);
            self.reg("fxOn", r.rect);
            if !self.learn(ui, &r, "fxOn", -1) && r.clicked() { self.app.fx_on(!on); }
        }
    }
    fn reg_resp(&mut self, name: &str, r: Response) -> Response { self.rects.insert(name.into(), r.rect); r }
    // ===================================================================== library
    fn library(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, PANEL());
        ui.painter().hline(rect.x_range(), rect.top(), Stroke::new(1.0_f32, LINE()));
        let tool_h = 30.0;
        let tool = Rect::from_min_size(rect.min, vec2(rect.width(), tool_h));
        let tw = self.app.store.settings.tree_w;
        let tree_w = if tw > 0.0 { tw.clamp(120.0, rect.width() * 0.5) } else { (rect.width() * 0.17).clamp(170.0, 240.0) };
        let tree = Rect::from_min_max(pos2(rect.left(), tool.bottom()), pos2(rect.left() + tree_w, rect.bottom()));
        let list = Rect::from_min_max(pos2(tree.right(), tool.bottom()), rect.max);
        {
            let ui = &mut ui.new_child(UiBuilder::new().max_rect(tool.shrink2(vec2(10.0, 4.0))).layout(Layout::left_to_right(Align::Center)));
            self.toolbar(ui);
        }
        ui.painter().hline(rect.x_range(), tool.bottom(), Stroke::new(1.0_f32, LINE()));
        {
            let ui = &mut ui.new_child(UiBuilder::new().max_rect(tree).layout(Layout::top_down(Align::Min)));
            self.tree(ui);
        }
        ui.painter().vline(tree.right(), tree.y_range(), Stroke::new(1.0_f32, LINE()));
        {
            let ui = &mut ui.new_child(UiBuilder::new().max_rect(list.shrink2(vec2(1.0, 0.0))).layout(Layout::top_down(Align::Min)));
            self.track_list(ui);
        }
        // drag the line between the playlists and the tracks (double-click: back to automatic)
        let handle = Rect::from_min_max(pos2(tree.right() - 3.0, tree.top()), pos2(tree.right() + 3.0, tree.bottom()));
        let r = ui.interact(handle, ui.id().with("tree_split"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if r.hovered() || r.dragged() { ui.painter().vline(tree.right(), tree.y_range(), Stroke::new(2.0_f32, deck_color(0))); }
        if r.dragged() { if let Some(p) = r.interact_pointer_pos() { self.app.store.settings.tree_w = (p.x - rect.left()).clamp(120.0, rect.width() * 0.5); } }
        if r.drag_stopped() { self.app.store.dirty_settings = true; }
        if r.double_clicked() { self.app.store.settings.tree_w = 0.0; self.app.store.dirty_settings = true; }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        if ui.button("Add music folder").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::AddFolder, None)); }
        ui.label(egui::RichText::new(self.app.lists.view_name(&self.app.view)).font(bold(14.0)));
        let sel = self.app.selected_tracks();
        ui.menu_button("Add to ▾", |ui| {
            if sel.is_empty() { ui.label("Select a track first"); return; }
            let mut own: Vec<(String, String, usize)> = self.app.lists.lists.iter().filter(|p| !p.folder).map(|p| (p.id.clone(), self.app.lists.path_name(p), p.items.len())).collect();
            own.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
            for (id, name, n) in own { if ui.button(format!("{name} ({n})")).clicked() { self.app.add_to_playlist(&id, &sel); ui.close(); } }
            ui.separator();
            if ui.button("+ New playlist…").clicked() {
                let name = self.app.lists.next_name("Playlist");
                let id = self.app.lists.create(&name, false, None);
                self.app.add_to_playlist(&id, &sel);
                self.app.ui.renaming = Some((id, name, false));
                ui.close();
            }
        }).response.on_hover_text("Add the selected tracks to a playlist");
        if matches!(self.app.view, View::Pl(_)) && ui.button("Remove from playlist").on_hover_text("Remove the selected tracks from this playlist (Delete key)").clicked() { self.app.remove_from_playlist(); }
        let r = ui.add(egui::TextEdit::singleline(&mut self.app.list.query).hint_text("Search title, artist or key (8A)").desired_width(200.0));
        self.reg("search", r.rect);
        if std::mem::take(&mut self.focus_search) { r.request_focus(); }
        if r.changed() { self.app.apply_view(false); }
        // quick sort: BPM, KEY, MATCH (what mixes into the playing deck first); again flips the order
        for (label, k, tip) in [("BPM", SortKey::Bpm, "Sort by tempo"), ("KEY", SortKey::Key, "Sort by Camelot key"), ("MATCH", SortKey::Match, "Tracks that mix into the playing deck first: same key, then neighbours, nearest tempo")] {
            let on = self.app.list.sort == Some(k);
            let txt = if on && k != SortKey::Match { format!("{label} {}", if self.app.list.dir > 0 { "▴" } else { "▾" }) } else { label.to_string() };
            let r = ui.selectable_label(on, txt).on_hover_text(tip);
            self.reg(format!("sort:{label}"), r.rect);
            if r.clicked() {
                self.app.list.dir = if on && k != SortKey::Match { -self.app.list.dir } else { 1 };
                self.app.list.sort = if on && k == SortKey::Match { None } else { Some(k) };
                self.app.apply_view(true);
            }
        }
        let browse = self.app.store.settings.browse;
        if ui.selectable_label(browse, "BROWSE").on_hover_text("Big library, waveforms only (SHIFT + press BROWSE on the FLX4)").clicked() {
            self.app.store.settings.browse = !browse; self.app.store.dirty_settings = true;
        }
        // COPY USB: the open stick (or the first one) → this computer; UPDATE USB once a copy exists
        let stick = self.app.lists.devices.iter().filter(|d| !crate::usbcopy::is_copy(&d.root))
            .find(|d| matches!(&self.app.view, View::Dev(x) | View::DevPl(x, _) if x == &d.id))
            .or_else(|| self.app.lists.devices.iter().find(|d| !crate::usbcopy::is_copy(&d.root)))
            .map(|d| (d.id.clone(), crate::usbcopy::copy_root(&d.root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()).exists()));
        let label = match (self.app.copying, &stick) {
            (Some((done, total)), _) => format!("COPYING {}%", if total > 0 { done * 100 / total } else { 0 }),
            (None, Some((_, true))) => "UPDATE USB".to_string(),
            _ => "COPY USB".to_string(),
        };
        let r = ui.add_enabled(stick.is_some() && self.app.copying.is_none(), egui::Button::new(label))
            .on_hover_text("Copy the rekordbox stick to this computer, or update that copy (nothing is deleted)")
            .on_disabled_hover_text("Plug in a rekordbox stick");
        self.reg("copyusb", r.rect);
        if r.clicked() { if let Some((id, _)) = stick.clone() { self.app.copy_device(&id); } }
        let r = ui.add_enabled(stick.is_some() && self.app.copying.is_none(), egui::Button::new("EJECT"))
            .on_hover_text("Unmount the stick so it's safe to pull out; Cratebox carries on from the local copy");
        self.reg("eject", r.rect);
        if r.clicked() { if let Some((id, _)) = stick { self.app.eject_device(&id); } }
        ui.label(egui::RichText::new(&self.app.progress).size(12.0).color(MUTED()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(egui::Label::new(egui::RichText::new("Browse knob scrolls · press it for playlists · drag rows onto a deck or a playlist").size(10.0).color(DIM())).truncate());
        });
    }

    fn tree(&mut self, ui: &mut Ui) {
        let rows = self.app.tree_rows();
        let focus = self.app.focus_tree;
        let scroll = std::mem::take(&mut self.app.ui.pending_scroll_tree);
        egui::ScrollArea::vertical().id_salt("tree").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(4.0);
            for (k, r) in rows.iter().enumerate() {
                if let RowKind::Head(what) = r.kind {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        let resp = ui.label(egui::RichText::new(&r.label).size(10.0).color(MUTED()));
                        if what == "pl" {
                            // drop a playlist here to take it back to the top level
                            if let Some(p) = resp.dnd_release_payload::<DragList>() { let id = p.0.clone(); self.app.lists.move_to(&id, None); }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add_space(6.0);
                                ui.menu_button("⋯", |ui| {
                                    if ui.button("Import rekordbox XML…").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::ImportXml, None)); ui.close(); }
                                    if ui.button("Export to rekordbox XML…").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::ExportXml, None)); ui.close(); }
                                }).response.on_hover_text("rekordbox XML: import / export");
                                if ui.small_button("▤").on_hover_text("New folder").clicked() {
                                    let n = self.app.lists.next_name("Folder");
                                    let id = self.app.lists.create(&n, true, None);
                                    self.app.ui.renaming = Some((id, n, true));
                                }
                                let nb = ui.small_button("+").on_hover_text("New playlist");
                                self.rects.insert("tree.newpl".into(), nb.rect);
                                if nb.clicked() {
                                    let n = self.app.lists.next_name("Playlist");
                                    let id = self.app.lists.create(&n, false, None);
                                    self.app.ui.renaming = Some((id, n, true));
                                }
                            });
                        } else {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add_space(6.0);
                                if ui.small_button("+ USB").on_hover_text("Open a rekordbox USB stick or SD card (read-only)").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::OpenUsb, None)); }
                            });
                        }
                    });
                    continue;
                }
                // renaming this one?
                if let Some((rid, mut text, is_new)) = self.app.ui.renaming.clone() {
                    if (r.kind == RowKind::Pl || r.kind == RowKind::Folder) && r.id == rid {
                        let te = ui.horizontal(|ui| { ui.add_space(6.0 + r.depth as f32 * 12.0 + 26.0); ui.add(egui::TextEdit::singleline(&mut text).desired_width(150.0)) }).inner;
                        te.request_focus();
                        let enter = ui.input(|x| x.key_pressed(Key::Enter));
                        let esc = ui.input(|x| x.key_pressed(Key::Escape));
                        if enter || te.lost_focus() || esc {
                            if !esc { self.app.lists.rename(&rid, &text); }
                            self.app.ui.renaming = None;
                            if is_new && r.kind == RowKind::Pl { self.app.set_view(View::Pl(rid.clone())); }
                        } else { self.app.ui.renaming = Some((rid, text, is_new)); }
                        continue;
                    }
                }
                if self.app.ui.confirm_delete.as_deref() == Some(r.id.as_str()) && !r.id.is_empty() {
                    let mut done = false;
                    egui::Frame::new().fill(Color32::from_rgb(0x2a, 0x1a, 0x1c)).inner_margin(6.0).show(ui, |ui| {
                        ui.label(format!("Delete \"{}\"{}?", r.label, if r.kind == RowKind::Folder { " and what's in it" } else { "" }));
                        ui.horizontal(|ui| {
                            if ui.button(egui::RichText::new("Delete").color(Color32::WHITE)).clicked() { let id = r.id.clone(); self.app.delete_playlist(&id); done = true; }
                            if ui.button("Cancel").clicked() { done = true; }
                        });
                    });
                    if done { self.app.ui.confirm_delete = None; }
                    continue;
                }
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click_and_drag());
                self.rects.insert(format!("tree:{}", r.label), rect);
                let is_view = row_is_view(r, &self.app.view);
                let p = ui.painter();
                if is_view { p.rect_filled(rect, 0.0, SEL2()); }
                else if resp.hovered() { p.rect_filled(rect, 0.0, HOVER()); }
                if focus && k == self.app.tree_sel { p.rect_filled(rect, 0.0, SEL()); p.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 0.0, deck_color(0)); }
                if scroll && k == self.app.tree_sel { ui.scroll_to_rect(rect, None); }
                let x0 = rect.left() + 6.0 + r.depth as f32 * 12.0;
                let folderish = matches!(r.kind, RowKind::Folder | RowKind::DevFolder | RowKind::Dev);
                let open = folderish && self.app.lists.open.contains(&Lists::open_key(r));
                let caret_rect = Rect::from_min_size(pos2(x0, rect.top()), vec2(14.0, rect.height()));
                if folderish { p.text(caret_rect.center(), egui::Align2::CENTER_CENTER, if open { "▾" } else { "▸" }, prop(10.0), MUTED()); }
                let icon = match r.kind { RowKind::All => "♫", RowKind::Pl | RowKind::DevPl => "≡", RowKind::Dev => "⏏", _ => "" };
                p.text(pos2(x0 + 20.0, rect.center().y), egui::Align2::CENTER_CENTER, icon, prop(12.0), MUTED());
                let ro = matches!(r.kind, RowKind::Dev | RowKind::DevPl | RowKind::DevFolder);
                p.text(pos2(x0 + 30.0, rect.center().y), egui::Align2::LEFT_CENTER, &r.label, prop(13.0), if ro { Color32::from_rgb(0xc9, 0xd3, 0xe0) } else { FG() });
                if let Some(c) = r.count { p.text(pos2(rect.right() - 30.0, rect.center().y), egui::Align2::RIGHT_CENTER, c.to_string(), mono(11.0), MUTED()); }
                // row actions on hover: × delete / ⏏ close / ⧉ copy
                let xr = Rect::from_min_size(pos2(rect.right() - 24.0, rect.top()), vec2(20.0, rect.height()));
                let hovering_x = resp.hover_pos().is_some_and(|hp| xr.contains(hp));
                if resp.hovered() || is_view {
                    let sym = match r.kind { RowKind::Pl | RowKind::Folder | RowKind::Dev => "×", RowKind::DevPl => "+", _ => "" };
                    if !sym.is_empty() { p.text(xr.center(), egui::Align2::CENTER_CENTER, sym, prop(14.0), if hovering_x { FG() } else { MUTED() }); }
                }
                if resp.clicked() {
                    self.app.tree_sel = k;
                    self.app.focus_tree = false;
                    let on_caret = resp.interact_pointer_pos().is_some_and(|hp| caret_rect.contains(hp));
                    if hovering_x {
                        match r.kind {
                            RowKind::Pl | RowKind::Folder => self.app.ui.confirm_delete = Some(r.id.clone()),
                            RowKind::Dev => { let d = r.dev.clone(); self.app.close_device(&d); }
                            RowKind::DevPl => { let (d, n) = (r.dev.clone(), r.rb); self.app.copy_device_playlist(&d, n); }
                            _ => {}
                        }
                    } else if on_caret && folderish { self.app.lists.toggle_open(r); }
                    else { self.app.open_row(r); }
                }
                if resp.double_clicked() && matches!(r.kind, RowKind::Pl | RowKind::Folder) { self.app.ui.renaming = Some((r.id.clone(), r.label.clone(), false)); }
                // drag a playlist onto a folder; drop tracks onto a playlist
                if matches!(r.kind, RowKind::Pl | RowKind::Folder) { resp.dnd_set_drag_payload(DragList(r.id.clone())); }
                if r.kind == RowKind::Pl {
                    if resp.dnd_hover_payload::<DragRows>().is_some() { ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.5_f32, deck_color(0)), StrokeKind::Inside); }
                    if let Some(p) = resp.dnd_release_payload::<DragRows>() {
                        let tracks: Vec<Track> = p.ids.iter().filter_map(|id| self.app.track(id).cloned()).collect();
                        let id = r.id.clone();
                        self.app.add_to_playlist(&id, &tracks);
                    }
                }
                if r.kind == RowKind::Folder {
                    if let Some(p) = resp.dnd_release_payload::<DragList>() { let (a, b) = (p.0.clone(), r.id.clone()); if a != b { self.app.lists.move_to(&a, Some(b)); } }
                    if resp.dnd_hover_payload::<DragList>().is_some() { ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.5_f32, deck_color(0)), StrokeKind::Inside); }
                }
            }
            ui.add_space(10.0);
        });
        if focus { ui.painter().rect_stroke(ui.max_rect(), 0.0, Stroke::new(1.0_f32, deck_color(0)), StrokeKind::Inside); }
    }

    fn track_list(&mut self, ui: &mut Ui) {
        let w = ui.available_width();
        // columns: #, Title, Artist, Playlists, BPM, Key, Time, Deck
        let fixed = 44.0 + 72.0 + 60.0 + 64.0 + 52.0;
        let flex = (w - fixed).max(200.0);
        let cols = [44.0, flex * 0.45, flex * 0.3, flex * 0.25, 72.0, 60.0, 64.0, 52.0];
        let fp = self.app.lists.fingerprint();
        if fp != self.pl_names.0 { self.pl_names = (fp, self.app.lists.membership()); }
        let xs: Vec<f32> = cols.iter().scan(0.0, |acc, c| { let x = *acc; *acc += c; Some(x) }).collect();
        // header
        let (hr, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
        ui.painter().rect_filled(hr, 0.0, RAISE());
        let heads: [(&str, Option<SortKey>); 8] = [("#", None), ("TITLE", Some(SortKey::Title)), ("ARTIST", Some(SortKey::Artist)), ("PLAYLISTS", None), ("BPM", Some(SortKey::Bpm)), ("KEY", Some(SortKey::Key)), ("TIME", Some(SortKey::Time)), ("DECK", None)];
        for (k, (label, key)) in heads.iter().enumerate() {
            let r = Rect::from_min_size(pos2(hr.left() + xs[k], hr.top()), vec2(cols[k], hr.height()));
            let resp = ui.interact(r, ui.id().with(("hdr", k)), Sense::click());
            let sorted = key.is_some() && self.app.list.sort == *key;
            let txt = format!("{label}{}", if sorted { if self.app.list.dir > 0 { " ▴" } else { " ▾" } } else { "" });
            ui.painter().text(pos2(r.left() + 8.0, r.center().y), egui::Align2::LEFT_CENTER, txt, prop(10.5), if sorted { FG() } else { MUTED() });
            if resp.clicked() {
                match key {
                    Some(k) => { self.app.list.dir = if self.app.list.sort == Some(*k) { -self.app.list.dir } else { 1 }; self.app.list.sort = Some(*k); }
                    None if k == 0 => self.app.list.sort = None, // back to the list's own order
                    None => {}
                }
                self.app.apply_view(true);
            }
        }
        if self.app.list.view.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                if self.app.tracks.is_empty() && self.app.view == View::All {
                    ui.label(egui::RichText::new("No music yet.").strong());
                    ui.label("Click Add music folder and pick the folder you DJ from. Cratebox reads it in place: nothing is copied.");
                    ui.label("You can also drop audio files or folders onto the window.");
                } else { ui.label(egui::RichText::new("Nothing here").color(MUTED())); }
            });
            return;
        }
        // MATCH follows the playing deck: re-sort when its key changes or its tempo moves by a BPM or more
        if self.app.list.sort == Some(SortKey::Match) {
            let (k, b) = self.app.mix_ref();
            let (k0, b0) = self.app.list.mix_ref;
            if k != k0 || (b - b0).abs() >= 1.0 { self.app.apply_view(true); }
        }
        let n = self.app.list.view.len();
        let ref_key = self.ref_key();
        let mut sa = egui::ScrollArea::vertical().id_salt("list").auto_shrink([false, false]);
        if std::mem::take(&mut self.app.list.scroll_to_sel) {
            let sel = self.app.list.sel as f32 * ROW;
            let vis = ui.available_height();
            let cur = ui.ctx().memory(|m| m.data.get_temp::<f32>(ui.id().with("listoff"))).unwrap_or(0.0);
            let off = if sel < cur { sel } else if sel + ROW > cur + vis { sel + ROW - vis } else { cur };
            sa = sa.vertical_scroll_offset(off);
        }
        let own_pl = self.app.is_own_playlist_view();
        let out = sa.show_rows(ui, ROW, n, |ui, range| {
            for k in range {
                let t = self.app.list.view[k].clone();
                let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW), Sense::click_and_drag());
                self.rects.insert(format!("row{k}"), rect);
                let p = ui.painter();
                let sel = k == self.app.list.sel;
                let msel = self.app.list.sel_ids.contains(&t.id);
                if sel { p.rect_filled(rect, 0.0, SEL()); }
                else if msel { p.rect_filled(rect, 0.0, SEL2()); }
                else if resp.hovered() { p.rect_filled(rect, 0.0, HOVER()); }
                p.hline(rect.x_range(), rect.bottom(), Stroke::new(1.0_f32, Color32::from_rgb(0x19, 0x1c, 0x22)));
                let on_deck = self.app.decks.iter().position(|d| d.track.as_ref().is_some_and(|x| x.id == t.id));
                let dim = t.missing;
                let tc = if dim { DIM() } else if let Some(d) = on_deck { deck_color(d) } else { FG() };
                let cell = |c: usize, txt: String, color: Color32, monof: bool| {
                    let r = Rect::from_min_size(pos2(rect.left() + xs[c] + 8.0, rect.top()), vec2(cols[c] - 12.0, ROW));
                    let font = if monof { mono(12.0) } else { prop(13.0) };
                    let g = p.layout_no_wrap(txt, font, color);
                    let pp = p.with_clip_rect(r);
                    pp.galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, color);
                };
                cell(0, format!("{}", self.app.list.view_pos.get(k).copied().unwrap_or(k) + 1), MUTED(), true);
                cell(1, t.title.clone(), tc, false);
                cell(2, t.artist.clone(), if dim { DIM() } else { FG() }, false);
                cell(3, self.pl_names.1.get(&t.id).cloned().unwrap_or_default(), MUTED(), false);
                let bpm = match t.bpm { Some(b) if b > 0.0 => if b.fract().abs() > 1e-9 { format!("{b:.2}") } else { format!("{b:.0}") }, Some(_) => "?".into(), None if t.failed => "?".into(), None => "…".into() };
                cell(4, bpm, if t.grid_manual { LOOP } else { MUTED() }, true);
                match t.key.and_then(camelot) {
                    Some(c) => {
                        // mixes with the playing deck: same key bright, a neighbour (±1, A⇄B) softer
                        let a = if ref_key == Some(c) { 90 } else if keys_match(Some(c), ref_key) { 50 } else { 0 };
                        if a > 0 {
                            let kr = Rect::from_min_size(pos2(rect.left() + xs[5] + 4.0, rect.top() + 3.0), vec2(36.0, ROW - 6.0));
                            p.rect_filled(kr, 3.0, Color32::from_rgba_unmultiplied(80, 220, 140, a));
                            p.rect_stroke(kr, 3.0, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(80, 220, 140, a + 60)), StrokeKind::Inside);
                        }
                        cell(5, c.label(), rgb(c.color()), true);
                    }
                    None => cell(5, if t.key == Some(-1) || t.failed { "?".into() } else if t.bpm.is_none() { "…".into() } else { String::new() }, MUTED(), true),
                }
                cell(6, t.duration.map(|d| { let s = fmt_time(d); s[..s.len() - 2].to_string() }).unwrap_or_default(), MUTED(), true);
                cell(7, on_deck.map(|d| (d + 1).to_string()).unwrap_or_default(), MUTED(), true);
                if resp.clicked() {
                    let m = ui.input(|x| x.modifiers);
                    self.app.list.click(k, m.command || m.ctrl, m.shift);
                    self.app.focus_tree = false;
                }
                if resp.double_clicked() { let deck = self.load_target(); self.app.list.sel = k; self.app.load_selected(deck); }
                if resp.drag_started() {
                    let in_sel = msel && self.app.list.sel_ids.len() > 1;
                    let rows: Vec<usize> = if in_sel { (0..n).filter(|&j| self.app.list.sel_ids.contains(&self.app.list.view[j].id)).collect() } else { vec![k] };
                    let ids = rows.iter().map(|&j| &self.app.list.view[j]).filter(|x| !x.missing).map(|x| x.id.clone()).collect();
                    let positions = if own_pl { Some(rows.iter().map(|&j| self.app.list.view_pos[j]).collect()) } else { None };
                    resp.dnd_set_drag_payload(DragRows { ids, positions });
                }
                // rearrange an own playlist by dragging rows
                if own_pl {
                    if let Some(pl) = resp.dnd_hover_payload::<DragRows>() {
                        if pl.positions.is_some() {
                            let below = resp.hover_pos().is_some_and(|hp| hp.y > rect.center().y);
                            let y = if below { rect.bottom() - 1.0 } else { rect.top() + 1.0 };
                            ui.painter().hline(rect.x_range(), y, Stroke::new(2.0_f32, deck_color(0)));
                        }
                    }
                    if let Some(pl) = resp.dnd_release_payload::<DragRows>() {
                        if let Some(pos) = &pl.positions {
                            let below = ui.input(|x| x.pointer.interact_pos()).is_some_and(|hp| hp.y > rect.center().y);
                            let to = self.app.list.view_pos[k] + below as usize;
                            let pos = pos.clone();
                            self.app.move_in_playlist(&pos, to);
                        }
                    }
                }
                resp.context_menu(|ui| {
                    if self.app.list.sel != k && !msel { self.app.list.click(k, false, false); }
                    let shown: Vec<usize> = (0..DECKS).filter(|&d| self.app.deck_active(d)).collect();
                    for d in shown {
                        if ui.button(format!("Load on deck {}", d + 1)).clicked() { self.app.list.sel = k; self.app.load_selected(d); ui.close(); }
                    }
                    let sel = self.app.selected_tracks();
                    ui.menu_button("Add to playlist", |ui| {
                        let own: Vec<(String, String)> = self.app.lists.lists.iter().filter(|p| !p.folder).map(|p| (p.id.clone(), self.app.lists.path_name(p))).collect();
                        if own.is_empty() { ui.label("No playlists yet"); }
                        for (id, name) in own { if ui.button(name).clicked() { self.app.add_to_playlist(&id, &sel); ui.close(); } }
                    });
                    if matches!(self.app.view, View::Pl(_)) && ui.button("Remove from playlist").clicked() { self.app.remove_from_playlist(); ui.close(); }
                    if let Some(tr) = self.app.track(&t.id) {
                        if ui.button("Show in folder").clicked() { if let Some(dir) = tr.path.parent() { let _ = std::process::Command::new("xdg-open").arg(dir).spawn(); } ui.close(); }
                    }
                });
            }
        });
        ui.ctx().memory_mut(|m| m.data.insert_temp(ui.id().with("listoff"), out.state.offset.y));
    }
    /// the key to mix into: the deck that started playing most recently (else the first loaded one)
    fn ref_key(&self) -> Option<crate::keys::Camelot> { self.app.mix_ref().0 }

    // ===================================================================== settings + learn bar
    fn settings(&mut self, ctx: &egui::Context) {
        let mut open = self.app.ui.settings_open;
        let max_h = (ctx.screen_rect().height() - 90.0).max(200.0);
        egui::Window::new("Settings").open(&mut open).resizable(true).default_width(480.0).collapsible(false).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                ui.heading("Look");
                ui.horizontal_wrapped(|ui| {
                    for t in THEMES.iter() {
                        let on = theme().name == t.name;
                        let r = ui.add(egui::Button::new(egui::RichText::new(t.name).color(if on { FG() } else { MUTED() })).selected(on).min_size(vec2(84.0, 26.0)));
                        // the theme's four deck colours under its name
                        let y = r.rect.bottom() - 4.0;
                        for k in 0..4 { ui.painter().rect_filled(Rect::from_min_size(pos2(r.rect.left() + 8.0 + k as f32 * 17.0, y), vec2(15.0, 2.0)), 0.0, t.deck(k)); }
                        if r.clicked() { set_theme(t.name); style(ui.ctx()); self.app.store.settings.theme = t.name.to_string(); self.app.store.dirty_settings = true; }
                    }
                });
                ui.separator();
                ui.heading("Library");
                let s = &self.app.store.settings;
                let (mut br, custom) = (s.bpm_range, crate::analysis::BPM_CUSTOM);
                let name = if br >= custom { "Custom" } else { crate::analysis::BPM_RANGES[br].0 };
                egui::ComboBox::from_label("BPM range (where half/double-time tracks land)").selected_text(name).show_ui(ui, |ui| {
                    for (k, r) in crate::analysis::BPM_RANGES.iter().enumerate() { ui.selectable_value(&mut br, k, r.0); }
                    ui.selectable_value(&mut br, custom, "Custom");
                });
                if br != s.bpm_range { self.app.set_bpm_range(br); }
                if br >= custom {
                    // the top end is kept at least an octave above the bottom, so every tempo has a place
                    let (mut lo, mut hi) = (self.app.store.settings.bpm_lo, self.app.store.settings.bpm_hi);
                    let mut done = false;
                    ui.horizontal(|ui| {
                        ui.label("From");
                        let r = ui.add(egui::DragValue::new(&mut lo).range(40.0..=150.0).speed(0.5).suffix(" BPM")); done |= r.lost_focus() || r.drag_stopped();
                        ui.label("to");
                        let r = ui.add(egui::DragValue::new(&mut hi).range(79.0..=300.0).speed(0.5).suffix(" BPM")); done |= r.lost_focus() || r.drag_stopped();
                    });
                    let (l, h) = crate::analysis::custom_range(lo, hi);
                    self.app.store.settings.bpm_lo = l;
                    self.app.store.settings.bpm_hi = h - 1.0;
                    if done { self.app.set_bpm_range(custom); }
                }
                let mut dv = self.app.store.settings.deck_view.clone();
                let dv_name = |v: &str| match v { "screens" => "Screens only", "controls" => "Screens and buttons", _ => "Auto (screens while a controller is connected)" };
                egui::ComboBox::from_label("Deck area").selected_text(dv_name(&dv)).show_ui(ui, |ui| {
                    for v in ["auto", "screens", "controls"] { ui.selectable_value(&mut dv, v.to_string(), dv_name(v)); }
                });
                if dv != self.app.store.settings.deck_view { self.app.store.settings.deck_view = dv; self.app.store.dirty_settings = true; }
                let mut ws = self.app.store.settings.wave_style.clone();
                egui::ComboBox::from_label("Waveform").selected_text(if ws == "3band" { "3-band" } else { "RGB (red lows, green mids, blue highs)" }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut ws, "rgb".to_string(), "RGB (red lows, green mids, blue highs)");
                    ui.selectable_value(&mut ws, "3band".to_string(), "3-band");
                });
                if ws != self.app.store.settings.wave_style { self.app.store.settings.wave_style = ws; self.app.store.dirty_settings = true; }
                let mut spin = self.app.store.settings.spin;
                let spins = [(0.8, "Short"), (1.6, "Medium"), (2.5, "Long"), (4.0, "Extra long"), (7.0, "Ridiculous")];
                egui::ComboBox::from_label("Backspin (spin time after a fling)").selected_text(spins.iter().find(|x| (x.0 - spin).abs() < 1e-6).map(|x| x.1).unwrap_or("Long")).show_ui(ui, |ui| {
                    for (v, l) in spins { ui.selectable_value(&mut spin, v, l); }
                });
                if (spin - self.app.store.settings.spin).abs() > 1e-9 { self.app.set_spin(spin); }
                ui.horizontal(|ui| {
                    let r = ui.add(egui::Slider::new(&mut self.app.store.settings.wave_offset_ms, -100.0..=100.0).step_by(1.0).suffix(" ms").text("Waveform timing"));
                    if r.changed() { self.app.store.dirty_settings = true; }
                    r.on_hover_text("Cratebox already allows for the sound card's delay. If the waveform still runs ahead of what you hear, move this up; if it trails, move it down");
                });
                if ui.checkbox(&mut self.app.store.settings.show_fps, "Show frame rate in the header").on_hover_text("Frames per second and missed screen refreshes over the last 5 s: 60 FPS · 0 MISSED is smooth").changed() { self.app.store.dirty_settings = true; }
                if ui.checkbox(&mut self.app.store.settings.browse_close_on_load, "Back to the decks after loading a track from BROWSE").changed() { self.app.store.dirty_settings = true; }
                ui.label(egui::RichText::new("Music folders").strong());
                for f in self.app.store.settings.folders.clone() {
                    ui.horizontal(|ui| {
                        ui.label(f.display().to_string());
                        if ui.small_button("Rescan").clicked() { self.app.scan_folder(&f, false); }
                        if ui.small_button("Remove").clicked() { self.app.remove_music_folder(&f); }
                    });
                }
                if ui.button("Add music folder…").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::AddFolder, None)); }

                ui.separator();
                ui.heading("Sound");
                let cur = self.app.audio.device.clone();
                let mut pick = cur.clone();
                egui::ComboBox::from_label("Output").width(300.0).selected_text(if cur.is_empty() { "System default".to_string() } else { short_dev(&cur) }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut pick, String::new(), "System default");
                    for d in &self.app.outputs { ui.selectable_value(&mut pick, d.name.clone(), format!("{}  ({} ch)", d.name, d.max_channels)); }
                });
                if pick != cur { self.app.set_output(&pick); }
                if ui.small_button("Refresh devices").clicked() { self.app.refresh_outputs(); }
                let mut hp = self.app.store.settings.hp_mode.clone();
                let modes = [("auto", "Auto (FLX4: channels 3–4)"), ("quad", "Same device, channels 3–4"), ("split", "Split: master left, cue right"), ("device", "Another device"), ("off", "Off")];
                egui::ComboBox::from_label("Headphones").selected_text(modes.iter().find(|m| m.0 == hp).map(|m| m.1).unwrap_or("Auto")).show_ui(ui, |ui| {
                    for (v, l) in modes { ui.selectable_value(&mut hp, v.to_string(), l); }
                });
                if hp != self.app.store.settings.hp_mode { self.app.store.settings.hp_mode = hp.clone(); self.app.store.dirty_settings = true; self.app.apply_headphones(true); }
                if hp == "device" {
                    let mut hd = self.app.store.settings.hp_device.clone();
                    egui::ComboBox::from_label("Headphone device").width(300.0).selected_text(if hd.is_empty() { "System default".into() } else { short_dev(&hd) }).show_ui(ui, |ui| {
                        ui.selectable_value(&mut hd, String::new(), "System default");
                        for d in &self.app.outputs { ui.selectable_value(&mut hd, d.name.clone(), d.name.clone()); }
                    });
                    if hd != self.app.store.settings.hp_device { self.app.store.settings.hp_device = hd; self.app.store.dirty_settings = true; self.app.apply_headphones(true); }
                }
                ui.label(egui::RichText::new(&self.app.hp_note).size(11.5).color(MUTED()));
                let mut buf = self.app.store.settings.buffer;
                egui::ComboBox::from_label("Audio buffer (lower = tighter jog, higher = no dropouts)").selected_text(format!("{buf} samples ({:.1} ms)", buf as f64 / self.app.audio.sr * 1000.0)).show_ui(ui, |ui| {
                    for b in [128u32, 256, 512, 1024, 2048] { ui.selectable_value(&mut buf, b, format!("{b} samples ({:.1} ms)", b as f64 / self.app.audio.sr * 1000.0)); }
                });
                if buf != self.app.store.settings.buffer { self.app.store.settings.buffer = buf; self.app.store.dirty_settings = true; let d = self.app.audio.device.clone(); self.app.set_output(&d); }
                ui.label(egui::RichText::new(format!("{} Hz, {} channels", self.app.audio.sr, self.app.audio.channels)).size(11.0).color(DIM()));
                ui.horizontal(|ui| {
                    ui.label("Recordings go to:");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.app.store.settings.rec_dir).hint_text(crate::recorder::default_dir().display().to_string()).desired_width(260.0));
                    if r.changed() { self.app.store.dirty_settings = true; }
                }).response.on_hover_text("● REC in the header records the master (after the limiter) to a 16-bit WAV here");

                ui.separator();
                ui.heading("Controller");
                let ports = self.app.midi_io.ports.clone();
                let mut pin = self.app.midi_io.name.clone();
                egui::ComboBox::from_label("Controller").width(300.0).selected_text(if pin.is_empty() { "No controller connected".into() } else { pin.clone() }).show_ui(ui, |ui| {
                    if ports.is_empty() { ui.label("No MIDI devices"); }
                    for p in &ports { ui.selectable_value(&mut pin, p.clone(), p); }
                });
                if pin != self.app.midi_io.name && !pin.is_empty() { self.app.pick_midi_in(&pin); }
                let mut prof = self.app.midi.profile.to_string();
                let profs = midi::profiles();
                egui::ComboBox::from_label("Mapping").width(300.0).selected_text(profs.iter().find(|p| p.id == prof).map(|p| p.name).unwrap_or("")).show_ui(ui, |ui| {
                    for p in &profs { ui.selectable_value(&mut prof, p.id.to_string(), p.name); }
                });
                if prof != self.app.midi.profile { self.app.pick_profile(&prof); }
                let info = self.app.midi.profile_info();
                let n = self.app.midi.learned.len();
                ui.label(egui::RichText::new(format!("{}{}", midi::src_note(info.src), if n > 0 { format!(" You've mapped {n} control{} yourself.", if n == 1 { "" } else { "s" }) } else { String::new() })).size(11.5).color(MUTED()));
                ui.horizontal(|ui| {
                    if ui.button("MIDI learn…").clicked() { self.app.start_learn(); }
                    if ui.button("Forget learned controls").clicked() { self.app.clear_learned(); }
                    if ui.button("Export mapping…").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::ExportMapping, None)); }
                    if ui.button("Import mapping…").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::ImportMapping, None)); }
                });
                ui.horizontal(|ui| {
                    if ui.button("Set up layer buttons…").on_hover_text("Pick the two buttons that switch the controller's sides between decks 1⇄3 and 2⇄4 (FLX4: SMART CFX and SMART FADER)").clicked() {
                        self.app.midi.setup_layer_buttons();
                        self.app.ui.settings_open = false;
                    }
                    ui.label(egui::RichText::new("4-deck mode: which buttons switch sides").size(11.0).color(DIM()));
                });

                ui.separator();
                ui.heading("rekordbox");
                ui.horizontal(|ui| {
                    if ui.button("Open USB / SD…").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::OpenUsb, None)); }
                    if ui.button("Import XML…").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::ImportXml, None)); }
                    if ui.button("Export XML…").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::ExportXml, None)); }
                });
                ui.label(egui::RichText::new("Tracks that are also on a plugged-in rekordbox stick play on rekordbox's grid.").size(11.0).color(MUTED()));
                if ui.button("Compare Cratebox's grids with the stick").on_hover_text("Checks where Cratebox puts bar 1 against rekordbox, for tracks on both (writes grid-compare.log)").clicked() { self.app.compare_grids(); }
                ui.horizontal(|ui| {
                    ui.label("Export paths as if the music folders were in:");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.app.store.settings.base_path).hint_text("(leave empty: their real location)").desired_width(220.0));
                    if r.changed() { self.app.store.dirty_settings = true; }
                }).response.on_hover_text("For moving the XML to another computer, e.g. C:\\Users\\you\\Music");
                ui.separator();
                ui.label(egui::RichText::new(format!("Library data: {}", crate::store::data_dir().display())).size(11.0).color(DIM()));
            });
        });
        self.app.ui.settings_open = open && self.app.ui.settings_open;
    }

    fn learn_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            ui.label(egui::RichText::new("LEARN").strong().color(LOOP));
            ui.label(self.app.status_text().to_string());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Done").clicked() { self.app.stop_learn(); }
                for (label, a, d) in [("Jog side 2", "jogSide", 1i8), ("Jog side 1", "jogSide", 0), ("Load 2", "load", 1), ("Load 1", "load", 0), ("Browse press", "browsePress", -1), ("Browse knob", "browse", -1)] {
                    let picked = self.app.ui.learn_pick.as_ref().is_some_and(|(x, y)| x == a && *y == d);
                    if ui.selectable_label(picked, label).clicked() { self.app.learn_pick(a, d); }
                }
                ui.label(egui::RichText::new("Library:").color(MUTED()));
            });
        });
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() { return; }
        // presses and releases (not key repeat), so CUE and PAD FX can be held
        let evs: Vec<(Key, bool, egui::Modifiers)> = ctx.input(|i| i.events.iter().filter_map(|e| match e {
            egui::Event::Key { key, pressed, repeat: false, modifiers, .. } => Some((*key, *pressed, *modifiers)),
            _ => None,
        }).collect());
        let (down, up) = ctx.input(|i| (i.key_pressed(Key::ArrowDown), i.key_pressed(Key::ArrowUp)));
        if down { self.app.list.move_sel(1); }
        if up { self.app.list.move_sel(-1); }
        if (down || up) && self.wave_browse.is_some() { self.wave_browse = Some(std::time::Instant::now()); }
        let evs: Vec<_> = if self.wave_browse.is_some() {
            let n = self.app.deck_count();
            evs.into_iter().filter(|(key, pressed, m)| {
                let d = match key { Key::Num1 => Some(0), Key::Num2 => Some(1), Key::Num3 => Some(2), Key::Num4 => Some(3), _ => None };
                if m.any() { return true; }
                if let Some(d) = d.filter(|d| *d < n) {
                    if *pressed { self.app.load_selected(d); self.wave_browse = None; }
                    return false;
                }
                match key {
                    Key::Enter => { if *pressed { let d = self.load_target(); self.app.load_selected(d); self.wave_browse = None; } false }
                    Key::Escape => { if *pressed { self.wave_browse = None; } false }
                    _ => true,
                }
            }).collect()
        } else { evs };
        for (key, pressed, m) in evs {
            if !pressed {
                // let go: CUE preview, held PAD FX
                if let Some((side, pidx)) = deck_key(key) {
                    let i = self.app.midi.side[side];
                    match pidx { Some(p) => { if self.app.decks[i].pad_mode.fx_bank().is_some() { self.app.pad_up(i, p); } } None => if key == Key::Z || key == Key::M { self.app.decks[i].cue_up(); } }
                }
                continue;
            }
            if m.command || m.ctrl {
                match key {
                    // Command+Shift X / . come in through the window manager (remote_cmds); Ctrl+Shift does it too
                    Key::X if m.shift => self.beat1(0),
                    Key::Period if m.shift => self.beat1(1),
                    Key::A => self.app.list.sel_ids = self.app.list.view.iter().map(|t| t.id.clone()).collect(),
                    Key::F => self.focus_search = true,
                    Key::R => self.app.toggle_rec(),
                    _ => {}
                }
                continue;
            }
            match key {
                Key::ArrowLeft if m.shift => { let d = self.app.midi.side[0]; self.app.load_selected(d); }
                Key::ArrowRight if m.shift => { let d = self.app.midi.side[1]; self.app.load_selected(d); }
                Key::Enter => { let d = self.load_target(); self.app.load_selected(d); }
                Key::Delete | Key::Backspace => if matches!(self.app.view, View::Pl(_)) { self.app.remove_from_playlist(); },
                Key::Escape => { if self.app.learning() { self.app.stop_learn(); } self.app.ui.settings_open = false; self.heat_open = false; self.keys_open = false; self.power_open = false; }
                Key::F1 => { let w = &mut self.app.store.settings.wave; *w = !*w; self.app.store.dirty_settings = true; }
                Key::F2 => { let n = self.app.deck_count(); self.app.set_decks(if n == 4 { 2 } else { n + 1 }); }
                Key::F3 => { let b = &mut self.app.store.settings.browse; *b = !*b; self.app.store.dirty_settings = true; }
                Key::F4 => self.heat_open = !self.heat_open,
                Key::F11 => { let fs = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)); ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs)); }
                Key::F12 => self.keys_open = !self.keys_open,
                Key::Backtick => { if let Some(a) = self.app.midi.toggle_side(0) { self.app.midi_action(a); } }
                Key::Backslash => { if let Some(a) = self.app.midi.toggle_side(1) { self.app.midi_action(a); } }
                Key::B => { let on = self.app.fx.on; self.app.fx_on(!on); }
                Key::OpenBracket => self.app.fx_select(-1),
                Key::CloseBracket => self.app.fx_select(1),
                Key::Minus => self.app.fx_beat(-1),
                Key::Equals | Key::Plus => self.app.fx_beat(1),
                _ => {
                    let Some((side, pidx)) = deck_key(key) else { continue };
                    let i = self.app.midi.side[side];
                    if let Some(p) = pidx { self.app.pad_down(i, p, m.shift); continue; }
                    // Shift (or a Command combo, which arrives here without Command) never plays or jumps
                    if m.shift { continue; }
                    let d = &mut self.app.decks[i];
                    match key {
                        Key::Z | Key::M => d.cue_down(),
                        Key::X | Key::Comma => self.app.toggle_play(i),
                        Key::A | Key::J => { let s = d.sync; self.app.set_sync(i, !s); }
                        Key::S | Key::K => { if d.lp.on { d.reloop_exit(); } else { d.beat_loop(4.0); } }
                        Key::D | Key::L => d.loop_scale(0.5),
                        Key::F | Key::Semicolon => d.loop_scale(2.0),
                        Key::C | Key::Period => d.beat_jump(-4.0),
                        Key::V | Key::Slash => d.beat_jump(4.0),
                        _ => {}
                    }
                }
            }
        }
    }

    /// 5 %: a warning with OK. 3 %: stop every deck and eject sticks (a stick pulled by a dying laptop can
    /// corrupt), then ask: plugged in, or shut down cleanly now. Once each per discharge.
    fn battery_watch(&mut self) {
        let every = if self.battery.1.is_some_and(|(pc, ch)| !ch && pc <= 8) { 5 } else { 15 };
        if self.battery.0.elapsed().as_secs() < every { return; }
        self.battery = (std::time::Instant::now(), read_battery());
        let Some((pc, charging)) = self.battery.1 else { return };
        if charging { self.bat_alerted = 100; if self.bat_dialog == Some(5) { self.bat_dialog = None; } return; }
        if pc <= 3 && self.bat_alerted > 3 {
            self.bat_alerted = 3;
            self.app.stop_all();
            self.app.eject_all(None);
            self.bat_dialog = Some(3);
        } else if pc <= 5 && self.bat_alerted > 5 {
            self.bat_alerted = 5;
            self.bat_dialog = Some(5);
        }
    }
    fn battery_dialog(&mut self, ctx: &egui::Context) {
        let Some(level) = self.bat_dialog else { return };
        let pc = self.battery.1.map(|b| b.0).unwrap_or(level);
        egui::Window::new("battery").title_bar(false).resizable(false).collapsible(false).order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0)).fixed_size(vec2(420.0, 0.0))
            .frame(egui::Frame::window(&ctx.style()).fill(PANEL()).stroke(Stroke::new(2.0_f32, RED)))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                if level > 3 {
                    ui.label(egui::RichText::new(format!("Battery low: {pc}%")).font(bold(18.0)).color(RED));
                    ui.label(egui::RichText::new("Plug in now. At 3% Cratebox stops the music and ejects the USB stick so it can't be damaged.").size(13.0).color(FG()));
                    if button(ui, "OK", vec2(ui.available_width(), 32.0), false, FG(), 13.0).clicked() { self.bat_dialog = None; }
                } else {
                    ui.label(egui::RichText::new(format!("Battery critical: {pc}%")).font(bold(18.0)).color(RED));
                    ui.label(egui::RichText::new("Playback stopped and the USB stick was ejected so it can't be damaged if the laptop dies.").size(13.0).color(FG()));
                    let half = vec2((ui.available_width() - 8.0) / 2.0, 34.0);
                    ui.horizontal(|ui| {
                        if button(ui, "I've plugged in", half, false, deck_color(0), 13.0).clicked() {
                            self.bat_dialog = None;
                            self.battery = (std::time::Instant::now(), read_battery());
                            if !self.battery.1.is_some_and(|b| b.1) { self.app.say("Still on battery: check the charger", 6.0); }
                        }
                        if button(ui, "Shut down now", half, true, RED, 13.0).clicked() { self.bat_dialog = None; self.app.power_off("poweroff"); }
                    });
                }
            });
    }

    /// bar 1 at the playhead on the deck under that hand (side 0: deck 1 or 3, side 1: deck 2 or 4)
    fn beat1(&mut self, side: usize) {
        let i = self.app.midi.side[side];
        self.app.decks[i].downbeat_here();
    }
    fn remote_cmds(&mut self) {
        let cmds: Vec<String> = self.remote.as_ref().map(|r| r.try_iter().collect()).unwrap_or_default();
        for c in cmds {
            match c.as_str() {
                "beat1 left" => self.beat1(0),
                "beat1 right" => self.beat1(1),
                _ => eprintln!("cratebox: unknown command {c:?}"),
            }
        }
    }

    /// F12: the keyboard shortcuts
    fn keys_window(&mut self, ctx: &egui::Context) {
        let mut open = self.keys_open;
        egui::Window::new("Keyboard shortcuts").open(&mut open).collapsible(false).resizable(false).default_width(520.0).show(ctx, |ui| {
            egui::Grid::new("keys").num_columns(2).spacing(vec2(18.0, 3.0)).striped(true).show(ui, |ui| {
                for (k, what) in SHORTCUTS {
                    if k.is_empty() { ui.label(egui::RichText::new(*what).font(bold(12.0)).color(LOOP)); ui.label(""); ui.end_row(); continue; }
                    ui.label(egui::RichText::new(*k).font(mono(11.5)).color(FG()));
                    ui.label(egui::RichText::new(*what).size(11.5).color(MUTED()));
                    ui.end_row();
                }
            });
        });
        self.keys_open = open;
    }

    fn dropped_files(&mut self, ctx: &egui::Context) {
        let files: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if files.is_empty() { return; }
        let pos = ctx.input(|i| i.pointer.hover_pos());
        let target = pos.and_then(|p| (0..DECKS).filter(|&k| self.app.deck_active(k)).find(|&k| self.deck_rects[k].contains(p)));
        for f in files {
            if f.is_dir() {
                if f.join("PIONEER").is_dir() { self.app.mount(&f, true); } else { self.app.add_music_folder(&f); }
            } else if let Some(id) = self.app.add_loose_file(&f) {
                if let Some(k) = target { self.app.load_id(k, &id); }
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(d) = self.app.ui.dialog.as_mut() else { return };
        let Some(res) = d.show(ctx) else { return };
        let purpose = d.purpose;
        self.app.ui.dialog = None;
        let Some(p) = res else { return };
        match purpose {
            Purpose::AddFolder => self.app.add_music_folder(&p),
            Purpose::OpenUsb => self.app.mount(&p, true),
            Purpose::ImportXml => self.app.import_xml(&p),
            Purpose::ExportXml => self.app.export_xml(&p),
            Purpose::ImportMapping => self.app.import_mapping(&p),
            Purpose::ExportMapping => self.app.export_mapping(&p),
        }
    }
}

impl Gui {
    /// double-click: the first shown deck that isn't playing (the left one if they all are)
    fn load_target(&self) -> usize {
        let shown: Vec<usize> = (0..DECKS).filter(|&d| self.app.deck_active(d)).collect();
        shown.iter().copied().find(|&d| !self.app.decks[d].playing && self.app.decks[d].track.is_none())
            .or_else(|| shown.iter().copied().find(|&d| !self.app.decks[d].playing)).unwrap_or(0)
    }
    /// TrueCurve mini: device, curve, Start; HOLD for PUFF. Once something starts the panel closes and the
    /// HEAT button carries the bowl temperature.
    /// OFF: leave the Cratebox session (back to the login screen on Cratebox OS), restart or shut down.
    /// Restart and shut down ask once more, so a stray tap can't end a set; they go through logind
    /// (systemctl), which lets the logged-in user do it without a password.
    /// WAVE mode hides the library; scrolling (browse knob, arrow keys) pops it up over the lanes.
    /// 1-4 load the highlighted track on that deck, Enter on the usual one, Esc closes; it goes after 6 s idle.
    fn wave_browser(&mut self, ctx: &egui::Context) {
        // a track went onto a deck (keys, mouse or the controller's LOAD): close
        if self.app.loads != self.seen_loads { self.seen_loads = self.app.loads; self.wave_browse = None; }
        let sel = self.app.list.sel;
        let moved = sel != self.last_sel && self.last_sel != usize::MAX;
        self.last_sel = sel;
        let s = &self.app.store.settings;
        if !(s.wave || (s.lib_hidden && !s.browse)) { self.wave_browse = None; return; }
        if moved { self.wave_browse = Some(std::time::Instant::now()); }
        let Some(t0) = self.wave_browse else { return };
        if t0.elapsed().as_secs_f32() > 6.0 { self.wave_browse = None; return; }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        let rows = 11usize;
        let view = &self.app.list.view;
        let start = sel.saturating_sub(rows / 2).min(view.len().saturating_sub(rows));
        egui::Window::new("wave_browser").title_bar(false).resizable(false).collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0)).fixed_size(vec2(560.0, 0.0))
            .frame(egui::Frame::window(&ctx.style()).fill(PANEL()))
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(format!("LOAD · {}", self.app.lists.view_name(&self.app.view))).strong().color(FG()));
                ui.add_space(4.0);
                if view.is_empty() { ui.label(egui::RichText::new("Nothing in this view").color(MUTED())); }
                for (k, t) in view.iter().enumerate().skip(start).take(rows) {
                    let on = k == sel;
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
                    let p = ui.painter();
                    if on { p.rect_filled(r, 3.0, SEL()); p.rect_filled(Rect::from_min_size(r.min, vec2(3.0, r.height())), 0.0, deck_color(0)); }
                    let c = if on { FG() } else { MUTED() };
                    let name = if t.artist.is_empty() { t.title.clone() } else { format!("{} – {}", t.title, t.artist) };
                    p.with_clip_rect(Rect::from_min_max(r.min, pos2(r.right() - 110.0, r.bottom())))
                        .text(pos2(r.left() + 10.0, r.center().y), egui::Align2::LEFT_CENTER, name, prop(13.0), c);
                    let bpm = t.bpm.map(|b| format!("{b:.1}")).unwrap_or_default();
                    let key = t.key.and_then(camelot).map(|c| c.label()).unwrap_or_default();
                    p.text(pos2(r.right() - 50.0, r.center().y), egui::Align2::RIGHT_CENTER, bpm, mono(12.0), c);
                    p.text(pos2(r.right() - 6.0, r.center().y), egui::Align2::RIGHT_CENTER, key, mono(12.0), c);
                }
                ui.add_space(4.0);
                ui.label(egui::RichText::new(format!("{} load on that deck · Enter loads · Esc closes", (1..=self.app.deck_count()).map(|d| d.to_string()).collect::<Vec<_>>().join(" "))).size(11.0).color(MUTED()));
            });
    }

    fn power_menu(&mut self, ctx: &egui::Context) {
        let anchor = self.rects.get("power").map(|r| pos2(r.right(), r.bottom() + 6.0)).unwrap_or(pos2(1100.0, 40.0));
        let mut open = true;
        egui::Window::new("Power").open(&mut open).collapsible(false).resizable(false).title_bar(false)
            .fixed_pos(pos2(anchor.x - 220.0, anchor.y)).fixed_size(vec2(220.0, 0.0)).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            let playing = self.app.decks.iter().any(|d| d.playing);
            if playing { ui.label(egui::RichText::new("A deck is playing.").size(11.0).color(CUE)); }
            let w = vec2(ui.available_width(), 28.0);
            if let Some(what) = self.power_confirm {
                ui.label(egui::RichText::new(format!("{what} now?")).size(13.0).color(FG()));
                ui.horizontal(|ui| {
                    let half = vec2((w.x - 6.0) / 2.0, 28.0);
                    if button(ui, what, half, true, RED, 12.0).clicked() {
                        // empty the decks and eject sticks first (App::power_off)
                        self.app.power_off(if what == "Shut down" { "poweroff" } else { "reboot" });
                        self.power_open = false;
                    }
                    if button(ui, "Cancel", half, false, FG(), 12.0).clicked() { self.power_confirm = None; }
                });
                return;
            }
            if button(ui, "Exit to login screen", w, false, FG(), 12.0).on_hover_text("Quit Cratebox (same as Alt+F4)").clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if button(ui, "Restart", w, false, FG(), 12.0).clicked() { self.power_confirm = Some("Restart"); }
            if button(ui, "Shut down", w, false, RED, 12.0).clicked() { self.power_confirm = Some("Shut down"); }
        });
        if !open { self.power_open = false; }
    }

    fn heat_panel(&mut self, ctx: &egui::Context) {
        let mut open = self.heat_open;
        let anchor = self.rects.get("heat").map(|r| pos2(r.right(), r.bottom() + 6.0)).unwrap_or(pos2(900.0, 40.0));
        let max_h = (ctx.screen_rect().height() - 60.0).max(200.0);
        let mut close = false;
        egui::Window::new("HEAT").open(&mut open).collapsible(false).resizable(false).fixed_pos(pos2(anchor.x - 340.0, anchor.y)).default_width(340.0).max_width(340.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            let v = self.app.heat.view.clone();
            let running = v.phase != crate::heat::Phase::Idle;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("TrueCurve").font(bold(14.0)));
                ui.label(egui::RichText::new("heat curves for Peak Pro / Proxy").size(11.0).color(MUTED()));
            });
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(egui::RichText::new(&v.dev_text).size(11.5).color(MUTED())).truncate());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if v.connected {
                        if ui.button("Disconnect").clicked() { self.app.heat.disconnect(); }
                    } else {
                        if ui.button("Simulator").clicked() { self.app.heat.use_simulator(); }
                        let r = ui.add_enabled(!v.scanning, egui::Button::new(if v.scanning { "Looking…" } else { "Connect…" }));
                        self.reg("heat.connect", r.rect);
                        if r.clicked() { let dev = self.app.store.settings.heat_device.clone(); self.app.heat.scan(&dev); }
                    }
                });
            });
            if !v.connected {
                for f in &v.found { if ui.add_sized(vec2(ui.available_width(), 24.0), egui::Button::new(format!("🔥 {}", f.name))).clicked() { self.app.heat.pick(f); } }
                if v.found.is_empty() && !v.others.is_empty() && !v.scanning {
                    ui.label(egui::RichText::new("Other devices nearby").size(11.0).color(MUTED()));
                    egui::ScrollArea::vertical().id_salt("heat_others").max_height(110.0).show(ui, |ui| {
                        for f in &v.others { if ui.add_sized(vec2(ui.available_width(), 22.0), egui::Button::new(&f.name)).clicked() { self.app.heat.pick(f); } }
                    });
                }
            }
            // ---- curve
            let (presets, own) = self.app.heat_names();
            let mut name = self.app.store.settings.heat_last.clone();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Curve").size(12.0).color(MUTED()));
                ui.add_enabled_ui(!running, |ui| {
                    egui::ComboBox::from_id_salt("heat_curve").width(240.0).selected_text(&name).show_ui(ui, |ui| {
                        for n in &presets { ui.selectable_value(&mut name, n.clone(), n); }
                        if !own.is_empty() { ui.separator(); ui.label(egui::RichText::new("From TrueCurve").size(10.5).color(MUTED())); }
                        for n in &own { ui.selectable_value(&mut name, n.clone(), n); }
                    });
                });
            });
            if name != self.app.store.settings.heat_last { self.app.store.settings.heat_last = name; self.app.store.dirty_settings = true; }
            let curve_on = running && v.phase != crate::heat::Phase::Hold;
            let can = v.connected && v.phase != crate::heat::Phase::Prep && v.phase != crate::heat::Phase::Hold;
            let r = ui.add_enabled(can, egui::Button::new(egui::RichText::new(if curve_on { "Stop" } else { "Start curve" }).font(bold(13.0)).color(BG()))
                .fill(if curve_on { RED } else { CUE }).min_size(vec2(ui.available_width(), 30.0)));
            self.reg("heat.start", r.rect);
            if r.clicked() { self.app.heat_toggle(); if !curve_on { close = true; } }
            // ---- HOLD for PUFF
            ui.separator();
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!running, |ui| {
                    let mut hf = self.app.store.settings.heat_hold_f;
                    ui.label(egui::RichText::new("Hold").size(12.0));
                    if ui.add(egui::DragValue::new(&mut hf).range(crate::heat::MIN_F..=crate::heat::MAX_F).speed(1.0).suffix("°F")).changed() {
                        self.app.store.settings.heat_hold_f = (hf / 5.0).round() * 5.0; self.app.store.dirty_settings = true;
                    }
                    let mut pf = self.app.store.settings.heat_puff_full;
                    ui.label(egui::RichText::new("full warble at").size(12.0).color(MUTED()));
                    egui::ComboBox::from_id_salt("heat_full").width(90.0).selected_text(if pf == 0.0 { "Auto".to_string() } else { format!("{pf:.0}°F drop") }).show_ui(ui, |ui| {
                        ui.selectable_value(&mut pf, 0.0, "Auto");
                        for d in [4.0, 8.0, 15.0, 25.0, 40.0] { ui.selectable_value(&mut pf, d, format!("{d:.0}°F drop")); }
                    });
                    if pf != self.app.store.settings.heat_puff_full { self.app.store.settings.heat_puff_full = pf; self.app.store.dirty_settings = true; }
                });
            });
            let holding = v.phase == crate::heat::Phase::Hold;
            let can = v.connected && v.phase != crate::heat::Phase::Prep && (!running || holding);
            let r = ui.add_enabled(can, egui::Button::new(egui::RichText::new(if holding { "Stop HOLD" } else { "HOLD for PUFF" }).font(bold(13.0)).color(BG()))
                .fill(if holding { RED } else { LOOP }).min_size(vec2(ui.available_width(), 30.0)));
            self.reg("heat.hold", r.rect);
            if r.clicked() { self.app.heat_hold(); if !holding && !v.is_sim { close = true; } }
            if holding {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
                ui.painter().rect_filled(rect, 3.0, RAISE());
                ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(rect.width() * v.puff as f32, rect.height())), 3.0, RED);
                if !v.puff_info.is_empty() { ui.label(egui::RichText::new(&v.puff_info).font(mono(10.5)).color(MUTED())); }
                if v.is_sim {
                    // press and hold: a pretend draw
                    let r = button(ui, if self.hitting { "Hitting…" } else { "Hold to hit (simulator)" }, vec2(ui.available_width(), 32.0), self.hitting, RED, 12.5);
                    let down = r.is_pointer_button_down_on();
                    if down != self.hitting { self.hitting = down; self.app.heat.hit(if down { 1.0 } else { 0.0 }); }
                }
            }
            ui.label(egui::RichText::new("Beat FX → PUFF (on offer while a Puffco or the simulator is connected). It plays like a theremin: a harder hit drops the bowl temperature further, and the voice glides higher (up to two octaves) and louder over a tape warble. LEVEL caps it.").size(10.5).color(MUTED()));
            if !v.msg.is_empty() { ui.label(egui::RichText::new(&v.msg).size(12.0)); }
            // ---- GLOW: the lantern as a slow rainbow that breathes with the music
            ui.separator();
            ui.horizontal(|ui| {
                let mut on = self.app.store.settings.heat_glow;
                if ui.checkbox(&mut on, egui::RichText::new("GLOW").font(bold(12.5))).on_hover_text("The Puffco's lantern drifts through the rainbow and brightens with the music. Paused during a heat run").changed() {
                    self.app.store.settings.heat_glow = on; self.app.store.dirty_settings = true;
                }
                let k = self.app.store.settings.heat_glow_knob.clone();
                let name = |k: &str| match k { "speed" => "speed", "sensitivity" => "sensitivity", "off" => "nothing", _ => "brightness" };
                ui.label(egui::RichText::new("FX knob sets").size(11.5).color(MUTED()));
                let mut nk = k.clone();
                egui::ComboBox::from_id_salt("glow_knob").width(96.0).selected_text(name(&k)).show_ui(ui, |ui| {
                    for x in ["intensity", "speed", "sensitivity", "off"] { ui.selectable_value(&mut nk, x.to_string(), name(x)); }
                });
                if nk != k { self.app.store.settings.heat_glow_knob = nk; self.app.store.dirty_settings = true; }
                if on && v.connected {
                    let c = self.app.glow.shown();
                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 7.0, Color32::from_rgb(c[0], c[1], c[2]));
                }
            });
            // ---- curves from TrueCurve
            ui.separator();
            ui.label(egui::RichText::new("Curves from TrueCurve: press \"Copy curves for Cratebox\" there, paste here").size(11.0).color(MUTED()));
            ui.add(egui::TextEdit::multiline(&mut self.heat_paste).desired_rows(2).desired_width(f32::INFINITY).font(mono(10.5)).hint_text("{\"My curve\": {\"points\": [...]}}"));
            if ui.add_enabled(!self.heat_paste.trim().is_empty(), egui::Button::new("Add curves")).clicked() {
                let m = self.app.heat_paste(&self.heat_paste.clone());
                if m.starts_with("Added") { self.heat_paste.clear(); }
                self.app.say(&m, 5.0);
            }
            ui.label(egui::RichText::new("Uses profile slot 4 while it runs and puts it back after; stops at 600°F. Close this and keep mixing: the HEAT button shows the bowl temperature.").size(10.5).color(DIM()));
            });
        });
        if close { open = false; }
        if !open && self.hitting { self.hitting = false; self.app.heat.hit(0.0); }
        self.heat_open = open;
        self.app.heat.set_panel(open);
    }

    /// CRATEBOX_DEMO=1: load two tracks, play one with a loop and hot cues (for screenshots)
    fn demo(&mut self) {
        // CRATEBOX_DEMO_HEAT=1: the simulator holding 500°F with the HEAT panel open
        if std::env::var("CRATEBOX_DEMO_HEAT").is_ok() {
            if self.frames == 3 { self.app.heat.use_simulator(); self.heat_open = true; }
            if self.app.heat.view.connected && !self.app.heat.running() && self.frames < 200 && self.frames % 20 == 0 { self.app.heat_hold(); }
        }
        let a = &mut self.app;
        let ready = a.tracks.len() >= 2 && a.tracks.iter().all(|t| t.bpm.is_some());
        if !ready { return; }
        if a.four() && a.tracks.len() >= 4 && a.decks[2].track.is_none() && !a.decks[2].loading && a.decks[0].track.is_some() {
            a.list.sel = 0; a.load_selected(2);
            a.list.sel = 3; a.load_selected(3);
        }
        if a.decks[0].track.is_none() && !a.decks[0].loading {
            a.list.sort = Some(SortKey::Bpm); a.apply_view(false);
            a.list.sel = 1; a.load_selected(0);
            a.list.sel = 2; a.load_selected(1);
            if a.lists.lists.is_empty() {
                let f = a.lists.create("Saturday", true, None);
                let p = a.lists.create("Peak time", false, Some(f));
                let tr: Vec<Track> = a.tracks.iter().take(3).cloned().collect();
                a.lists.add(&p, &tr, None);
                a.lists.create("Warmup", false, None);
            }
        }
        if a.decks[0].loaded() && a.decks[1].loaded() && !a.decks[0].playing && self.frames % 1000 > 0 && a.decks[0].hot[0].is_none() {
            a.decks[0].hotcue(0);
            let t = a.decks[0].now() + 8.0; a.decks[0].seek(t); a.decks[0].hotcue(1);
            a.toggle_play(0);
            a.set_sync(1, true);
            a.decks[1].beat_loop(4.0);
            a.mixer.pfl[1] = true; a.send_mixer();
            a.decks[1].pad_mode = PadMode::BeatLoop;
            a.decks[0].pad_mode = PadMode::PadFx;
        }
    }
}

/// a deck key: (controller side, Some(pad) for a pad key). Left hand plays the left side (deck 1, or 3 in
/// 4-deck mode after a layer switch), right hand the right side.
/// the "selected deck" pulse (mixer tabs, lanes, deck screens): 1.5 Hz between 35 % and full
fn strobe(ctx: &egui::Context) -> f32 {
    ctx.request_repaint();
    let t = ctx.input(|x| x.time);
    0.35 + 0.65 * (0.5 + 0.5 * (t * std::f64::consts::TAU * 1.5).cos()) as f32
}
/// CPU % (from /proc/stat between two samples) and memory in use (MemTotal - MemAvailable)
/// CPU, memory, Wi-Fi signal (% from /proc/net/wireless, None = not connected), Bluetooth on, and network
/// rates (bytes/s down, up) over every interface but lo
struct SysStats { at: std::time::Instant, last: (u64, u64), cpu: f32, mem_mb: u64, net_last: Option<(u64, u64)>, down: f64, up: f64, wifi: Option<u32>, bt: bool }
impl SysStats {
    fn new() -> Self { let mut s = SysStats { at: std::time::Instant::now(), last: (0, 0), cpu: 0.0, mem_mb: 0, net_last: None, down: 0.0, up: 0.0, wifi: None, bt: false }; s.sample(); s }
    fn sample(&mut self) {
        let dt = self.at.elapsed().as_secs_f64().max(0.1);
        self.at = std::time::Instant::now();
        if let Ok(n) = std::fs::read_to_string("/proc/net/dev") {
            let (mut rx, mut tx) = (0u64, 0u64);
            for l in n.lines().skip(2) {
                let Some((name, rest)) = l.split_once(':') else { continue };
                if name.trim() == "lo" { continue; }
                let v: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                if v.len() >= 9 { rx += v[0]; tx += v[8]; }
            }
            if let Some((r0, t0)) = self.net_last { self.down = rx.saturating_sub(r0) as f64 / dt; self.up = tx.saturating_sub(t0) as f64 / dt; }
            self.net_last = Some((rx, tx));
        }
        // link quality is out of 70 on most drivers (brcmfmac included)
        self.wifi = std::fs::read_to_string("/proc/net/wireless").ok().and_then(|w| w.lines().nth(2).and_then(|l| l.split_whitespace().nth(2)?.trim_end_matches('.').parse::<f64>().ok()))
            .map(|q| ((q / 70.0 * 100.0).round() as u32).min(100));
        self.bt = std::fs::read_dir("/sys/class/rfkill").map(|d| d.flatten().any(|e| {
            let r = |f: &str| std::fs::read_to_string(e.path().join(f)).unwrap_or_default();
            r("type").trim() == "bluetooth" && r("soft").trim() == "0" && r("hard").trim() == "0"
        })).unwrap_or(false);
        if let Some(l) = std::fs::read_to_string("/proc/stat").ok().and_then(|s| s.lines().next().map(str::to_string)) {
            let v: Vec<u64> = l.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
            if v.len() >= 5 {
                let idle = v[3] + v[4];
                let total: u64 = v.iter().take(8).sum();
                let (dt, di) = (total.saturating_sub(self.last.0), idle.saturating_sub(self.last.1));
                if self.last.0 > 0 && dt > 0 { self.cpu = 100.0 * (dt - di.min(dt)) as f32 / dt as f32; }
                self.last = (total, idle);
            }
        }
        if let Ok(m) = std::fs::read_to_string("/proc/meminfo") {
            let kb = |k: &str| m.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0);
            self.mem_mb = kb("MemTotal:").saturating_sub(kb("MemAvailable:")) / 1024;
        }
    }
}
/// the laptop battery from /sys/class/power_supply: (percent, charging or full on AC); None on a desktop
fn read_battery() -> Option<(u32, bool)> {
    // CRATEBOX_FAKE_BATTERY="4,0": percent, charging (screenshots and trying the alerts)
    if let Ok(f) = std::env::var("CRATEBOX_FAKE_BATTERY") { let mut it = f.split(','); return Some((it.next()?.trim().parse().ok()?, it.next().is_some_and(|c| c.trim() == "1"))); }
    for e in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let p = e.path();
        if std::fs::read_to_string(p.join("type")).map(|t| t.trim() == "Battery").unwrap_or(false) {
            let pc = std::fs::read_to_string(p.join("capacity")).ok()?.trim().parse().ok()?;
            let st = std::fs::read_to_string(p.join("status")).unwrap_or_default();
            return Some((pc, matches!(st.trim(), "Charging" | "Full" | "Not charging")));
        }
    }
    None
}
/// the power symbol: a ring open at the top with a bar through the gap
fn paint_power(p: &egui::Painter, r: Rect, c: Color32) {
    let (ctr, rad) = (r.center() + vec2(0.0, 0.5), 6.0);
    let pts: Vec<egui::Pos2> = (0..=24).map(|k| {
        let a = (-60.0_f32 + 300.0 * k as f32 / 24.0 - 90.0).to_radians() + std::f32::consts::PI;
        ctr + vec2(a.cos(), a.sin()) * rad
    }).collect();
    p.add(egui::Shape::line(pts, Stroke::new(1.7_f32, c)));
    p.line_segment([ctr + vec2(0.0, -rad - 1.0), ctr + vec2(0.0, -1.0)], Stroke::new(1.7_f32, c));
}
/// a padlock: shackle closed when locked, lifted open when not
fn paint_lock(p: &egui::Painter, r: Rect, locked: bool, c: Color32) {
    let body = Rect::from_center_size(pos2(r.center().x, r.center().y + 3.0), vec2(10.0, 8.0));
    p.rect_filled(body, 1.5, c);
    let (cx, top, lift) = (body.center().x, body.top(), if locked { 0.0 } else { 3.0 });
    let pts: Vec<egui::Pos2> = (0..=12).map(|k| {
        let a = std::f32::consts::PI * k as f32 / 12.0;
        pos2(cx - 3.2 * a.cos(), top - lift - 3.5 * a.sin())
    }).collect();
    let mut line = vec![pos2(cx - 3.2, top)];
    line.extend(pts);
    if locked { line.push(pos2(cx + 3.2, top)); }
    p.add(egui::Shape::line(line, Stroke::new(1.6_f32, c)));
}
fn deck_key(k: Key) -> Option<(usize, Option<usize>)> {
    use Key::*;
    let left = [Num1, Num2, Num3, Num4, Q, W, E, R];
    let right = [Num7, Num8, Num9, Num0, U, I, O, P];
    if let Some(p) = left.iter().position(|x| *x == k) { return Some((0, Some(p))); }
    if let Some(p) = right.iter().position(|x| *x == k) { return Some((1, Some(p))); }
    match k { Z | X | A | S | D | F | C | V => Some((0, None)), M | Comma | J | K | L | Semicolon | Period | Slash => Some((1, None)), _ => None }
}
/// the shortcut list (the F12 window; same as docs/keyboard.md)
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("", "Decks: left hand = left deck (1, or 3), right hand = right deck (2, or 4)"),
    ("X   ,", "Play / pause"),
    ("Z   M", "CUE (hold to preview)"),
    ("A   J", "SYNC"),
    ("S   K", "4-beat loop on / off"),
    ("D   L", "Loop ½"),
    ("F   ;", "Loop ×2"),
    ("C   .", "Beat jump back 4"),
    ("V   /", "Beat jump ahead 4"),
    ("⌘⇧X   ⌘⇧.", "Bar 1 at the playhead, left / right deck (Ctrl+Shift works too)"),
    ("1 2 3 4  Q W E R", "Left deck pads 1-8 (Shift: delete hot cue)"),
    ("7 8 9 0  U I O P", "Right deck pads 1-8 (Shift: delete hot cue)"),
    ("`   \\", "Layer: left side 1 ⇄ 3, right side 2 ⇄ 4 (4 decks)"),
    ("", "Beat FX"),
    ("B", "FX on / off"),
    ("[   ]", "Previous / next effect"),
    ("-   =", "Fewer / more beats"),
    ("", "Library"),
    ("↑ ↓", "Move through the tracks"),
    ("Enter", "Load onto the first free deck"),
    ("Shift ← / →", "Load onto the left / right deck"),
    ("Ctrl F", "Search"),
    ("Ctrl A", "Select all"),
    ("Delete", "Remove from this playlist"),
    ("", "Screen"),
    ("F1", "WAVE (just the waveforms)"),
    ("F2", "DECKS: 2 → 3 → 4"),
    ("F3", "BROWSE (big library)"),
    ("F4", "HEAT panel"),
    ("Ctrl R", "REC on / off"),
    ("↑ ↓", "WAVE mode: pops up the library; then 1 2 3 4 load on that deck"),
    ("F11", "Full screen on/off"),
    ("F12", "This list"),
    ("Esc", "Close panels, stop MIDI learn"),
    ("", "Trackpad"),
    ("Drag", "Knobs: up/down (Shift: fine) · faders: grab and slide"),
    ("Two-finger scroll", "Knobs and faders: nudge · waveform: zoom (sideways: scrub while paused)"),
    ("Double-click", "Knob / tempo / crossfader back to the middle"),
];

fn pill(ui: &mut Ui, text: &str, on: bool) -> Response {
    let g = ui.painter().layout_no_wrap(text.to_string(), prop(11.0), FG());
    let (rect, r) = ui.allocate_exact_size(vec2(g.size().x + 26.0, 20.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 10.0, RAISE());
    p.rect_stroke(rect, 10.0, Stroke::new(1.0_f32, LINE2()), StrokeKind::Inside);
    p.circle_filled(pos2(rect.left() + 11.0, rect.center().y), 3.5, if on { GREEN } else { DIM() });
    p.galley(pos2(rect.left() + 19.0, rect.center().y - g.size().y / 2.0), g, FG());
    r
}
fn short_dev(n: &str) -> String {
    // "hw:CARD=DDJFLX4,DEV=0" -> "DDJFLX4 (hw)"
    if let Some(c) = n.split("CARD=").nth(1) { let card = c.split(',').next().unwrap_or(c); let kind = n.split(':').next().unwrap_or(""); return format!("{card} ({kind})"); }
    n.to_string()
}
fn pad(ui: &mut Ui, text: &str, size: egui::Vec2, lit: bool, accent: Color32, font: f32) -> Response {
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, if lit { Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 46) } else if r.hovered() { Color32::from_rgb(0x23, 0x28, 0x31) } else { RAISE() });
    p.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, if lit { accent } else { LINE2() }), StrokeKind::Inside);
    p.with_clip_rect(rect).text(rect.center(), egui::Align2::CENTER_CENTER, text, prop(font), if lit { accent } else { FG() });
    r
}
/// an effect's name short enough for a pad
fn short_fx(fx: FxKind) -> &'static str {
    match fx.name() { "PING PONG" => "PING", "REVERB" => "VERB", "FLANGER" => "FLANG", "FILTER" => "FILT", n => n }
}

impl eframe::App for Gui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let _prof = FrameProf(std::time::Instant::now());
        crate::deck::frame_tick();
        self.frames += 1;
        self.app.poll();
        if std::env::var("CRATEBOX_DEMO").is_ok() { self.demo(); }
        self.keyboard(ctx);
        self.remote_cmds();
        self.battery_watch();
        self.dropped_files(ctx);
        egui::TopBottomPanel::top("header").exact_height(34.0).frame(egui::Frame::new().fill(PANEL()).inner_margin(egui::Margin::symmetric(10, 3))).show(ctx, |ui| self.header(ui));
        if self.app.learning() {
            egui::TopBottomPanel::bottom("learn").exact_height(34.0).frame(egui::Frame::new().fill(Color32::from_rgb(0x2a, 0x26, 0x10)).inner_margin(egui::Margin::symmetric(10, 3))).show(ctx, |ui| self.learn_bar(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG()).inner_margin(4.0)).show(ctx, |ui| {
            let full = ui.available_rect_before_wrap();
            let four = self.app.four();
            let shown: Vec<usize> = (0..DECKS).filter(|&i| self.app.deck_active(i)).collect();
            let n = shown.len() as f32;
            let s = &self.app.store.settings;
            let (wave, browse) = (s.wave, s.browse);
            let lib_off = s.lib_hidden && !browse;
            // the deck row keeps its size; dragging the library's top edge trades height between the
            // waveforms and the library (double-click: back to automatic)
            // screen-only decks need less height than buttons: the waveforms get the rest
            let row_h = match (four, self.screens()) {
                (true, false) => (full.height() * 0.46).clamp(300.0, 440.0),
                (true, true) => (full.height() * 0.42).clamp(280.0, 400.0),
                (false, false) => (full.height() * 0.47).clamp(260.0, 400.0),
                (false, true) => (full.height() * 0.42).clamp(280.0, 360.0),
            };
            let lib_h = s.lib_h;
            // ---- the waveforms, stacked 1 2 (3 4)
            let lane_h = if wave { (full.height() - (n - 1.0) * GAP) / n }
                else if lib_off { ((full.height() - row_h - 4.0 - GAP) - n * GAP) / n }
                else if browse { if four { 40.0 } else { 64.0 } }
                else if lib_h > 0.0 {
                    let spare = full.height() - row_h - 4.0 - GAP - n * GAP;
                    let min_lane = if four { 36.0 } else { 48.0 };
                    ((spare - lib_h) / n).clamp(min_lane, ((spare - 90.0) / n).max(min_lane))
                }
                else if four { (full.height() * 0.075).clamp(42.0, 72.0) } else { (full.height() * 0.12).clamp(70.0, 130.0) };
            for (k, &i) in shown.iter().enumerate() {
                let r = Rect::from_min_size(pos2(full.left(), full.top() + k as f32 * (lane_h + GAP)), vec2(full.width(), lane_h));
                self.lane(ui, i, r);
            }
            if wave { return; }
            let mut top = full.top() + n * (lane_h + GAP) + 2.0;
            // ---- decks either side of the mixer (4 decks: 1 over 3 on the left, 2 over 4 on the right)
            if !browse {
                let mixer_w = if four { 300.0 } else { 250.0 };
                let dw = (full.width() - mixer_w - 2.0 * GAP) / 2.0;
                let mx = Rect::from_min_size(pos2(full.left() + dw + GAP, top), vec2(mixer_w, row_h));
                let per = if four { (row_h - GAP) / 2.0 } else { row_h };
                for &i in &shown {
                    let x = if i % 2 == 0 { full.left() } else { full.right() - dw };
                    let y = top + if i >= 2 { per + GAP } else { 0.0 };
                    // 3 decks: deck 2 has the right side to itself
                    let h = if i == 1 && !self.app.deck_active(3) { row_h } else { per };
                    self.deck(ui, i, Rect::from_min_size(pos2(x, y), vec2(dw, h)));
                }
                self.mixer(ui, mx);
                top += row_h + GAP + 2.0;
                let handle = Rect::from_min_max(pos2(full.left(), top - GAP - 2.0), pos2(full.right(), top + 3.0));
                let r = ui.interact(handle, ui.id().with("lib_split"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::ResizeVertical);
                self.reg("lib_split", handle);
                if r.hovered() || r.dragged() { ui.painter().hline(handle.x_range(), handle.center().y, Stroke::new(2.0_f32, deck_color(0))); }
                if r.dragged() { if let Some(p) = r.interact_pointer_pos() { self.app.store.settings.lib_h = (full.bottom() - p.y).max(90.0); } }
                if r.drag_stopped() { self.app.store.dirty_settings = true; }
                if r.double_clicked() { self.app.store.settings.lib_h = 0.0; self.app.store.dirty_settings = true; }
            }
            if lib_off { return; }
            let lib = Rect::from_min_max(pos2(full.left(), top), full.max);
            let ui = &mut ui.new_child(UiBuilder::new().max_rect(lib).layout(Layout::top_down(Align::Min)));
            self.library(ui);
        });
        if self.app.ui.settings_open { self.settings(ctx); }
        if self.power_open { self.power_menu(ctx); }
        self.wave_browser(ctx);
        self.battery_dialog(ctx);
        if self.heat_open { self.heat_panel(ctx); } else { self.app.heat.set_panel(false); }
        if self.keys_open { self.keys_window(ctx); }
        self.dialogs(ctx);
        if let Some((path, at)) = &self.test_shot {
            if self.frames == *at { ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default())); }
            let shot = ctx.input(|i| i.raw.events.iter().find_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }));
            if let Some(img) = shot { save_png(path, &img); ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
        }
        // full frame rate while anything moves; otherwise 10 frames a second (MIDI and the workers wake us)
        let busy = self.app.decks.iter().any(|d| d.playing || d.loading || d.touch || d.coasting) || self.app.learning() || self.app.rec.is_some()
            || ctx.input(|i| i.pointer.any_down()) || self.uitest.is_some() || self.test_shot.is_some() || !self.app.progress.starts_with(|c: char| c.is_ascii_digit()) && !self.app.progress.is_empty();
        if busy { ctx.request_repaint(); } else { ctx.request_repaint_after(std::time::Duration::from_millis(100)); }
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) { self.app.flush_now(); }
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(t) = self.uitest.as_mut() {
            t.input(raw, &self.app, &self.rects);
            if t.done { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
        }
    }
}

fn save_png(path: &PathBuf, img: &egui::ColorImage) {
    let Ok(f) = std::fs::File::create(path) else { return };
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), img.size[0] as u32, img.size[1] as u32);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    if let Ok(mut w) = e.write_header() { let _ = w.write_image_data(img.as_raw()); }
}

/// beat lock: how far a synced deck's beats are from the master's, as on the web and iOS apps
/// ("+3 ms"): green within 6 ms, amber within 20, red beyond
fn phase_tag(d: &crate::deck::Deck) -> Option<(String, Color32)> {
    let pe = if d.sync { d.phase_err? } else { return None };
    let c = if pe.abs() < 0.006 { Color32::from_rgb(0x3d, 0xdc, 0x84) } else if pe.abs() < 0.02 { Color32::from_rgb(0xff, 0xb0, 0x20) } else { Color32::from_rgb(0xff, 0x3b, 0x55) };
    Some((format!("{}{} ms", if pe >= 0.0 { "+" } else { "−" }, (pe.abs() * 1000.0 + 0.5).floor() as i64), c))
}

/// a gain as a dB readout: "+1.6", "-12.0", "-∞"
fn db_text(g: f64) -> String { if g <= 1e-4 { "-∞".into() } else { format!("{:+.1}", 20.0 * g.log10()) } }

/// a network rate: "820 kbps", "12.4 Mbps"
fn rate_text(bytes_s: f64) -> String { let kb = bytes_s * 8.0 / 1000.0; if kb >= 1000.0 { format!("{:.1} Mbps", kb / 1000.0) } else { format!("{:.0} kbps", kb) } }

/// frame timing, summed every 5 s for the header readout; CRATEBOX_PROFILE=1 also prints the frame rate, missed refreshes and the UI thread's time per frame (stderr)
struct FrameProf(std::time::Instant);
/// the last 5 s of FrameProf, for the header (Settings → Look → Show frame rate)
static FRAME_STATS: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
impl Drop for FrameProf {
    fn drop(&mut self) {
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
        static SUM: AtomicU64 = AtomicU64::new(0);
        static N: AtomicU64 = AtomicU64::new(0);
        static MAX: AtomicU64 = AtomicU64::new(0);
        static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        static LAST_MS: AtomicU64 = AtomicU64::new(0);
        static PREV_US: AtomicU64 = AtomicU64::new(0);
        static LATE: AtomicU64 = AtomicU64::new(0);
        static GAP: AtomicU64 = AtomicU64::new(0);
        let us = self.0.elapsed().as_micros() as u64;
        SUM.fetch_add(us, Relaxed); N.fetch_add(1, Relaxed); MAX.fetch_max(us, Relaxed);
        let base = BASE.get_or_init(std::time::Instant::now);
        // frame to frame: at 60 Hz a frame every 16.7 ms; a gap over 25 ms is a missed refresh (a visible hitch)
        let start_us = self.0.duration_since(*base).as_micros() as u64;
        let prev = PREV_US.swap(start_us, Relaxed);
        if prev > 0 && start_us > prev { let gap = start_us - prev; GAP.fetch_max(gap, Relaxed); if gap > 25_000 { LATE.fetch_add(1, Relaxed); } }
        let now_ms = base.elapsed().as_millis() as u64;
        if now_ms - LAST_MS.load(Relaxed) >= 5000 {
            let secs = (now_ms - LAST_MS.swap(now_ms, Relaxed)) as f64 / 1000.0;
            let n = N.swap(0, Relaxed);
            let (fps, late, gap) = (n as f64 / secs, LATE.swap(0, Relaxed), GAP.swap(0, Relaxed) as f64 / 1000.0);
            let (avg, worst) = (SUM.swap(0, Relaxed) as f64 / n.max(1) as f64 / 1000.0, MAX.swap(0, Relaxed) as f64 / 1000.0);
            if let Ok(mut s) = FRAME_STATS.lock() { *s = format!("{fps:.0} FPS · {late} MISSED"); }
            static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if *ON.get_or_init(|| std::env::var_os("CRATEBOX_PROFILE").is_some()) {
                eprintln!("cratebox frame: {fps:.1} fps, {late} missed refreshes, longest gap {gap:.1} ms; UI work avg {avg:.2} ms, worst {worst:.2} ms");
            }
        }
    }
}
