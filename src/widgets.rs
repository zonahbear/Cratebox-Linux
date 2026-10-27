//! Custom-drawn controls (knobs, faders, jog wheels, buttons, meters) and the waveform painters,
//! in the web version's look.

use crate::analysis::{phrase_color, Wave};
use crate::deck::{Deck, Overview};
use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::f32::consts::PI;

pub const CUE: Color32 = Color32::from_rgb(0xff, 0xb0, 0x20);
pub const LOOP: Color32 = Color32::from_rgb(0xf5, 0xd6, 0x4a);
pub const GREEN: Color32 = Color32::from_rgb(0x3d, 0xdc, 0x84);
pub const RED: Color32 = Color32::from_rgb(0xff, 0x3b, 0x55);

/// a colour theme: the panels' greys and the four deck colours (CUE, LOOP, GREEN and RED keep their
/// meaning in every theme). Settings → Look; `set_theme` then `style` again.
pub struct Theme {
    pub name: &'static str,
    bg: u32, panel: u32, panel_d: u32, raise: u32, line: u32, line2: u32, fg: u32, muted: u32, dim: u32, wave_bg: u32,
    /// selected row, cursor row, hovered row
    sel: u32, sel2: u32, hover: u32,
    decks: [u32; 4],
}
const fn hex(c: u32) -> Color32 { Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8) }
pub const THEMES: [Theme; 5] = [
    // the web app's look
    Theme { name: "Night", bg: 0x0b0d10, panel: 0x13161b, panel_d: 0x101216, raise: 0x1b1f26, line: 0x272c35, line2: 0x343a45, fg: 0xe6e9ee, muted: 0x8a93a3, dim: 0x586070, wave_bg: 0x050608,
        sel: 0x24364d, sel2: 0x1d2c3e, hover: 0x1a1e25, decks: [0x39a7ff, 0xff8a3d, 0x2fd3c0, 0xe15bd2] },
    Theme { name: "Midnight", bg: 0x080c16, panel: 0x0f1524, panel_d: 0x0c111d, raise: 0x16203a, line: 0x223050, line2: 0x2e3f63, fg: 0xe4ebf8, muted: 0x8796b5, dim: 0x56648a, wave_bg: 0x04060c,
        sel: 0x223a6a, sel2: 0x1b2d52, hover: 0x152036, decks: [0x4fc3ff, 0xff5fa2, 0x45e0b0, 0xb28cff] },
    Theme { name: "Ember", bg: 0x100c0a, panel: 0x18120f, panel_d: 0x140f0c, raise: 0x221a15, line: 0x33271f, line2: 0x45362b, fg: 0xf2e9e2, muted: 0xa8968a, dim: 0x6e5f55, wave_bg: 0x070504,
        sel: 0x4a2f1f, sel2: 0x3a2619, hover: 0x251c16, decks: [0xf7d08a, 0xe2603b, 0xb7d36b, 0xd48aa8] },
    Theme { name: "Neon", bg: 0x050507, panel: 0x0c0c11, panel_d: 0x09090d, raise: 0x14141c, line: 0x22222e, line2: 0x30303f, fg: 0xf0f0ff, muted: 0x9090b0, dim: 0x5a5a78, wave_bg: 0x000000,
        sel: 0x2a1f48, sel2: 0x21183a, hover: 0x16141f, decks: [0x2dff9a, 0xff3dd5, 0x3de7ff, 0xffe23d] },
    // greys only, the decks stay coloured but softer: least distracting in a dark booth
    Theme { name: "Mono", bg: 0x0a0a0a, panel: 0x121212, panel_d: 0x0f0f0f, raise: 0x1a1a1a, line: 0x262626, line2: 0x333333, fg: 0xe8e8e8, muted: 0x8c8c8c, dim: 0x5a5a5a, wave_bg: 0x040404,
        sel: 0x2e2e2e, sel2: 0x262626, hover: 0x1c1c1c, decks: [0x8fb8de, 0xdeb38f, 0x8fdec8, 0xc89fde] },
];
impl Theme { pub fn deck(&self, i: usize) -> Color32 { hex(self.decks[i.min(3)]) } }
static THEME: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub fn theme() -> &'static Theme { &THEMES[THEME.load(std::sync::atomic::Ordering::Relaxed).min(THEMES.len() - 1)] }
/// pick a theme by name (unknown: Night)
pub fn set_theme(name: &str) { THEME.store(THEMES.iter().position(|t| t.name == name).unwrap_or(0), std::sync::atomic::Ordering::Relaxed); }
#[allow(non_snake_case)] pub fn BG() -> Color32 { hex(theme().bg) }
#[allow(non_snake_case)] pub fn PANEL() -> Color32 { hex(theme().panel) }
#[allow(non_snake_case)] pub fn PANEL_D() -> Color32 { hex(theme().panel_d) }
#[allow(non_snake_case)] pub fn RAISE() -> Color32 { hex(theme().raise) }
#[allow(non_snake_case)] pub fn LINE() -> Color32 { hex(theme().line) }
#[allow(non_snake_case)] pub fn LINE2() -> Color32 { hex(theme().line2) }
#[allow(non_snake_case)] pub fn FG() -> Color32 { hex(theme().fg) }
#[allow(non_snake_case)] pub fn MUTED() -> Color32 { hex(theme().muted) }
#[allow(non_snake_case)] pub fn DIM() -> Color32 { hex(theme().dim) }
#[allow(non_snake_case)] pub fn WAVE_BG() -> Color32 { hex(theme().wave_bg) }
#[allow(non_snake_case)] pub fn SEL() -> Color32 { hex(theme().sel) }
#[allow(non_snake_case)] pub fn SEL2() -> Color32 { hex(theme().sel2) }
#[allow(non_snake_case)] pub fn HOVER() -> Color32 { hex(theme().hover) }
/// the deck colours (Night: the web app's 1 blue, 2 orange, 3 teal, 4 magenta)
pub fn deck_color(i: usize) -> Color32 { hex(theme().decks[i.min(3)]) }
pub fn rgb(c: [u8; 3]) -> Color32 { Color32::from_rgb(c[0], c[1], c[2]) }
pub fn mono(size: f32) -> FontId { FontId::monospace(size) }
pub fn prop(size: f32) -> FontId { FontId::proportional(size) }
pub fn bold(size: f32) -> FontId { FontId::new(size, egui::FontFamily::Name("bold".into())) }

pub fn fmt_time(s: f64) -> String {
    // tenths first, so 59.96 reads 1:00.0 and not 0:60.0
    let s = if s.is_finite() && s > 0.0 { (s * 10.0).round() / 10.0 } else { 0.0 };
    let m = (s / 60.0).floor();
    format!("{}:{:04.1}", m as i64, s - m * 60.0)
}

/// a flat button; `lit` fills it with `accent`
pub fn button(ui: &mut Ui, text: &str, size: Vec2, lit: bool, accent: Color32, font: f32) -> Response {
    let (rect, r) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let p = ui.painter();
    let hover = r.hovered();
    let (fill, txt, stroke) = if lit { (accent, BG(), accent) } else { (if hover { HOVER() } else { RAISE() }, FG(), LINE2()) };
    p.rect_filled(rect, 4.0, fill);
    p.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, stroke), StrokeKind::Inside);
    p.text(rect.center(), egui::Align2::CENTER_CENTER, text, prop(font), txt);
    r
}
/// line icons for the header buttons (drawn, so they don't depend on a font having the glyph)
#[derive(Clone, Copy, PartialEq)]
pub enum Icon { Folder, Globe, List, Wave, Search, Palette, Deck, Speaker, Knob, Wifi, Bluetooth, Usb }
pub fn paint_icon(p: &egui::Painter, r: Rect, icon: Icon, c: Color32) {
    let st = Stroke::new(1.5_f32, c);
    let o = r.center();
    match icon {
        Icon::Folder => {
            let b = Rect::from_center_size(o + vec2(0.0, 1.0), vec2(14.0, 9.5));
            p.rect_stroke(b, 1.5, st, StrokeKind::Middle);
            p.line_segment([pos2(b.left(), b.top()), pos2(b.left() + 2.0, b.top() - 2.5)], st);
            p.line_segment([pos2(b.left() + 2.0, b.top() - 2.5), pos2(b.left() + 6.0, b.top() - 2.5)], st);
            p.line_segment([pos2(b.left() + 6.0, b.top() - 2.5), pos2(b.left() + 7.5, b.top())], st);
        }
        Icon::Globe => {
            p.circle_stroke(o, 6.5, st);
            p.line_segment([o - vec2(6.5, 0.0), o + vec2(6.5, 0.0)], st);
            let pts: Vec<Pos2> = (0..16).map(|k| { let a = PI * k as f32 / 8.0; o + vec2(2.8 * a.cos(), 6.5 * a.sin()) }).collect();
            p.add(egui::Shape::closed_line(pts, st));
        }
        Icon::Search => {
            let c0 = o + vec2(-1.5, -1.5);
            p.circle_stroke(c0, 4.8, st);
            p.line_segment([c0 + vec2(3.5, 3.5), o + vec2(6.0, 6.0)], Stroke::new(2.0_f32, c));
        }
        Icon::List => {
            for k in 0..3 {
                let y = o.y - 4.0 + k as f32 * 4.0;
                p.circle_filled(pos2(o.x - 6.0, y), 1.1, c);
                p.line_segment([pos2(o.x - 3.0, y), pos2(o.x + 7.0, y)], st);
            }
        }
        Icon::Palette => {
            p.circle_stroke(o, 6.5, st);
            for (dx, dy) in [(-3.0, -2.0), (0.5, -3.8), (3.6, -1.2)] { p.circle_filled(o + vec2(dx, dy), 1.2, c); }
            p.circle_filled(o + vec2(1.5, 3.0), 1.8, c);
        }
        Icon::Deck => {
            p.circle_stroke(o, 6.5, st);
            p.circle_filled(o, 1.6, c);
            p.line_segment([o, o + vec2(4.0, -4.0)], st);
        }
        Icon::Speaker => {
            let pts = vec![o + vec2(-6.5, -2.2), o + vec2(-3.5, -2.2), o + vec2(0.0, -5.5), o + vec2(0.0, 5.5), o + vec2(-3.5, 2.2), o + vec2(-6.5, 2.2)];
            p.add(egui::Shape::closed_line(pts, st));
            for rr in [3.0_f32, 5.8] {
                let arc: Vec<Pos2> = (0..=8).map(|k| { let a = -0.8 + 1.6 * k as f32 / 8.0; o + vec2(1.0 + rr * a.cos(), rr * a.sin()) }).collect();
                p.add(egui::Shape::line(arc, st));
            }
        }
        Icon::Knob => {
            for (k, x) in [-4.5_f32, 0.0, 4.5].iter().enumerate() {
                p.line_segment([pos2(o.x + x, o.y - 6.5), pos2(o.x + x, o.y + 6.5)], Stroke::new(1.0_f32, c));
                let y = o.y + [2.5, -3.0, 0.5][k];
                p.rect_filled(Rect::from_center_size(pos2(o.x + x, y), vec2(3.6, 2.6)), 0.5, c);
            }
        }
        Icon::Wifi => {
            p.circle_filled(o + vec2(0.0, 5.0), 1.4, c);
            for rr in [3.5_f32, 6.5, 9.5] {
                let arc: Vec<Pos2> = (0..=10).map(|k| { let a = -PI / 2.0 - 0.75 + 1.5 * k as f32 / 10.0; o + vec2(rr * a.cos(), 5.0 + rr * a.sin()) }).collect();
                p.add(egui::Shape::line(arc, st));
            }
        }
        Icon::Bluetooth => {
            let (t, b) = (o + vec2(0.0, -7.0), o + vec2(0.0, 7.0));
            p.line_segment([t, b], st);
            p.add(egui::Shape::line(vec![o + vec2(-4.0, -3.5), o + vec2(4.0, 3.5), b], st));
            p.add(egui::Shape::line(vec![o + vec2(-4.0, 3.5), o + vec2(4.0, -3.5), t], st));
        }
        Icon::Usb => {
            let b = Rect::from_center_size(o + vec2(0.0, 2.0), vec2(8.0, 9.0));
            p.rect_stroke(b, 1.5, st, StrokeKind::Middle);
            let h = Rect::from_min_max(pos2(o.x - 3.0, b.top() - 5.0), pos2(o.x + 3.0, b.top()));
            p.rect_stroke(h, 0.5, st, StrokeKind::Middle);
            // the plug's two contacts, so it reads as a stick, not a padlock
            for dx in [-1.3, 1.3] { p.rect_filled(Rect::from_center_size(pos2(o.x + dx, b.top() - 2.5), vec2(1.2, 1.2)), 0.0, c); }
        }
        Icon::Wave => {
            let hs = [1.0, 3.5, 6.0, 2.5, 5.0, 2.0, 4.0, 1.0];
            for (k, h) in hs.iter().enumerate() {
                let x = o.x - 7.0 + k as f32 * 2.0;
                p.line_segment([pos2(x, o.y - h), pos2(x, o.y + h)], Stroke::new(1.3_f32, c));
            }
        }
    }
}
/// a header button with an icon instead of a word (the word is the hover text)
pub fn icon_button(ui: &mut Ui, icon: Icon, lit: bool, accent: Color32) -> Response {
    let r = button(ui, "", vec2(30.0, 22.0), lit, accent, 11.0);
    paint_icon(ui.painter(), r.rect, icon, if lit { BG() } else { FG() });
    r
}
/// an outlined tag (Q, MT, GRID...): lit = coloured outline + text
pub fn tag(ui: &mut Ui, text: &str, lit: bool, color: Color32) -> Response {
    let g = ui.painter().layout_no_wrap(text.to_string(), prop(11.5), FG());
    let size = vec2(g.size().x + 12.0, 20.0);
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let c = if lit { color } else { DIM() };
    p.rect_stroke(rect, 3.0, Stroke::new(1.0_f32, c), StrokeKind::Inside);
    if r.hovered() { p.rect_filled(rect, 3.0, Color32::from_white_alpha(10)); }
    p.text(rect.center(), egui::Align2::CENTER_CENTER, text, prop(11.5), if lit { color } else { MUTED() });
    r
}
/// a pad-mode tab: an underline in the deck colour when it's the mode
pub fn tab(ui: &mut Ui, text: &str, size: Vec2, lit: bool, color: Color32, font: f32) -> Response {
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, if lit { Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 40) } else if r.hovered() { HOVER() } else { RAISE() });
    if lit { p.rect_filled(Rect::from_min_size(pos2(rect.left(), rect.bottom() - 2.0), vec2(rect.width(), 2.0)), 0.0, color); }
    p.text(rect.center(), egui::Align2::CENTER_CENTER, text, bold(font), if lit { color } else { MUTED() });
    r
}
pub fn round_button(ui: &mut Ui, text: &str, d: f32, lit: bool, accent: Color32) -> Response {
    let (rect, r) = ui.allocate_exact_size(vec2(d, d), Sense::click_and_drag());
    let p = ui.painter();
    let c = rect.center();
    p.circle_filled(c, d / 2.0 - 1.0, if lit { accent } else { RAISE() });
    p.circle_stroke(c, d / 2.0 - 1.0, Stroke::new(2.0_f32, if lit { accent } else { LINE2() }));
    p.text(c, egui::Align2::CENTER_CENTER, text, prop(d * 0.28), if lit { BG() } else { FG() });
    r
}

/// a rotary knob 0..1: drag up/down (Shift = fine), double-click resets, wheel nudges
pub fn knob(ui: &mut Ui, v: &mut f32, label: &str, color: Color32, default: f32, size: f32, frozen: bool) -> Response {
    let (rect, mut r) = ui.allocate_exact_size(vec2(size + 8.0, size + if label.is_empty() { 4.0 } else { 14.0 }), Sense::click_and_drag());
    if !frozen {
        if r.dragged() {
            let fine = ui.input(|i| i.modifiers.shift);
            *v = (*v - r.drag_delta().y / if fine { 600.0 } else { 150.0 }).clamp(0.0, 1.0);
            r.mark_changed();
        }
        if r.double_clicked() { *v = default; r.mark_changed(); }
        if r.hovered() {
            let s = ui.input(|i| i.smooth_scroll_delta.y);
            // a trackpad's two-finger scroll comes in many small steps: follow it, not one notch per event
            let fine = ui.input(|i| i.modifiers.shift);
            if s != 0.0 { *v = (*v + s / if fine { 1200.0 } else { 300.0 }).clamp(0.0, 1.0); r.mark_changed(); }
        }
    }
    let p = ui.painter();
    // as big as the space allows (the arc rides the edge, the cap sits inside it)
    let c = pos2(rect.center().x, rect.top() + size / 2.0 + 2.0);
    let rad = (size / 2.0 + 1.0).min(rect.width() / 2.0 - 1.0);
    p.circle_filled(c, rad - 3.0, Color32::from_rgb(0x25, 0x2a, 0x32));
    p.circle_stroke(c, rad - 3.0, Stroke::new(1.0_f32, if r.hovered() || r.dragged() { color } else { Color32::from_rgb(0x3a, 0x41, 0x4c) }));
    let ang = |t: f32| (-135.0 + 270.0 * t) * PI / 180.0;
    let arc = |a0: f32, a1: f32, col: Color32| {
        let n = ((a1 - a0).abs() * 20.0) as usize + 2;
        let pts: Vec<Pos2> = (0..=n).map(|k| { let a = a0 + (a1 - a0) * k as f32 / n as f32; pos2(c.x + rad * a.sin(), c.y - rad * a.cos()) }).collect();
        p.add(egui::Shape::line(pts, Stroke::new(3.0_f32, col)));
    };
    arc(ang(0.0), ang(1.0), Color32::from_rgb(0x2a, 0x2f, 0x38));
    let center = if (default - 0.5).abs() < 1e-3 { 0.5 } else { 0.0 };
    let (a, b) = (ang(v.min(center)), ang(v.max(center)));
    if (b - a).abs() > 0.01 { arc(a, b, color); }
    let a = ang(*v);
    p.line_segment([c, pos2(c.x + (rad - 5.0) * a.sin(), c.y - (rad - 5.0) * a.cos())], Stroke::new(2.5_f32, FG()));
    if !label.is_empty() { p.text(pos2(rect.center().x, rect.bottom() - 6.0), egui::Align2::CENTER_CENTER, label, prop(9.5), MUTED()); }
    r
}

/// a vertical fader 0 (bottom) .. 1 (top)
/// a vertical fader for a trackpad: grab and slide (it moves with the hand, it doesn't jump to the click),
/// Shift = fine, two-finger scroll nudges, double-click centres the tempo
pub fn vslider(ui: &mut Ui, v: &mut f32, size: Vec2, center_line: bool, frozen: bool) -> Response {
    let (rect, mut r) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if !frozen {
        let fine = ui.input(|i| i.modifiers.shift);
        let span = (rect.height() - 12.0).max(10.0) * if fine { 4.0 } else { 1.0 };
        if r.dragged() { let dy = r.drag_delta().y; if dy != 0.0 { *v = (*v - dy / span).clamp(0.0, 1.0); r.mark_changed(); } }
        if r.hovered() && !r.dragged() {
            let s = ui.input(|i| i.smooth_scroll_delta.y);
            if s != 0.0 { *v = (*v + s / (span * 2.0)).clamp(0.0, 1.0); r.mark_changed(); }
        }
        if r.double_clicked() && center_line { *v = 0.5; r.mark_changed(); }
    }
    let p = ui.painter();
    let track = Rect::from_center_size(rect.center(), vec2(4.0, rect.height() - 8.0));
    p.rect_filled(track, 2.0, Color32::from_rgb(0x07, 0x08, 0x0a));
    if center_line { p.hline(rect.x_range(), rect.center().y, Stroke::new(1.0_f32, LINE2())); }
    let y = rect.top() + 6.0 + (1.0 - *v) * (rect.height() - 12.0);
    let th = Rect::from_center_size(pos2(rect.center().x, y), vec2(size.x - 4.0, 12.0));
    p.rect_filled(th, 2.0, if r.hovered() || r.dragged() { FG() } else { Color32::from_rgb(0xc9, 0xcf, 0xd8) });
    p.hline(th.x_range(), y, Stroke::new(1.0_f32, BG()));
    r
}
/// a horizontal fader: grab and slide, Shift = fine, two-finger scroll (either way) nudges, double-click centres
pub fn hslider(ui: &mut Ui, v: &mut f32, size: Vec2, frozen: bool) -> Response {
    let (rect, mut r) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if !frozen {
        let fine = ui.input(|i| i.modifiers.shift);
        let span = (rect.width() - 16.0).max(10.0) * if fine { 4.0 } else { 1.0 };
        if r.dragged() { let dx = r.drag_delta().x; if dx != 0.0 { *v = (*v + dx / span).clamp(0.0, 1.0); r.mark_changed(); } }
        if r.hovered() && !r.dragged() {
            let d = ui.input(|i| i.smooth_scroll_delta);
            let s = if d.x.abs() > d.y.abs() { d.x } else { d.y };
            if s != 0.0 { *v = (*v + s / (span * 2.0)).clamp(0.0, 1.0); r.mark_changed(); }
        }
        if r.double_clicked() { *v = 0.5; r.mark_changed(); }
    }
    let p = ui.painter();
    p.rect_filled(Rect::from_center_size(rect.center(), vec2(rect.width() - 8.0, 4.0)), 2.0, Color32::from_rgb(0x07, 0x08, 0x0a));
    let x = rect.left() + 8.0 + *v * (rect.width() - 16.0);
    let th = Rect::from_center_size(pos2(x, rect.center().y), vec2(14.0, size.y - 4.0));
    p.rect_filled(th, 2.0, if r.hovered() || r.dragged() { FG() } else { Color32::from_rgb(0xc9, 0xcf, 0xd8) });
    p.vline(x, th.y_range(), Stroke::new(1.0_f32, BG()));
    r
}
pub fn meter(ui: &mut Ui, size: Vec2, level: f32) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    paint_meter(ui.painter(), rect, level);
}
pub fn paint_meter(p: &egui::Painter, rect: Rect, level: f32) {
    p.rect_filled(rect, 1.0, Color32::from_rgb(0x07, 0x08, 0x0a));
    let segs = 15;
    let sh = rect.height() / segs as f32;
    for s in 0..segs {
        let on = level * segs as f32 > s as f32;
        let c = if on { if s >= 13 { RED } else if s >= 10 { CUE } else { GREEN } } else { Color32::from_rgb(0x15, 0x18, 0x1d) };
        let y = rect.bottom() - (s + 1) as f32 * sh;
        p.rect_filled(Rect::from_min_max(pos2(rect.left() + 1.0, y + 1.0), pos2(rect.right() - 1.0, y + sh - 1.0)), 0.0, c);
    }
}

/// jog wheel picture; the caller handles the input
pub fn paint_jog(p: &egui::Painter, rect: Rect, d: &Deck, i: usize) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) / 2.0;
    let col = deck_color(i);
    p.circle_filled(c, r - 3.0, Color32::from_rgb(0x14, 0x17, 0x1c));
    p.circle_stroke(c, r - 3.0, Stroke::new(5.0_f32, if d.touch { col } else { Color32::from_rgb(0x2d, 0x33, 0x3d) }));
    p.circle_filled(c, r * 0.62, Color32::from_rgb(0x1c, 0x20, 0x27));
    let now = if d.loaded() { d.now() } else { 0.0 };
    let ang = (now / 1.8 * std::f64::consts::TAU) as f32;
    let (s, co) = ang.sin_cos();
    let a = pos2(c.x + (r - 6.0) * s, c.y - (r - 6.0) * co);
    let b = pos2(c.x + (r * 0.68) * s, c.y - (r * 0.68) * co);
    p.line_segment([a, b], Stroke::new(5.0_f32, col));
    if d.loaded() && d.duration > 0.0 {
        let frac = (now / d.duration) as f32;
        let n = (frac * 80.0) as usize + 2;
        let rr = r - 13.0;
        let pts: Vec<Pos2> = (0..=n).map(|k| { let t = -PI / 2.0 + frac * 2.0 * PI * k as f32 / n as f32; pos2(c.x + rr * t.cos(), c.y + rr * t.sin()) }).collect();
        p.add(egui::Shape::line(pts, Stroke::new(3.0_f32, Color32::from_white_alpha(46))));
    }
    p.text(c, egui::Align2::CENTER_CENTER, if d.playing { "▶" } else if d.loaded() { "❚❚" } else { "" }, prop(r * 0.36), FG());
}

// ---------------------------------------------------------------------------------------------
// waveforms: aggregate the 300/s data into one column, then paint it as coloured bars into a texture
#[derive(Clone, Copy, Default)]
pub struct Col { pub peak: u8, pub rms: u8, pub low: u8, pub mid: u8, pub high: u8 }

fn agg(peak: &[u8], rms: &[u8], low: &[u8], mid: &[u8], high: &[u8], a: i64, b: i64) -> Col {
    let n = peak.len() as i64;
    let mut c = Col::default();
    let b = if b <= a { a + 1 } else { b };
    let (lo, hi) = (a.max(0), b.min(n));
    for j in lo..hi {
        let j = j as usize;
        c.peak = c.peak.max(peak[j]); c.rms = c.rms.max(rms[j]); c.low = c.low.max(low[j]); c.mid = c.mid.max(mid[j]); c.high = c.high.max(high[j]);
    }
    c
}
fn scale(c: Color32, k: f32) -> Color32 { Color32::from_rgb((c.r() as f32 * k) as u8, (c.g() as f32 * k) as u8, (c.b() as f32 * k) as u8) }

/// one column's bars, outermost first: (half-height, colour)
fn col_bars(half: f32, v: Col, rgb_style: bool, dim: f32) -> [(f32, Color32); 3] {
    if !rgb_style {
        // 3-band: lows blue, mids amber, highs white, layered
        let amp = (v.peak as f32 / 255.0).max(0.25);
        let hl = v.low as f32 / 255.0 * half * amp;
        let hm = v.mid as f32 / 255.0 * half * 0.72 * amp;
        let hh = v.high as f32 / 255.0 * half * 0.42 * amp;
        return [(hl, scale(Color32::from_rgb(0x1f, 0x6f, 0xe0), dim)), (hm, scale(Color32::from_rgb(0xe8, 0x96, 0x2e), dim)), (hh, scale(Color32::from_rgb(0xf2, 0xf5, 0xf8), dim))];
    }
    // RGB: red = lows, green = mids, blue = highs; brightness follows loudness; RMS core lighter.
    // squaring the band levels lets the dominant band set the hue (kicks red, hats blue)
    let (mut r, mut g, mut b) = ((v.low as f32 / 255.0).powi(2), (v.mid as f32 / 255.0).powi(2), (v.high as f32 / 255.0).powi(2));
    let mx = r.max(g).max(b).max(0.02);
    let amp = v.peak as f32 / 255.0;
    let bri = (0.55 + 0.45 * (amp * 1.25).min(1.0)) * dim;
    r = r / mx * 0.92 + 0.08; g = g / mx * 0.92 + 0.08; b = b / mx * 0.92 + 0.08;
    let outer = Color32::from_rgb((r * 235.0 * bri) as u8, (g * 235.0 * bri) as u8, (b * 235.0 * bri) as u8);
    let core = Color32::from_rgb(((r * 150.0 + 105.0) * bri).min(255.0) as u8, ((g * 150.0 + 105.0) * bri).min(255.0) as u8, ((b * 150.0 + 105.0) * bri).min(255.0) as u8);
    let ph = (amp * half).round();
    let rh = (v.rms as f32 / 255.0 * half).round();
    [(ph, outer), (rh.min(ph), core), (0.0, core)]
}

/// the scrolling waveform's pictures, cut into tiles of TILE physical pixels at the deck's normal speed and
/// kept on the GPU. Drawn every frame at the exact (fractional) position with linear filtering, so the
/// waveform glides by fractions of a pixel instead of stepping whole pixels (4, 5, 4… px a frame at
/// 60 Hz was the judder) and no column is re-cut or re-drawn while it scrolls. Tempo changes stretch the
/// tiles; a new zoom, height, style or analysis starts over.
#[derive(Default)]
pub struct WaveCache { key: (usize, usize, u64, u32, u32, bool), tiles: std::collections::HashMap<i64, egui::TextureHandle> }
const TILE: usize = 512;
impl WaveCache {
    fn tile(&mut self, ctx: &egui::Context, w: &Wave, k: i64, cps: f64, hp: usize, rgb_style: bool) -> egui::TextureId {
        self.tiles.entry(k).or_insert_with(|| {
            let img = wave_image(TILE, hp, rgb_style, |x| {
                let c = k * TILE as i64 + x as i64;
                let k0 = (c as f64 / cps * w.wps).floor() as i64;
                let k1 = ((c + 1) as f64 / cps * w.wps).floor() as i64;
                (k1 >= 0 && k0 < w.peak.len() as i64).then(|| agg(&w.peak, &w.rms, &w.low, &w.mid, &w.high, k0, k1))
            });
            ctx.load_texture(format!("wave{k}"), img, egui::TextureOptions::LINEAR)
        }).id()
    }
}

/// a waveform picture `w`×`h` pixels on a transparent background: column x from `col(x)`
fn wave_image(w: usize, h: usize, rgb_style: bool, col: impl Fn(usize) -> Option<Col>) -> egui::ColorImage {
    let mut img = egui::ColorImage::filled([w, h], Color32::TRANSPARENT);
    let mid = h as f32 / 2.0;
    let half = mid - 2.0;
    for x in 0..w {
        let Some(v) = col(x) else { continue };
        for (bh, c) in col_bars(half, v, rgb_style, 1.0) {
            if bh <= 0.0 { continue; }
            let (y0, y1) = (((mid - bh).round().max(0.0)) as usize, ((mid + bh).round() as usize).min(h));
            for y in y0..y1 { img.pixels[y * w + x] = c; }
        }
    }
    img
}

/// a whole-track overview drawn once into a texture (it only changes with the track, size or style;
/// rebuilding thousands of bars every frame for up to 8 overviews was most of the UI's drawing work)
#[derive(Default)]
pub struct OverviewCache(Option<((usize, u32, u32, bool), egui::TextureHandle)>);

/// the scrolling waveform for deck `i` (tempo-scaled like rekordbox: synced decks line up)
pub fn paint_lane(p: &egui::Painter, rect: Rect, d: &Deck, i: usize, zoom: f64, rgb_style: bool, cache: &mut WaveCache) {
    p.rect_filled(rect, 0.0, WAVE_BG());
    let col = deck_color(i);
    let mid = rect.center().y;
    let cx = rect.center().x;
    if !d.loaded() || d.ana.is_none() {
        cache.tiles.clear();
        p.rect_filled(Rect::from_min_size(rect.min, vec2(4.0, rect.height())), 0.0, col);
        p.text(pos2(rect.left() + 14.0, mid), egui::Align2::LEFT_CENTER, if d.loading { format!("Deck {}: analyzing…", i + 1) } else { format!("Deck {}: empty", i + 1) }, prop(13.0), DIM());
        return;
    }
    let a = d.ana.as_ref().unwrap();
    let w: &Wave = &a.wave;
    let t = d.vis_now();
    let pps = rect.width() as f64 / zoom / d.rate().max(0.05);
    let t0 = t - (cx - rect.left()) as f64 / pps;
    let ppp = p.ctx().pixels_per_point();
    // tiles at normal speed: cps columns (physical pixels) per second of track; on screen each lasts 1/cps s
    let cps = rect.width() as f64 * ppp as f64 / zoom;
    let hp = (rect.height() * ppp).round().max(4.0) as usize;
    let key = (w.peak.as_ptr() as usize, w.peak.len(), zoom.to_bits(), hp as u32, ppp.to_bits(), rgb_style);
    if cache.key != key { cache.tiles.clear(); cache.key = key; }
    let t1 = t + (rect.right() - cx) as f64 / pps;
    let (k0, k1) = ((t0 * cps / TILE as f64).floor() as i64, (t1 * cps / TILE as f64).floor() as i64);
    cache.tiles.retain(|k, _| *k >= k0 - 2 && *k <= k1 + 2);
    let clip = p.with_clip_rect(rect);
    let tile_s = TILE as f64 / cps;
    for k in k0..=k1 {
        let (s0, s1) = (k as f64 * tile_s, (k + 1) as f64 * tile_s);
        if s1 < 0.0 || s0 > w.duration + tile_s { continue; }
        let id = cache.tile(p.ctx(), w, k, cps, hp, rgb_style);
        let (x0, x1) = (cx + ((s0 - t) * pps) as f32, cx + ((s1 - t) * pps) as f32);
        // what's played is dimmed: the part left of the playhead drawn tinted
        let part = |a: f32, b: f32, tint: Color32| {
            if b <= a { return; }
            let uv = Rect::from_min_max(pos2((a - x0) / (x1 - x0), 0.0), pos2((b - x0) / (x1 - x0), 1.0));
            clip.image(id, Rect::from_min_max(pos2(a, rect.top()), pos2(b, rect.bottom())), uv, tint);
        };
        part(x0, x1.min(cx), Color32::from_gray(158));
        part(x0.max(cx), x1, Color32::WHITE);
    }
    p.rect_filled(Rect::from_min_size(rect.min, vec2(4.0, rect.height())), 0.0, col);
    let xt = |time: f64| cx + ((time - t) * pps) as f32;
    // loop region
    if let (Some(la), Some(lb)) = (d.lp.a, d.lp.b) {
        let (xa, xb) = (xt(la).max(rect.left()), xt(lb).min(rect.right()));
        if xb > xa { p.rect_filled(Rect::from_x_y_ranges(xa..=xb, rect.y_range()), 0.0, Color32::from_rgba_unmultiplied(245, 214, 74, if d.lp.on { 56 } else { 20 })); }
    }
    // beat grid: every beat thin, every bar bright
    let b0 = d.beat_at(t0).ceil() as i64;
    let b1 = d.beat_at(t0 + rect.width() as f64 / pps).floor() as i64;
    if b1 - b0 < 600 {
        for b in b0..=b1 {
            let x = xt(d.time_of_beat(b as f64)).round();
            if b.rem_euclid(4) == 0 { p.rect_filled(Rect::from_min_size(pos2(x, rect.top()), vec2(2.0, rect.height())), 0.0, Color32::from_white_alpha(190)); }
            else {
                p.rect_filled(Rect::from_min_size(pos2(x, rect.top()), vec2(1.0, 8.0)), 0.0, Color32::from_white_alpha(64));
                p.rect_filled(Rect::from_min_size(pos2(x, rect.bottom() - 8.0), vec2(1.0, 8.0)), 0.0, Color32::from_white_alpha(64));
            }
        }
    }
    // cue + hot cues
    let marker = |time: f64, c: Color32, label: &str| {
        let x = xt(time);
        if x < rect.left() - 20.0 || x > rect.right() + 20.0 { return; }
        p.rect_filled(Rect::from_min_size(pos2(x - 1.0, rect.top()), vec2(2.0, rect.height())), 0.0, c);
        p.add(egui::Shape::convex_polygon(vec![pos2(x - 6.0, rect.top()), pos2(x + 6.0, rect.top()), pos2(x, rect.top() + 8.0)], c, Stroke::NONE));
        if !label.is_empty() { p.text(pos2(x + 4.0, rect.top() + 14.0), egui::Align2::LEFT_CENTER, label, prop(11.0), c); }
    };
    marker(d.cue, CUE, "");
    for (k, h) in d.hot.iter().enumerate() { if let Some(c) = h { marker(*c, GREEN, &"ABCDEFGH"[k..k + 1]); } }
    // phrases: coloured strip on top with the name at each phrase start
    for (s, e, bars, label, _) in d.phrase_spans() {
        let (x0, x1) = (xt(s), xt(e));
        if x1 < rect.left() || x0 > rect.right() { continue; }
        let c = rgb(phrase_color(&label));
        let r = Rect::from_min_max(pos2(x0.max(rect.left()), rect.top()), pos2((x1 - 2.0).min(rect.right()), rect.top() + 4.0));
        if r.width() > 0.0 { p.rect_filled(r, 0.0, c); }
        if x0 >= rect.left() {
            p.rect_filled(Rect::from_min_size(pos2(x0, rect.top()), vec2(2.0, rect.height())), 0.0, c);
            p.text(pos2(x0 + 5.0, rect.bottom() - 8.0), egui::Align2::LEFT_CENTER, format!("{} {}", label.to_uppercase(), bars), prop(11.0), c);
        }
    }
    // playhead
    p.rect_filled(Rect::from_min_size(pos2(cx - 1.0, rect.top()), vec2(2.0, rect.height())), 0.0, RED);
}

/// the whole-track overview with phrase strip, cues, loop and playhead
pub fn paint_overview(p: &egui::Painter, rect: Rect, d: &Deck, rgb_style: bool, cache: &mut OverviewCache) {
    p.rect_filled(rect, CornerRadius::same(3), WAVE_BG());
    let Some(a) = &d.ana else { return };
    let o: &Overview = &a.overview;
    let n = o.peak.len() as f64;
    let ph = 5.0;
    let ppp = p.ctx().pixels_per_point();
    let (wp, hp) = ((rect.width() * ppp).round().max(1.0) as usize, ((rect.height() - ph) * ppp).round().max(2.0) as usize);
    let key = (o.peak.as_ptr() as usize, wp as u32, hp as u32, rgb_style);
    if cache.0.as_ref().is_none_or(|(k, _)| *k != key) {
        let img = wave_image(wp, hp, rgb_style, |x| {
            let k0 = (x as f64 / wp as f64 * n).floor() as i64;
            let k1 = ((x + 1) as f64 / wp as f64 * n).floor() as i64;
            Some(agg(&o.peak, &o.rms, &o.low, &o.mid, &o.high, k0, k1))
        });
        cache.0 = Some((key, p.ctx().load_texture("overview", img, egui::TextureOptions::LINEAR)));
    }
    if let Some((_, t)) = &cache.0 {
        let r = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - ph));
        p.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    let w = rect.width().max(1.0);
    let dur = d.duration.max(0.001);
    let xt = |t: f64| rect.left() + (t / dur) as f32 * w;
    for (s, e, _, label, _) in d.phrase_spans() {
        let (x0, x1) = (xt(s), xt(e));
        p.rect_filled(Rect::from_min_max(pos2(x0, rect.bottom() - ph), pos2((x1 - 1.0).max(x0 + 1.0).min(rect.right()), rect.bottom())), 0.0, rgb(phrase_color(&label)));
    }
    if !d.loaded() { return; }
    let x = xt(d.now());
    p.rect_filled(Rect::from_min_max(rect.min, pos2(x, rect.bottom())), 0.0, Color32::from_black_alpha(115));
    if let (Some(la), Some(lb)) = (d.lp.a, d.lp.b) {
        p.rect_filled(Rect::from_min_max(pos2(xt(la), rect.top()), pos2(xt(lb).max(xt(la) + 2.0), rect.bottom())), 0.0, Color32::from_rgba_unmultiplied(245, 214, 74, if d.lp.on { 76 } else { 30 }));
    }
    for h in d.hot.iter().flatten() { p.rect_filled(Rect::from_min_size(pos2(xt(*h) - 1.0, rect.top()), vec2(2.0, rect.height())), 0.0, GREEN); }
    p.rect_filled(Rect::from_min_size(pos2(xt(d.cue) - 1.0, rect.top()), vec2(2.0, rect.height())), 0.0, CUE);
    p.rect_filled(Rect::from_min_size(pos2(x - 1.0, rect.top()), vec2(2.0, rect.height())), 0.0, RED);
}

pub fn fonts(ctx: &egui::Context) {
    use egui::{FontData, FontFamily};
    use std::sync::Arc;
    let mut f = egui::FontDefinitions::default();
    f.font_data.insert("inter".into(), Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Inter-Medium.otf"))));
    f.font_data.insert("inter-bold".into(), Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Inter-Bold.otf"))));
    f.font_data.insert("dejavu".into(), Arc::new(FontData::from_static(include_bytes!("../assets/fonts/DejaVuSans.ttf"))));
    f.font_data.insert("dejavu-mono".into(), Arc::new(FontData::from_static(include_bytes!("../assets/fonts/DejaVuSansMono.ttf"))));
    let prop = f.families.entry(FontFamily::Proportional).or_default();
    prop.insert(0, "inter".into());
    prop.insert(1, "dejavu".into());
    let mono = f.families.entry(FontFamily::Monospace).or_default();
    mono.insert(0, "dejavu-mono".into());
    let mut b: Vec<String> = vec!["inter-bold".into(), "dejavu".into()];
    b.extend(f.families[&FontFamily::Proportional].iter().skip(2).cloned());
    f.families.insert(FontFamily::Name("bold".into()), b);
    ctx.set_fonts(f);
}

pub fn style(ctx: &egui::Context) {
    fonts(ctx);
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG();
    v.window_fill = PANEL();
    v.extreme_bg_color = Color32::from_rgb(0x0e, 0x10, 0x14);
    v.faint_bg_color = PANEL();
    v.window_stroke = Stroke::new(1.0_f32, LINE2());
    v.override_text_color = Some(FG());
    v.selection.bg_fill = SEL();
    v.selection.stroke = Stroke::new(1.0_f32, deck_color(0));
    for w in [&mut v.widgets.inactive, &mut v.widgets.noninteractive] { w.bg_fill = RAISE(); w.weak_bg_fill = RAISE(); w.bg_stroke = Stroke::new(1.0_f32, LINE2()); }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE());
    v.widgets.hovered.weak_bg_fill = HOVER();
    v.widgets.hovered.bg_fill = HOVER();
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = vec2(6.0, 4.0);
        s.spacing.button_padding = vec2(8.0, 3.0);
        s.text_styles.insert(egui::TextStyle::Body, prop(13.0));
        s.text_styles.insert(egui::TextStyle::Button, prop(12.5));
        s.text_styles.insert(egui::TextStyle::Small, prop(10.5));
    });
}

pub fn pt(p: Pos2) -> Pos2 { p }
