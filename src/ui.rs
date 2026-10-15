//! The window: header, scrolling waveforms, the two decks with the mixer between them, and the
//! library (playlist tree + track list). Same layout and behaviour as the web version.

use crate::app::{level, App};
use crate::deck::{PadMode, JUMPS, LOOP_SIZES, RANGES, JogKind};
use crate::dialog::{FileDialog, Purpose};
use crate::keys::{camelot, keys_match};
use crate::library::{SortKey, Track};
use crate::midi;
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
    deck_rects: [Rect; 2],
    jog_last: [Option<f32>; 2],
    lane_drag: [Option<(f32, f32, std::time::Instant, bool, bool)>; 2],
    pub frames: u64,
    pub test_shot: Option<(PathBuf, u64)>,
    /// named on-screen controls (for the scripted UI tests)
    pub rects: std::collections::HashMap<String, Rect>,
    pub uitest: Option<crate::uitest::UiTest>,
}

const ROW: f32 = 24.0;

impl Gui {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Gui {
        style(&cc.egui_ctx);
        let mut app = App::new();
        let ctx = cc.egui_ctx.clone();
        let wake: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || ctx.request_repaint());
        app.repaint = Some(wake.clone());
        app.midi_io.set_wake(wake);
        Gui { app, deck_rects: [Rect::NOTHING; 2], jog_last: [None; 2], lane_drag: [None; 2], frames: 0, test_shot: None, rects: Default::default(), uitest: crate::uitest::UiTest::from_env() }
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

    // ===================================================================== header
    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(egui::RichText::new("CRATE").font(bold(21.0)).color(FG));
            ui.label(egui::RichText::new("BOX").font(bold(21.0)).color(deck_color(0)));
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add_space(12.0);
            // controller pill
            let on = !self.app.midi_io.name.is_empty();
            let name = if on { self.app.midi_io.name.split(':').next().unwrap_or("").to_string() } else { self.app.midi_io.error.clone().unwrap_or_else(|| "No controller".into()) };
            pill(ui, &name, on);
            let ch = self.app.audio.channels;
            let out = if self.app.audio.running() { if self.app.audio.device.is_empty() { "Default output".to_string() } else { short_dev(&self.app.audio.device) } } else { "No sound output".into() };
            pill(ui, &format!("{out} · {ch} ch"), self.app.audio.running() && ch >= 4).on_hover_text(&self.app.hp_note);
            if ui.button("Test cue").on_hover_text("Beep in the headphones only").clicked() {
                let _ = self.app.tx.send(crate::engine::Cmd::TestCue);
                self.app.say("Beeping in the headphone mix only. Hear it from the speakers too? Then cue isn't separated yet.", 8.0);
            }
            ui.add_space(8.0);
            let st = self.app.status_text().to_string();
            ui.add(egui::Label::new(egui::RichText::new(st).color(MUTED).size(12.5)).truncate());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let lv = [level(self.app.status_snap.master_meter[0]), level(self.app.status_snap.master_meter[1])];
                meter(ui, vec2(6.0, 26.0), lv[1]);
                meter(ui, vec2(6.0, 26.0), lv[0]);
                let mut m = self.app.mixer.master;
                let frozen = self.app.learning();
                let r = knob(ui, &mut m, "", FG, 0.8, 30.0, frozen);
                if !self.learn(ui, &r, "master", -1) && r.changed() { self.app.mixer.master = m; self.app.send_mixer(); }
                ui.label(egui::RichText::new("MASTER").size(10.0).color(MUTED));
            });
        });
    }

    // ===================================================================== waveforms
    fn lanes(&mut self, ui: &mut Ui, h: f32) {
        for i in 0..2 {
            let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click_and_drag());
            self.reg(format!("lane{i}"), rect);
            let zoom = self.app.store.settings.zoom;
            let rgb = self.app.store.settings.wave_style != "3band";
            paint_lane(ui.painter(), rect, &self.app.decks[i], i, zoom, rgb);
            // drag to move through the track (audio follows the hand while playing), click to jump
            let d = &mut self.app.decks[i];
            if !d.loaded() || self.app.midi.learning.is_some() { continue; }
            let pps = rect.width() as f64 / zoom / d.rate().max(0.05);
            if r.hovered() {
                let s = ui.input(|i| i.smooth_scroll_delta.y);
                if s != 0.0 { let z = &mut self.app.store.settings.zoom; *z = (*z * if s < 0.0 { 1.15 } else { 1.0 / 1.15 }).clamp(1.0, 32.0); }
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
                if let Some(p) = r.interact_pointer_pos() { let t = (d.now() + (p.x - rect.center().x) as f64 / pps).clamp(0.0, d.duration); d.seek_to(t); }
            }
            ui.add_space(2.0);
        }
    }

    // ===================================================================== a deck
    fn deck(&mut self, ui: &mut Ui, i: usize, compact: bool) {
        let col = deck_color(i);
        let frozen = self.app.learning();
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 6.0, Color32::from_rgb(0x13, 0x16, 0x1b));
        ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(rect.width(), 3.0)), 2.0, col);
        // drop a track on the deck
        let resp = ui.interact(rect, ui.id().with(("deckdrop", i)), Sense::hover());
        if let Some(p) = resp.dnd_release_payload::<DragRows>() { if let Some(id) = p.ids.first() { let id = id.clone(); self.app.load_id(i, &id); } }
        if resp.dnd_hover_payload::<DragRows>().is_some() { ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0_f32, col), StrokeKind::Inside); }
        self.deck_rects[i] = rect;
        self.reg(format!("deck{i}.area"), rect);
        let ui = &mut ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 8.0))).layout(Layout::top_down(Align::Min)));
        ui.add_space(2.0);
        // ---- head: number, title/artist, BPM + tempo
        {
            let d = &self.app.decks[i];
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{}", i + 1)).font(bold(30.0)).color(col));
                ui.vertical(|ui| {
                    let (title, artist) = match &d.track {
                        Some(t) => (format!("{}{}", if d.loading { "Loading… " } else { "" }, t.title), if t.artist.is_empty() { "Unknown artist".to_string() } else { t.artist.clone() }),
                        None => ("No track loaded".into(), "Drag a track here, double-click one, or press LOAD on your controller".into()),
                    };
                    ui.set_width((ui.available_width() - 130.0).max(80.0));
                    ui.add(egui::Label::new(egui::RichText::new(title).font(bold(15.0))).truncate());
                    ui.add(egui::Label::new(egui::RichText::new(artist).size(12.0).color(MUTED)).truncate());
                });
                ui.with_layout(Layout::top_down(Align::Max), |ui| {
                    ui.label(egui::RichText::new(if d.bpm() > 0.0 { format!("{:.2}", d.eff_bpm()) } else { "--.--".into() }).font(mono(24.0)).color(FG));
                    let r = d.range();
                    ui.label(egui::RichText::new(format!("{:+.2}%  {}", (d.rate() - 1.0) * 100.0, if r == 100.0 { "WIDE".into() } else { format!("±{r}%") })).font(mono(11.5)).color(MUTED));
                });
            });
        }
        // ---- info row
        ui.horizontal(|ui| {
            let d = &self.app.decks[i];
            let t = d.now();
            ui.label(egui::RichText::new(fmt_time(t)).font(mono(13.0)));
            ui.label(egui::RichText::new(format!("-{}", fmt_time(d.duration - t))).font(mono(13.0)).color(MUTED));
            let beat = if d.loaded() && d.ana.is_some() { d.beat_at(t).floor().rem_euclid(4.0) as i32 } else { -1 };
            for k in 0..4 {
                let (r, _) = ui.allocate_exact_size(vec2(9.0, 9.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, if k == beat { if k == 0 { RED } else { col } } else { LINE2 });
            }
            if let Some((s, _e, bars, label, bar)) = d.phrase_spans().into_iter().find(|p| t >= p.0 && t < p.1) {
                let n = (((t - s) / bar).floor() as u32 + 1).min(bars);
                ui.label(egui::RichText::new(format!("{} {}/{}", label.to_uppercase(), n, bars)).strong().size(12.5).color(rgb(crate::analysis::phrase_color(&label))));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (q, mt, grid_open) = (self.app.decks[i].quant, self.app.decks[i].mt, self.app.ui.grid_open[i]);
                let r = tag(ui, "Q", q, col).on_hover_text("Quantize: snap cues and loops to the beat grid");
                if !self.learn(ui, &r, "quant", i as i8) && r.clicked() { self.app.decks[i].quant = !q; }
                let r = tag(ui, "MT", mt, col).on_hover_text("Master tempo (keylock): change tempo without changing the key");
                if !self.learn(ui, &r, "keylock", i as i8) && r.clicked() { self.app.set_mt(i, !mt); }
                let r = tag(ui, "GRID", grid_open, col).on_hover_text("Edit the beat grid");
                self.reg(format!("deck{i}.grid"), r.rect);
                if r.clicked() { self.app.ui.grid_open[i] = !grid_open; }
                let o = &self.app.decks[1 - i];
                let is_master = self.app.decks[i].loaded() && o.sync && crate::deck::master_for(o, &self.app.decks[i]).is_some();
                tag(ui, "MASTER", is_master, CUE).on_hover_text("This deck is the tempo master");
                let k = self.app.decks[i].key_now();
                if let Some(c) = camelot(k) { tag(ui, &c.label(), true, rgb(c.color())).on_hover_text(crate::keys::key_name(k)); }
            });
        });
        // ---- overview: click to jump, drag to scrub
        let (orect, or) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click_and_drag());
        self.reg(format!("deck{i}.overview"), orect);
        paint_overview(ui.painter(), orect, &self.app.decks[i], self.app.store.settings.wave_style != "3band");
        if !frozen && (or.dragged() || or.clicked() || or.drag_started()) {
            if let Some(p) = or.interact_pointer_pos() {
                let d = &mut self.app.decks[i];
                if d.loaded() { let f = ((p.x - orect.left()) / orect.width()).clamp(0.0, 1.0) as f64; let dur = d.duration; d.seek_to(f * dur); }
            }
        }
        if self.app.ui.grid_open[i] { self.grid_bar(ui, i); }
        if compact { return; }
        ui.add_space(4.0);
        // ---- body: tempo | jog + transport | controls (mirrored for deck 2)
        let body = ui.available_rect_before_wrap();
        let tempo_w = 46.0;
        let jog_w = (body.height() - 60.0).clamp(110.0, 170.0);
        let (tempo_r, jog_r, ctrl_r) = if i == 0 {
            let t = Rect::from_min_size(body.min, vec2(tempo_w, body.height()));
            let j = Rect::from_min_size(pos2(t.right() + 8.0, body.top()), vec2(jog_w, body.height()));
            (t, j, Rect::from_min_max(pos2(j.right() + 10.0, body.top()), body.max))
        } else {
            let t = Rect::from_min_size(pos2(body.right() - tempo_w, body.top()), vec2(tempo_w, body.height()));
            let j = Rect::from_min_size(pos2(t.left() - 8.0 - jog_w, body.top()), vec2(jog_w, body.height()));
            (t, j, Rect::from_min_max(body.min, pos2(j.left() - 10.0, body.bottom())))
        };
        // tempo fader: top = slower, like a CDJ
        {
            let ui = &mut ui.new_child(UiBuilder::new().max_rect(tempo_r).layout(Layout::top_down(Align::Center)));
            ui.label(egui::RichText::new("TEMPO").size(10.0).color(MUTED));
            let mut v = (0.5 - self.app.decks[i].pitch / 2.0) as f32;
            let r = vslider(ui, &mut v, vec2(30.0, (tempo_r.height() - 50.0).max(60.0)), true, frozen);
            if !self.learn(ui, &r, "tempo", i as i8) && r.changed() { self.app.decks[i].set_pitch((0.5 - v as f64) * 2.0); }
            let rl = RANGES[self.app.decks[i].range_idx];
            if button(ui, &if rl == 100.0 { "WIDE".to_string() } else { format!("±{rl}") }, vec2(42.0, 20.0), false, col, 11.0).clicked() { self.app.decks[i].cycle_range(); }
        }
        // jog + CUE / PLAY
        {
            let ui = &mut ui.new_child(UiBuilder::new().max_rect(jog_r).layout(Layout::top_down(Align::Center)));
            let (jr, resp) = ui.allocate_exact_size(vec2(jog_w, jog_w), Sense::click_and_drag());
            paint_jog(ui.painter(), jr, &self.app.decks[i], i);
            if !self.learn(ui, &resp, "jog", i as i8) {
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
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space((jog_w - 2.0 * 54.0 - 12.0).max(0.0) / 2.0);
                let d = &self.app.decks[i];
                let cue_lit = d.loaded() && (d.previewing || (d.now() - d.cue).abs() < 0.03);
                let r = round_button(ui, "CUE", 54.0, cue_lit, CUE);
                if !self.learn(ui, &r, "cue", i as i8) {
                    if r.is_pointer_button_down_on() && ui.input(|x| x.pointer.any_pressed()) { self.app.decks[i].cue_down(); }
                    if r.drag_stopped() || r.clicked() { self.app.decks[i].cue_up(); }
                }
                ui.add_space(12.0);
                let playing = self.app.decks[i].playing;
                let r = round_button(ui, "▶ ❚❚", 54.0, playing, GREEN);
                if !self.learn(ui, &r, "play", i as i8) && r.clicked() { self.app.toggle_play(i); }
            });
        }
        // controls: sync/loops, jumps, pad modes, pads
        let ui = &mut ui.new_child(UiBuilder::new().max_rect(ctrl_r).layout(Layout::top_down(Align::Min)));
        let w = ctrl_r.width();
        let bw = ((w - 6.0 * 5.0) / 7.0).max(30.0);
        ui.horizontal(|ui| {
            let d = &self.app.decks[i];
            let (sync, lp_on, a_set, b_set) = (d.sync, d.lp.on, d.lp.a.is_some(), d.lp.b.is_some());
            let r = button(ui, "SYNC", vec2(bw * 1.3, 26.0), sync, col, 12.0);
            if !self.learn(ui, &r, "sync", i as i8) && r.clicked() { self.app.set_sync(i, !sync); }
            let r = button(ui, "IN", vec2(bw, 26.0), lp_on || (a_set && !b_set), LOOP, 12.0);
            if !self.learn(ui, &r, "loopIn", i as i8) && r.clicked() { self.app.decks[i].loop_in(); }
            let r = button(ui, "OUT", vec2(bw, 26.0), lp_on, LOOP, 12.0);
            if !self.learn(ui, &r, "loopOut", i as i8) && r.clicked() { self.app.decks[i].loop_out(); }
            let r = button(ui, "RELOOP", vec2(bw * 1.4, 26.0), b_set, LOOP, 11.0);
            if !self.learn(ui, &r, "reloop", i as i8) && r.clicked() { self.app.decks[i].reloop_exit(); }
            if button(ui, "½", vec2(bw * 0.7, 26.0), false, LOOP, 12.0).on_hover_text("Halve loop").clicked() { self.app.decks[i].loop_scale(0.5); }
            if button(ui, "×2", vec2(bw * 0.7, 26.0), false, LOOP, 12.0).on_hover_text("Double loop").clicked() { self.app.decks[i].loop_scale(2.0); }
        });
        ui.horizontal(|ui| {
            for (lbl, b) in [("◀◀ 4", -4.0), ("◀ 1", -1.0), ("1 ▶", 1.0), ("4 ▶▶", 4.0)] {
                if button(ui, lbl, vec2(bw * 1.15, 24.0), false, col, 11.5).clicked() { self.app.decks[i].beat_jump(b); }
            }
            let d = &self.app.decks[i];
            let lb = d.loop_beats();
            let info = match (d.lp.a, d.lp.b) { (None, _) => "no loop".to_string(), (Some(_), None) => "loop in…".into(),
                _ => format!("{} beat{}", if lb < 1.0 { format!("1/{}", (1.0 / lb).round()) } else { format!("{}", (lb * 100.0).round() / 100.0) }, if lb == 1.0 { "" } else { "s" }) };
            ui.label(egui::RichText::new(info).size(11.5).color(if d.lp.on { LOOP } else { MUTED }));
        });
        ui.horizontal(|ui| {
            for (m, label, act) in [(PadMode::HotCue, "HOT CUE", "mode:hotcue"), (PadMode::BeatLoop, "BEAT LOOP", "mode:beatloop"), (PadMode::BeatJump, "BEAT JUMP", "mode:beatjump")] {
                let r = button(ui, label, vec2((w - 12.0) / 3.0, 22.0), self.app.decks[i].pad_mode == m, col, 11.0);
                if !self.learn(ui, &r, act, i as i8) && r.clicked() { self.app.decks[i].pad_mode = m; }
            }
        });
        let pad_h = ((ui.available_height() - 8.0) / 2.0).clamp(26.0, 48.0);
        for row in 0..2 {
            ui.horizontal(|ui| {
                for c in 0..4 {
                    let p = row * 4 + c;
                    let d = &self.app.decks[i];
                    let (label, lit, accent) = match d.pad_mode {
                        PadMode::HotCue => match d.hot[p] { Some(t) => (format!("{}  {}", &"ABCDEFGH"[p..p + 1], fmt_time(t).trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('.')), true, GREEN), None => ("ABCDEFGH"[p..p + 1].to_string(), false, GREEN) },
                        PadMode::BeatLoop => { let s = LOOP_SIZES[p]; (if s < 1.0 { format!("1/{}", 1.0 / s) } else { format!("{s}") }, d.lp.on && (d.loop_beats() - s).abs() < 0.01, LOOP) }
                        PadMode::BeatJump => { let j = JUMPS[p]; (format!("{}{}{}", if j < 0.0 { "◀ " } else { "" }, j.abs(), if j > 0.0 { " ▶" } else { "" }), false, col) }
                    };
                    let pw = (w - 18.0) / 4.0;
                    let r = pad(ui, &label, vec2(pw, pad_h), lit, accent);
                    self.reg(format!("deck{i}.pad{p}"), r.rect);
                    let act = format!("hot:{p}");
                    if !self.learn(ui, &r, &act, i as i8) {
                        if r.clicked() { let shift = ui.input(|x| x.modifiers.shift); self.app.decks[i].pad_press(p, shift); }
                        if r.secondary_clicked() { self.app.decks[i].pad_press(p, true); }
                    }
                }
            });
        }
    }

    fn grid_bar(&mut self, ui: &mut Ui, i: usize) {
        ui.add_space(3.0);
        egui::Frame::new().fill(Color32::from_rgb(0x0c, 0x0e, 0x12)).stroke(Stroke::new(1.0_f32, LINE2)).corner_radius(4.0).inner_margin(5.0).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("GRID").size(10.0).color(MUTED));
                let id = ui.id().with(("bpm", i));
                let te = egui::TextEdit::singleline(&mut self.app.ui.bpm_text[i]).id(id).desired_width(62.0).font(mono(13.0)).hint_text(format!("{:.3}", self.app.decks[i].bpm()));
                let r = ui.add(te);
                if r.gained_focus() { self.app.ui.bpm_text[i] = format!("{}", (self.app.decks[i].bpm() * 1000.0).round() / 1000.0); }
                if r.lost_focus() {
                    if ui.input(|x| x.key_pressed(Key::Enter)) {
                        if let Ok(v) = self.app.ui.bpm_text[i].replace(',', ".").trim().parse::<f64>() { if v > 20.0 && v < 400.0 { self.app.decks[i].set_bpm(v, "BPM typed in"); } }
                    }
                    self.app.ui.bpm_text[i].clear();
                }
                let shift = ui.input(|x| x.modifiers.shift);
                let b = self.app.decks[i].bpm();
                let d = &mut self.app.decks[i];
                if ui.small_button("−.01").on_hover_text("BPM −0.01 (Shift: −0.1)").clicked() { d.set_bpm(b - if shift { 0.1 } else { 0.01 }, ""); }
                if ui.small_button("+.01").on_hover_text("BPM +0.01 (Shift: +0.1)").clicked() { d.set_bpm(b + if shift { 0.1 } else { 0.01 }, ""); }
                if ui.small_button("÷2").clicked() { d.set_bpm(b / 2.0, "halved"); }
                if ui.small_button("×2").clicked() { d.set_bpm(b * 2.0, "doubled"); }
                if ui.small_button("TAP").on_hover_text("Tap along with the beat (4+ taps)").clicked() { d.tap(); }
                ui.add_space(6.0);
                if ui.small_button("◀ 5ms").on_hover_text("Slide the grid 5 ms earlier (Shift: 1 ms)").clicked() { d.nudge_grid(if shift { -0.001 } else { -0.005 }); }
                if ui.small_button("5ms ▶").on_hover_text("Slide the grid 5 ms later (Shift: 1 ms)").clicked() { d.nudge_grid(if shift { 0.001 } else { 0.005 }); }
                if ui.small_button("BEAT HERE").on_hover_text("Put the nearest beat line on the playhead (pause on a kick first)").clicked() { d.beat_here(); }
                if ui.small_button("BAR 1 HERE").on_hover_text("Make the playhead the first beat of a bar").clicked() { d.downbeat_here(); }
                if ui.small_button("RESET").on_hover_text("Back to the analysed grid").clicked() { d.reset_grid(); }
            });
        });
    }

    // ===================================================================== mixer
    fn mixer(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 6.0, PANEL);
        let frozen = self.app.learning();
        let ui = &mut ui.new_child(UiBuilder::new().max_rect(rect.shrink(8.0)).layout(Layout::top_down(Align::Center)));
        ui.horizontal(|ui| {
            let half = (ui.available_width() - 60.0) / 2.0;
            for i in [0usize, 1] {
                if i == 1 {
                    ui.vertical(|ui| {
                        ui.set_width(52.0);
                        let mut v = self.app.mixer.head_mix;
                        let r = knob(ui, &mut v, "CUE/MST", FG, 0.5, 30.0, frozen);
                        if !self.learn(ui, &r, "headMix", -1) && r.changed() { self.app.mixer.head_mix = v; self.app.send_mixer(); }
                    });
                }
                ui.vertical(|ui| {
                    ui.set_width(half);
                    let col = deck_color(i);
                    let names: [(&str, &str); 5] = [("TRIM", "trim"), ("HI", "hi"), ("MID", "mid"), ("LOW", "low"), ("CFX", "cfx")];
                    for (label, key) in names {
                        let m = &mut self.app.mixer;
                        let mut v = match key { "trim" => m.trim[i], "hi" => m.hi[i], "mid" => m.mid[i], "low" => m.low[i], _ => m.cfx[i] };
                        let r = knob(ui, &mut v, label, if key == "cfx" { LOOP } else { col }, 0.5, 24.0, frozen);
                        if !self.learn(ui, &r, key, i as i8) && r.changed() {
                            let m = &mut self.app.mixer;
                            match key { "trim" => m.trim[i] = v, "hi" => m.hi[i] = v, "mid" => m.mid[i] = v, "low" => m.low[i] = v, _ => m.cfx[i] = v }
                            self.app.send_mixer();
                        }
                    }
                    let lit = self.app.mixer.pfl[i];
                    let r = button(ui, "CUE", vec2(46.0, 22.0), lit, CUE, 11.5);
                    if !self.learn(ui, &r, "pfl", i as i8) && r.clicked() { self.app.mixer.pfl[i] = !lit; self.app.send_mixer(); }
                });
            }
        });
        ui.add_space(4.0);
        let fh = (ui.available_height() - 36.0).clamp(60.0, 140.0);
        ui.horizontal(|ui| {
            let w = ui.available_width();
            ui.add_space((w - 2.0 * 34.0 - 2.0 * 10.0 - 30.0) / 2.0);
            for i in [0usize, 1] {
                if i == 1 { let l = [level(self.app.status_snap.meters[0]), level(self.app.status_snap.meters[1])]; meter(ui, vec2(8.0, fh), l[0]); meter(ui, vec2(8.0, fh), l[1]); }
                let mut v = self.app.mixer.fader[i];
                let r = vslider(ui, &mut v, vec2(34.0, fh), false, frozen);
                if !self.learn(ui, &r, "fader", i as i8) && r.changed() { self.app.mixer.fader[i] = v; self.app.send_mixer(); }
            }
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("1").size(10.0).color(MUTED));
            let mut x = self.app.mixer.xfader;
            let r = hslider(ui, &mut x, vec2(ui.available_width() - 16.0, 22.0), frozen);
            if !self.learn(ui, &r, "xfader", -1) && r.changed() { self.app.mixer.xfader = x; self.app.send_mixer(); }
            ui.label(egui::RichText::new("2").size(10.0).color(MUTED));
        });
    }

    // ===================================================================== library
    fn library(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, PANEL);
        ui.painter().hline(rect.x_range(), rect.top(), Stroke::new(1.0_f32, LINE));
        let tool_h = 36.0;
        let tool = Rect::from_min_size(rect.min, vec2(rect.width(), tool_h));
        let tree_w = 220.0;
        let tree = Rect::from_min_max(pos2(rect.left(), tool.bottom()), pos2(rect.left() + tree_w, rect.bottom()));
        let list = Rect::from_min_max(pos2(tree.right(), tool.bottom()), rect.max);
        {
            let ui = &mut ui.new_child(UiBuilder::new().max_rect(tool.shrink2(vec2(10.0, 4.0))).layout(Layout::left_to_right(Align::Center)));
            self.toolbar(ui);
        }
        ui.painter().hline(rect.x_range(), tool.bottom(), Stroke::new(1.0_f32, LINE));
        {
            let ui = &mut ui.new_child(UiBuilder::new().max_rect(tree).layout(Layout::top_down(Align::Min)));
            self.tree(ui);
        }
        ui.painter().vline(tree.right(), tree.y_range(), Stroke::new(1.0_f32, LINE));
        {
            let ui = &mut ui.new_child(UiBuilder::new().max_rect(list.shrink2(vec2(1.0, 0.0))).layout(Layout::top_down(Align::Min)));
            self.track_list(ui);
        }
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
        if matches!(self.app.view, View::Pl(_)) && ui.button("Remove from list").on_hover_text("Remove the selected tracks from this playlist (Delete key)").clicked() { self.app.remove_from_playlist(); }
        let r = ui.add(egui::TextEdit::singleline(&mut self.app.list.query).hint_text("Search title, artist or key (8A)").desired_width(240.0));
        self.reg("search", r.rect);
        if r.changed() { self.app.apply_view(false); }
        let browse = self.app.store.settings.browse;
        if ui.selectable_label(browse, if browse { "Show decks" } else { "Expand library" }).on_hover_text("SHIFT + press BROWSE on the FLX4").clicked() {
            self.app.store.settings.browse = !browse; self.app.store.dirty_settings = true;
        }
        let r = ui.button("Settings");
        self.reg("settings", r.rect);
        if r.clicked() { self.app.ui.settings_open = !self.app.ui.settings_open; }
        ui.label(egui::RichText::new(&self.app.progress).size(12.0).color(MUTED));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(egui::RichText::new("Browse knob scrolls · press it for playlists · drag rows onto a deck or a playlist").size(10.5).color(DIM));
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
                        let resp = ui.label(egui::RichText::new(&r.label).size(10.0).color(MUTED));
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
                if is_view { p.rect_filled(rect, 0.0, Color32::from_rgb(0x1f, 0x2f, 0x44)); }
                else if resp.hovered() { p.rect_filled(rect, 0.0, Color32::from_rgb(0x1a, 0x1e, 0x25)); }
                if focus && k == self.app.tree_sel { p.rect_filled(rect, 0.0, Color32::from_rgb(0x22, 0x34, 0x4a)); p.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 0.0, deck_color(0)); }
                if scroll && k == self.app.tree_sel { ui.scroll_to_rect(rect, None); }
                let x0 = rect.left() + 6.0 + r.depth as f32 * 12.0;
                let folderish = matches!(r.kind, RowKind::Folder | RowKind::DevFolder | RowKind::Dev);
                let open = folderish && self.app.lists.open.contains(&Lists::open_key(r));
                let caret_rect = Rect::from_min_size(pos2(x0, rect.top()), vec2(14.0, rect.height()));
                if folderish { p.text(caret_rect.center(), egui::Align2::CENTER_CENTER, if open { "▾" } else { "▸" }, prop(10.0), MUTED); }
                let icon = match r.kind { RowKind::All => "♫", RowKind::Pl | RowKind::DevPl => "≡", RowKind::Dev => "⏏", _ => "" };
                p.text(pos2(x0 + 20.0, rect.center().y), egui::Align2::CENTER_CENTER, icon, prop(12.0), MUTED);
                let ro = matches!(r.kind, RowKind::Dev | RowKind::DevPl | RowKind::DevFolder);
                p.text(pos2(x0 + 30.0, rect.center().y), egui::Align2::LEFT_CENTER, &r.label, prop(13.0), if ro { Color32::from_rgb(0xc9, 0xd3, 0xe0) } else { FG });
                if let Some(c) = r.count { p.text(pos2(rect.right() - 30.0, rect.center().y), egui::Align2::RIGHT_CENTER, c.to_string(), mono(11.0), MUTED); }
                // row actions on hover: × delete / ⏏ close / ⧉ copy
                let xr = Rect::from_min_size(pos2(rect.right() - 24.0, rect.top()), vec2(20.0, rect.height()));
                let hovering_x = resp.hover_pos().is_some_and(|hp| xr.contains(hp));
                if resp.hovered() || is_view {
                    let sym = match r.kind { RowKind::Pl | RowKind::Folder | RowKind::Dev => "×", RowKind::DevPl => "+", _ => "" };
                    if !sym.is_empty() { p.text(xr.center(), egui::Align2::CENTER_CENTER, sym, prop(14.0), if hovering_x { FG } else { MUTED }); }
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
        // columns: #, Title, Artist, BPM, Key, Time, Deck
        let fixed = 44.0 + 72.0 + 60.0 + 64.0 + 52.0;
        let flex = (w - fixed).max(200.0);
        let cols = [44.0, flex * 0.6, flex * 0.4, 72.0, 60.0, 64.0, 52.0];
        let xs: Vec<f32> = cols.iter().scan(0.0, |acc, c| { let x = *acc; *acc += c; Some(x) }).collect();
        // header
        let (hr, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
        ui.painter().rect_filled(hr, 0.0, RAISE);
        let heads: [(&str, Option<SortKey>); 7] = [("#", None), ("TITLE", Some(SortKey::Title)), ("ARTIST", Some(SortKey::Artist)), ("BPM", Some(SortKey::Bpm)), ("KEY", Some(SortKey::Key)), ("TIME", Some(SortKey::Time)), ("DECK", None)];
        for (k, (label, key)) in heads.iter().enumerate() {
            let r = Rect::from_min_size(pos2(hr.left() + xs[k], hr.top()), vec2(cols[k], hr.height()));
            let resp = ui.interact(r, ui.id().with(("hdr", k)), Sense::click());
            let sorted = key.is_some() && self.app.list.sort == *key;
            let txt = format!("{label}{}", if sorted { if self.app.list.dir > 0 { " ▴" } else { " ▾" } } else { "" });
            ui.painter().text(pos2(r.left() + 8.0, r.center().y), egui::Align2::LEFT_CENTER, txt, prop(10.5), if sorted { FG } else { MUTED });
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
                } else { ui.label(egui::RichText::new("Nothing here").color(MUTED)); }
            });
            return;
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
                if sel { p.rect_filled(rect, 0.0, Color32::from_rgb(0x24, 0x36, 0x4d)); }
                else if msel { p.rect_filled(rect, 0.0, Color32::from_rgb(0x1d, 0x2c, 0x3e)); }
                else if resp.hovered() { p.rect_filled(rect, 0.0, Color32::from_rgb(0x1a, 0x1e, 0x25)); }
                p.hline(rect.x_range(), rect.bottom(), Stroke::new(1.0_f32, Color32::from_rgb(0x19, 0x1c, 0x22)));
                let on_deck = self.app.decks.iter().position(|d| d.track.as_ref().is_some_and(|x| x.id == t.id));
                let dim = t.missing;
                let tc = if dim { DIM } else if let Some(d) = on_deck { deck_color(d) } else { FG };
                let cell = |c: usize, txt: String, color: Color32, monof: bool| {
                    let r = Rect::from_min_size(pos2(rect.left() + xs[c] + 8.0, rect.top()), vec2(cols[c] - 12.0, ROW));
                    let font = if monof { mono(12.0) } else { prop(13.0) };
                    let g = p.layout_no_wrap(txt, font, color);
                    let pp = p.with_clip_rect(r);
                    pp.galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, color);
                };
                cell(0, format!("{}", self.app.list.view_pos.get(k).copied().unwrap_or(k) + 1), MUTED, true);
                cell(1, t.title.clone(), tc, false);
                cell(2, t.artist.clone(), if dim { DIM } else { FG }, false);
                let bpm = match t.bpm { Some(b) if b > 0.0 => if b.fract().abs() > 1e-9 { format!("{b:.2}") } else { format!("{b:.0}") }, Some(_) => "?".into(), None if t.failed => "?".into(), None => "…".into() };
                cell(3, bpm, if t.grid_manual { LOOP } else { MUTED }, true);
                match t.key.and_then(camelot) {
                    Some(c) => {
                        let m = keys_match(Some(c), ref_key);
                        if m { p.rect_filled(Rect::from_min_size(pos2(rect.left() + xs[4] + 4.0, rect.top() + 3.0), vec2(36.0, ROW - 6.0)), 3.0, Color32::from_rgba_unmultiplied(80, 220, 140, 40)); }
                        cell(4, c.label(), rgb(c.color()), true);
                    }
                    None => cell(4, if t.key == Some(-1) || t.failed { "?".into() } else if t.bpm.is_none() { "…".into() } else { String::new() }, MUTED, true),
                }
                cell(5, t.duration.map(|d| { let s = fmt_time(d); s[..s.len() - 2].to_string() }).unwrap_or_default(), MUTED, true);
                cell(6, on_deck.map(|d| (d + 1).to_string()).unwrap_or_default(), MUTED, true);
                if resp.clicked() {
                    let m = ui.input(|x| x.modifiers);
                    self.app.list.click(k, m.command || m.ctrl, m.shift);
                    self.app.focus_tree = false;
                }
                if resp.double_clicked() { let deck = if self.app.decks[0].playing && !self.app.decks[1].playing { 1 } else { 0 }; self.app.list.sel = k; self.app.load_selected(deck); }
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
                    if ui.button("Load on deck 1").clicked() { self.app.list.sel = k; self.app.load_selected(0); ui.close(); }
                    if ui.button("Load on deck 2").clicked() { self.app.list.sel = k; self.app.load_selected(1); ui.close(); }
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
    /// the key to mix into: the deck that started playing most recently
    fn ref_key(&self) -> Option<crate::keys::Camelot> {
        let mut p: Vec<&crate::deck::Deck> = self.app.decks.iter().filter(|d| d.playing && d.loaded()).collect();
        p.sort_by_key(|d| std::cmp::Reverse(d.started_at));
        p.first().and_then(|d| camelot(d.key_now()))
    }

    // ===================================================================== settings + learn bar
    fn settings(&mut self, ctx: &egui::Context) {
        let mut open = self.app.ui.settings_open;
        egui::Window::new("Settings").open(&mut open).resizable(true).default_width(480.0).collapsible(false).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(620.0).show(ui, |ui| {
                ui.heading("Library");
                let s = &self.app.store.settings;
                let mut br = s.bpm_range;
                egui::ComboBox::from_label("BPM range (where half/double-time tracks land)").selected_text(crate::analysis::BPM_RANGES[br].0).show_ui(ui, |ui| {
                    for (k, r) in crate::analysis::BPM_RANGES.iter().enumerate() { ui.selectable_value(&mut br, k, r.0); }
                });
                if br != s.bpm_range { self.app.set_bpm_range(br); }
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
                ui.label(egui::RichText::new(&self.app.hp_note).size(11.5).color(MUTED));
                let mut buf = self.app.store.settings.buffer;
                egui::ComboBox::from_label("Audio buffer (lower = tighter jog, higher = no dropouts)").selected_text(format!("{buf} samples ({:.1} ms)", buf as f64 / self.app.audio.sr * 1000.0)).show_ui(ui, |ui| {
                    for b in [128u32, 256, 512, 1024, 2048] { ui.selectable_value(&mut buf, b, format!("{b} samples ({:.1} ms)", b as f64 / self.app.audio.sr * 1000.0)); }
                });
                if buf != self.app.store.settings.buffer { self.app.store.settings.buffer = buf; self.app.store.dirty_settings = true; let d = self.app.audio.device.clone(); self.app.set_output(&d); }
                ui.label(egui::RichText::new(format!("{} Hz, {} channels", self.app.audio.sr, self.app.audio.channels)).size(11.0).color(DIM));

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
                ui.label(egui::RichText::new(format!("{}{}", midi::src_note(info.src), if n > 0 { format!(" You've mapped {n} control{} yourself.", if n == 1 { "" } else { "s" }) } else { String::new() })).size(11.5).color(MUTED));
                ui.horizontal(|ui| {
                    if ui.button("Learn…").clicked() { self.app.start_learn(); }
                    if ui.button("Clear learned").clicked() { self.app.clear_learned(); }
                    if ui.button("Export").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::ExportMapping, None)); }
                    if ui.button("Import").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::ImportMapping, None)); }
                });

                ui.separator();
                ui.heading("rekordbox");
                ui.horizontal(|ui| {
                    if ui.button("Open USB / SD…").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::OpenUsb, None)); }
                    if ui.button("Import XML…").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::ImportXml, None)); }
                    if ui.button("Export XML…").clicked() { self.app.ui.dialog = Some(FileDialog::new(Purpose::ExportXml, None)); }
                });
                ui.horizontal(|ui| {
                    ui.label("Export paths as if the music folders were in:");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.app.store.settings.base_path).hint_text("(leave empty: their real location)").desired_width(220.0));
                    if r.changed() { self.app.store.dirty_settings = true; }
                }).response.on_hover_text("For moving the XML to another computer, e.g. C:\\Users\\you\\Music");
                ui.separator();
                ui.label(egui::RichText::new(format!("Library data: {}", crate::store::data_dir().display())).size(11.0).color(DIM));
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
                ui.label(egui::RichText::new("Library:").color(MUTED));
            });
        });
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() { return; }
        let (down, up, left, right, del, all, esc, shift) = ctx.input(|i| (i.key_pressed(Key::ArrowDown), i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowLeft), i.key_pressed(Key::ArrowRight),
            i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace), i.modifiers.command && i.key_pressed(Key::A), i.key_pressed(Key::Escape), i.modifiers.shift));
        if down { self.app.list.move_sel(1); }
        if up { self.app.list.move_sel(-1); }
        if left && shift { self.app.load_selected(0); }
        if right && shift { self.app.load_selected(1); }
        if del && matches!(self.app.view, View::Pl(_)) { self.app.remove_from_playlist(); }
        if all { self.app.list.sel_ids = self.app.list.view.iter().map(|t| t.id.clone()).collect(); }
        if esc { if self.app.learning() { self.app.stop_learn(); } self.app.ui.settings_open = false; }
    }

    fn dropped_files(&mut self, ctx: &egui::Context) {
        let files: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if files.is_empty() { return; }
        let pos = ctx.input(|i| i.pointer.hover_pos());
        let target = pos.and_then(|p| (0..2).find(|&k| self.deck_rects[k].contains(p)));
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
    /// CRATEBOX_DEMO=1: load two tracks, play one with a loop and hot cues (for screenshots)
    fn demo(&mut self) {
        let a = &mut self.app;
        let ready = a.tracks.len() >= 2 && a.tracks.iter().all(|t| t.bpm.is_some());
        if !ready { return; }
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
            a.ui.grid_open[1] = true;
        }
    }
}

fn pill(ui: &mut Ui, text: &str, on: bool) -> Response {
    let g = ui.painter().layout_no_wrap(text.to_string(), prop(12.0), FG);
    let (rect, r) = ui.allocate_exact_size(vec2(g.size().x + 30.0, 24.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 12.0, RAISE);
    p.rect_stroke(rect, 12.0, Stroke::new(1.0_f32, LINE2), StrokeKind::Inside);
    p.circle_filled(pos2(rect.left() + 13.0, rect.center().y), 4.0, if on { GREEN } else { DIM });
    p.galley(pos2(rect.left() + 22.0, rect.center().y - g.size().y / 2.0), g, FG);
    r
}
fn short_dev(n: &str) -> String {
    // "hw:CARD=DDJFLX4,DEV=0" -> "DDJFLX4 (hw)"
    if let Some(c) = n.split("CARD=").nth(1) { let card = c.split(',').next().unwrap_or(c); let kind = n.split(':').next().unwrap_or(""); return format!("{card} ({kind})"); }
    n.to_string()
}
fn pad(ui: &mut Ui, text: &str, size: egui::Vec2, lit: bool, accent: Color32) -> Response {
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 5.0, if lit { Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 46) } else if r.hovered() { Color32::from_rgb(0x23, 0x28, 0x31) } else { RAISE });
    p.rect_stroke(rect, 5.0, Stroke::new(1.0_f32, if lit { accent } else { LINE2 }), StrokeKind::Inside);
    p.text(rect.center(), egui::Align2::CENTER_CENTER, text, prop(12.5), if lit { accent } else { FG });
    r
}

impl eframe::App for Gui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frames += 1;
        self.app.poll();
        if std::env::var("CRATEBOX_DEMO").is_ok() { self.demo(); }
        self.keyboard(ctx);
        self.dropped_files(ctx);
        egui::TopBottomPanel::top("header").exact_height(44.0).frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(12, 4))).show(ctx, |ui| self.header(ui));
        if self.app.learning() {
            egui::TopBottomPanel::bottom("learn").exact_height(40.0).frame(egui::Frame::new().fill(Color32::from_rgb(0x2a, 0x26, 0x10)).inner_margin(egui::Margin::symmetric(12, 4))).show(ctx, |ui| self.learn_bar(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(6.0)).show(ctx, |ui| {
            let full = ui.available_rect_before_wrap();
            let compact = self.app.store.settings.browse;
            let lane_h = 76.0;
            let deck_h = if compact { 118.0 } else { (full.height() * 0.42).clamp(300.0, 380.0) };
            {
                let r = Rect::from_min_size(full.min, vec2(full.width(), lane_h * 2.0 + 4.0));
                let ui = &mut ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::top_down(Align::Min)));
                self.lanes(ui, lane_h);
            }
            let dtop = full.top() + lane_h * 2.0 + 10.0;
            let mixer_w = if compact { 0.0 } else { 250.0 };
            let dw = (full.width() - mixer_w - 12.0) / 2.0;
            let d0 = Rect::from_min_size(pos2(full.left(), dtop), vec2(dw, deck_h));
            let mx = Rect::from_min_size(pos2(d0.right() + 6.0, dtop), vec2(mixer_w, deck_h));
            let d1 = Rect::from_min_size(pos2(full.right() - dw, dtop), vec2(dw, deck_h));
            for (i, r) in [(0usize, d0), (1, d1)] {
                let ui = &mut ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::top_down(Align::Min)));
                self.deck(ui, i, compact);
            }
            if !compact {
                let ui = &mut ui.new_child(UiBuilder::new().max_rect(mx).layout(Layout::top_down(Align::Min)));
                self.mixer(ui);
            }
            let lib = Rect::from_min_max(pos2(full.left(), dtop + deck_h + 6.0), full.max);
            let ui = &mut ui.new_child(UiBuilder::new().max_rect(lib).layout(Layout::top_down(Align::Min)));
            self.library(ui);
        });
        if self.app.ui.settings_open { self.settings(ctx); }
        self.dialogs(ctx);
        if let Some((path, at)) = &self.test_shot {
            if self.frames == *at { ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default())); }
            let shot = ctx.input(|i| i.raw.events.iter().find_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }));
            if let Some(img) = shot { save_png(path, &img); ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
        }
        // full frame rate while anything moves; otherwise 10 frames a second (MIDI and the workers wake us)
        let busy = self.app.decks.iter().any(|d| d.playing || d.loading || d.touch || d.coasting) || self.app.learning()
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
