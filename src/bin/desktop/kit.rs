//! Look and feel of the desktop client: palette, fonts, vector icons and the small widgets the
//! pages are built from.

use std::f32::consts::TAU;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Image, Margin, Painter, Pos2, Rect,
    Response, RichText, Sense, Shape, Stroke, StrokeKind, TextStyle, Ui, Vec2, pos2, vec2,
};
use egui::epaint::Mesh;
use egui::load::TexturePoll;

pub const BG: Color32 = Color32::from_rgb(0x0b, 0x0a, 0x10);
pub const PANEL: Color32 = Color32::from_rgb(0x14, 0x12, 0x1c);
pub const PANEL2: Color32 = Color32::from_rgb(0x1e, 0x1b, 0x2a);
pub const RAISED: Color32 = Color32::from_rgb(0x29, 0x25, 0x39);
pub const LINE: Color32 = Color32::from_rgb(0x2c, 0x28, 0x40);
pub const TEXT: Color32 = Color32::from_rgb(0xf1, 0xef, 0xf8);
pub const SOFT: Color32 = Color32::from_rgb(0xc9, 0xc4, 0xdc);
pub const MUTED: Color32 = Color32::from_rgb(0x8e, 0x88, 0xa8);
pub const ACCENT: Color32 = Color32::from_rgb(0x7b, 0x5b, 0xf5);
pub const ACCENT_HI: Color32 = Color32::from_rgb(0x93, 0x78, 0xff);
pub const ACCENT2: Color32 = Color32::from_rgb(0xa9, 0x93, 0xff);
pub const ERR: Color32 = Color32::from_rgb(0xff, 0x6b, 0x6b);
pub const OK: Color32 = Color32::from_rgb(0x3e, 0xcf, 0x8e);
pub const GOLD: Color32 = Color32::from_rgb(0xf5, 0xc5, 0x18);
pub const TEAL: Color32 = Color32::from_rgb(0x3c, 0xc8, 0xc8);

pub const SEMIBOLD: &str = "semibold";
pub const BOLD: &str = "bold";

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

/// Text in the semibold face.
pub fn strong(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(semibold(size))
}

pub fn muted(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).size(size).color(MUTED)
}

pub fn apply_theme(ctx: &egui::Context) {
    install_fonts(ctx);
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL2;
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.extreme_bg_color = PANEL2;
    v.faint_bg_color = PANEL;
    v.hyperlink_color = ACCENT2;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.text_edit_bg_color = Some(PANEL2);
    v.popup_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(140) };
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.inactive.bg_fill = PANEL2;
    v.widgets.inactive.weak_bg_fill = PANEL2;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, SOFT);
    v.widgets.hovered.bg_fill = RAISED;
    v.widgets.hovered.weak_bg_fill = RAISED;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, TEXT);
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.active.weak_bg_fill = ACCENT;
    v.widgets.open.weak_bg_fill = RAISED;
    v.widgets.open.bg_fill = RAISED;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.button_padding = vec2(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.spacing.scroll = egui::style::ScrollStyle::floating();
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
        s.text_styles.insert(TextStyle::Heading, bold(24.0));
    });
}

/// Uses Avenir Next when the system has it (macOS); egui's bundled fonts stay as fallbacks, so the
/// named families always resolve.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let defaults = fonts.families[&FontFamily::Proportional].clone();
    let mut family = |name: &str, faces: Vec<String>| {
        fonts.families.insert(FontFamily::Name(name.into()), faces.into_iter().chain(defaults.clone()).collect());
    };
    let collection = std::fs::read("/System/Library/Fonts/Avenir Next.ttc").ok();
    let Some(bytes) = collection else {
        family(SEMIBOLD, Vec::new());
        family(BOLD, Vec::new());
        ctx.set_fonts(fonts);
        return;
    };
    let faces = [("avenir-regular", 7), ("avenir-demibold", 2), ("avenir-bold", 0)];
    family(SEMIBOLD, vec![faces[1].0.into()]);
    family(BOLD, vec![faces[2].0.into()]);
    for (name, index) in faces {
        let mut data = egui::FontData::from_owned(bytes.clone());
        data.index = index;
        data.tweak.y_offset_factor = 0.04;
        fonts.font_data.insert(name.into(), data.into());
    }
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, faces[0].0.into());
    ctx.set_fonts(fonts);
}

// ---- Icons ----

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Home,
    Compass,
    Bookmark,
    Addons,
    Search,
    ChevronLeft,
    ChevronRight,
    Close,
    Play,
    Pause,
    Plus,
    Check,
    Star,
    Replay10,
    Forward10,
    Volume,
    Mute,
    Fullscreen,
    ExitFullscreen,
    External,
    Subtitles,
    Audio,
    Next,
    Help,
    Copy,
}

/// Draws `icon` in `rect` (designed on a 24×24 grid).
pub fn paint_icon(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let k = rect.width().min(rect.height()) / 24.0;
    let o = rect.center() - vec2(12.0, 12.0) * k;
    let at = |x: f32, y: f32| o + vec2(x, y) * k;
    let stroke = Stroke::new(1.8 * k, color);
    let line = |pts: &[(f32, f32)]| {
        p.add(Shape::line(pts.iter().map(|&(x, y)| at(x, y)).collect(), stroke));
    };
    let fill = |pts: &[(f32, f32)]| {
        p.add(Shape::convex_polygon(pts.iter().map(|&(x, y)| at(x, y)).collect(), color, Stroke::NONE));
    };
    let arc = |c: (f32, f32), r: f32, from: f32, to: f32| -> Vec<Pos2> {
        (0..=24).map(|i| from + (to - from) * i as f32 / 24.0).map(|a| at(c.0 + r * a.cos(), c.1 + r * a.sin())).collect()
    };
    match icon {
        Icon::Home => {
            line(&[(3.0, 11.5), (12.0, 3.5), (21.0, 11.5)]);
            line(&[(5.5, 9.5), (5.5, 20.5), (18.5, 20.5), (18.5, 9.5)]);
            line(&[(10.0, 20.5), (10.0, 15.0), (14.0, 15.0), (14.0, 20.5)]);
        }
        Icon::Compass => {
            p.circle_stroke(at(12.0, 12.0), 9.0 * k, stroke);
            fill(&[(15.8, 8.2), (13.4, 13.4), (8.2, 15.8), (10.6, 10.6)]);
        }
        Icon::Bookmark => line(&[(6.0, 3.5), (18.0, 3.5), (18.0, 20.5), (12.0, 16.0), (6.0, 20.5), (6.0, 3.5)]),
        Icon::Addons => {
            for (x, y) in [(4.0, 4.0), (13.5, 4.0), (4.0, 13.5), (13.5, 13.5)] {
                let r = Rect::from_min_max(at(x, y), at(x + 6.5, y + 6.5));
                p.rect_stroke(r, 2.0 * k, stroke, StrokeKind::Middle);
            }
        }
        Icon::Search => {
            p.circle_stroke(at(10.5, 10.5), 6.5 * k, stroke);
            line(&[(15.3, 15.3), (20.5, 20.5)]);
        }
        Icon::ChevronLeft => line(&[(15.0, 5.0), (8.0, 12.0), (15.0, 19.0)]),
        Icon::ChevronRight => line(&[(9.0, 5.0), (16.0, 12.0), (9.0, 19.0)]),
        Icon::Close => {
            line(&[(6.0, 6.0), (18.0, 18.0)]);
            line(&[(18.0, 6.0), (6.0, 18.0)]);
        }
        Icon::Play => fill(&[(7.0, 4.5), (19.5, 12.0), (7.0, 19.5)]),
        Icon::Pause => {
            for x in [6.0, 14.0] {
                p.rect_filled(Rect::from_min_max(at(x, 5.0), at(x + 4.0, 19.0)), 1.0 * k, color);
            }
        }
        Icon::Plus => {
            line(&[(12.0, 5.0), (12.0, 19.0)]);
            line(&[(5.0, 12.0), (19.0, 12.0)]);
        }
        Icon::Check => line(&[(5.0, 12.5), (10.0, 17.5), (19.0, 7.0)]),
        Icon::Star => {
            let c = at(12.0, 12.8);
            let pt = |i: usize, r: f32| c + egui::emath::Rot2::from_angle(i as f32 * TAU / 10.0) * vec2(0.0, -r * k);
            let inner: Vec<Pos2> = (0..5).map(|i| pt(2 * i + 1, 4.2)).collect();
            p.add(Shape::convex_polygon(inner.clone(), color, Stroke::NONE));
            let mut mesh = Mesh::default();
            for i in 0..5 {
                let base = mesh.vertices.len() as u32;
                mesh.colored_vertex(pt(2 * i, 9.5), color);
                mesh.colored_vertex(inner[i], color);
                mesh.colored_vertex(inner[(i + 4) % 5], color);
                mesh.add_triangle(base, base + 1, base + 2);
            }
            p.add(mesh);
        }
        Icon::Replay10 | Icon::Forward10 => {
            // An arc open at the top with a chevron at its top end (↺ / ↻), and "10" inside.
            let dir = if icon == Icon::Forward10 { -1.0 } else { 1.0 };
            let (start, end) = (-TAU / 4.0, -TAU / 4.0 + dir * TAU * 0.8);
            p.add(Shape::line(arc((12.0, 12.5), 8.5, start, end), stroke));
            let tip = at(12.0, 4.0);
            p.add(Shape::line(vec![tip + vec2(3.0 * dir, -3.0) * k, tip, tip + vec2(3.0 * dir, 3.0) * k], stroke));
            p.text(at(12.0, 13.0), Align2::CENTER_CENTER, "10", semibold(7.5 * k), color);
        }
        Icon::Volume | Icon::Mute => {
            p.rect_filled(Rect::from_min_max(at(3.5, 9.0), at(8.0, 15.0)), 1.0 * k, color);
            fill(&[(7.5, 9.0), (13.0, 4.5), (13.0, 19.5), (7.5, 15.0)]);
            if icon == Icon::Mute {
                line(&[(16.0, 9.0), (21.0, 15.0)]);
                line(&[(21.0, 9.0), (16.0, 15.0)]);
            } else {
                p.add(Shape::line(arc((13.0, 12.0), 4.0, -0.8, 0.8), stroke));
                p.add(Shape::line(arc((13.0, 12.0), 7.5, -0.85, 0.85), stroke));
            }
        }
        Icon::Fullscreen => {
            line(&[(4.0, 9.0), (4.0, 4.0), (9.0, 4.0)]);
            line(&[(15.0, 4.0), (20.0, 4.0), (20.0, 9.0)]);
            line(&[(20.0, 15.0), (20.0, 20.0), (15.0, 20.0)]);
            line(&[(9.0, 20.0), (4.0, 20.0), (4.0, 15.0)]);
        }
        Icon::ExitFullscreen => {
            line(&[(4.0, 9.0), (9.0, 9.0), (9.0, 4.0)]);
            line(&[(15.0, 4.0), (15.0, 9.0), (20.0, 9.0)]);
            line(&[(20.0, 15.0), (15.0, 15.0), (15.0, 20.0)]);
            line(&[(9.0, 20.0), (9.0, 15.0), (4.0, 15.0)]);
        }
        Icon::External => {
            line(&[(10.0, 4.5), (4.5, 4.5), (4.5, 19.5), (19.5, 19.5), (19.5, 14.0)]);
            line(&[(12.0, 12.0), (19.5, 4.5)]);
            line(&[(13.5, 4.5), (19.5, 4.5), (19.5, 10.5)]);
        }
        Icon::Subtitles => {
            p.rect_stroke(Rect::from_min_max(at(3.0, 5.0), at(21.0, 19.0)), 3.0 * k, stroke, StrokeKind::Middle);
            line(&[(7.0, 11.0), (10.5, 11.0)]);
            line(&[(13.0, 11.0), (17.0, 11.0)]);
            line(&[(7.0, 14.5), (14.0, 14.5)]);
        }
        Icon::Audio => {
            for (x, h) in [(5.0, 4.0), (9.0, 10.0), (13.0, 14.0), (17.0, 7.0)] {
                line(&[(x + 1.0, 12.0 - h / 2.0), (x + 1.0, 12.0 + h / 2.0)]);
            }
        }
        Icon::Next => {
            fill(&[(5.0, 5.0), (15.5, 12.0), (5.0, 19.0)]);
            p.rect_filled(Rect::from_min_max(at(16.5, 5.0), at(19.5, 19.0)), 1.0 * k, color);
        }
        Icon::Copy => {
            p.rect_stroke(Rect::from_min_max(at(8.5, 8.5), at(20.0, 20.0)), 2.5 * k, stroke, StrokeKind::Middle);
            line(&[(15.5, 4.0), (6.0, 4.0), (4.0, 6.0), (4.0, 15.5)]);
        }
        Icon::Help => {
            p.circle_stroke(at(12.0, 12.0), 9.0 * k, stroke);
            p.text(at(12.0, 12.5), Align2::CENTER_CENTER, "?", semibold(12.0 * k), color);
        }
    }
}

// ---- Painting helpers ----

/// Fills `rect` with a vertical gradient.
pub fn gradient_v(p: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    gradient(p, rect, [top, top, bottom, bottom]);
}

/// Fills `rect` with a horizontal gradient.
pub fn gradient_h(p: &Painter, rect: Rect, left: Color32, right: Color32) {
    gradient(p, rect, [left, right, right, left]);
}

/// Corners clockwise from the top left.
fn gradient(p: &Painter, rect: Rect, colors: [Color32; 4]) {
    let mut mesh = Mesh::default();
    for (pos, c) in [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()].into_iter().zip(colors) {
        mesh.colored_vertex(pos, c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(mesh);
}

/// Paints the image at `url` cropped to fill `rect` (CSS `object-fit: cover`). Returns false
/// until it has loaded.
pub fn paint_cover(ui: &Ui, url: &str, rect: Rect, radius: u8, tint: Color32) -> bool {
    let image = Image::new(url);
    let Ok(TexturePoll::Ready { texture }) = image.load_for_size(ui.ctx(), rect.size()) else { return false };
    let (img, want) = (texture.size, rect.size());
    let uv = if img.x * want.y > want.x * img.y {
        let w = (want.x / want.y) * img.y / img.x;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = (want.y / want.x) * img.x / img.y;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 4.0), pos2(1.0, (1.0 - h) / 4.0 + h))
    };
    Image::from_texture(texture).uv(uv).corner_radius(radius).tint(tint).paint_at(ui, rect);
    true
}

/// Whether the image at `url` failed to load (as opposed to still loading).
pub fn image_failed(ui: &Ui, url: &str, size: Vec2) -> bool {
    Image::new(url).load_for_size(ui.ctx(), size).is_err()
}

/// A pulsing placeholder while content loads.
pub fn skeleton(ui: &Ui, rect: Rect, radius: u8) {
    let t = ui.input(|i| i.time) as f32;
    let a = 0.5 + 0.5 * (t * 3.0).sin();
    let c = PANEL2.lerp_to_gamma(RAISED, a);
    ui.painter().rect_filled(rect, radius, c);
    ui.ctx().request_repaint();
}

// ---- Widgets ----

/// Round icon button; `size` is the button's diameter.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, tip: &str) -> Response {
    icon_button_with(ui, icon, size, tip, Color32::TRANSPARENT)
}

pub fn icon_button_with(ui: &mut Ui, icon: Icon, size: f32, tip: &str, fill: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let bg = fill.lerp_to_gamma(Color32::from_white_alpha(40), t);
    ui.painter().circle_filled(rect.center(), size / 2.0, bg);
    let color = if ui.is_enabled() { TEXT } else { MUTED };
    paint_icon(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(size * 0.55)), icon, color);
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    if tip.is_empty() { resp } else { resp.on_hover_text(tip) }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// Pill button with an optional leading icon.
pub fn button(ui: &mut Ui, kind: Kind, icon: Option<Icon>, text: &str) -> Response {
    button_sized(ui, kind, icon, text, 40.0)
}

pub fn button_sized(ui: &mut Ui, kind: Kind, icon: Option<Icon>, text: &str, height: f32) -> Response {
    let font = semibold(height * 0.36);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, Color32::PLACEHOLDER);
    let pad = height * 0.45;
    let icon_w = if icon.is_some() { height * 0.42 + if text.is_empty() { 0.0 } else { 8.0 } } else { 0.0 };
    let w = galley.size().x + icon_w + pad * 2.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    let enabled = ui.is_enabled();
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() && enabled, 0.12);
    let (bg, fg, stroke) = match kind {
        Kind::Primary => (ACCENT.lerp_to_gamma(ACCENT_HI, t), Color32::WHITE, Stroke::NONE),
        Kind::Secondary => (
            Color32::from_white_alpha(22).lerp_to_gamma(Color32::from_white_alpha(42), t),
            TEXT,
            Stroke::new(1.0, Color32::from_white_alpha(28)),
        ),
        Kind::Ghost => (Color32::TRANSPARENT.lerp_to_gamma(Color32::from_white_alpha(22), t), SOFT, Stroke::NONE),
        Kind::Danger => (ERR.gamma_multiply(0.8).lerp_to_gamma(ERR, t), Color32::WHITE, Stroke::NONE),
    };
    let (bg, fg) = if enabled { (bg, fg) } else { (bg.gamma_multiply(0.5), MUTED) };
    let p = ui.painter();
    p.rect(rect, height / 2.0, bg, stroke, StrokeKind::Inside);
    let mut x = rect.left() + pad;
    if let Some(icon) = icon {
        let s = height * 0.42;
        paint_icon(p, Rect::from_min_size(pos2(x, rect.center().y - s / 2.0), Vec2::splat(s)), icon, fg);
        x += icon_w;
    }
    p.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, fg);
    if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp }
}

/// Selectable pill, for tabs and filters.
pub fn chip(ui: &mut Ui, text: &str, selected: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), semibold(13.0), Color32::PLACEHOLDER);
    let size = vec2(galley.size().x + 28.0, 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let (bg, fg) = if selected {
        (TEXT, BG)
    } else {
        (Color32::from_white_alpha(14).lerp_to_gamma(Color32::from_white_alpha(32), t), SOFT)
    };
    ui.painter().rect_filled(rect, 16, bg);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Small non-interactive label on a tinted background.
pub fn badge(ui: &mut Ui, text: &str, color: Color32) -> Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(6)
        .inner_margin(Margin::symmetric(7, 2))
        .show(ui, |ui| ui.label(RichText::new(text).font(semibold(11.5)).color(color)))
        .response
}

pub fn section_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(bold(19.0)));
}

pub fn page_title(ui: &mut Ui, title: &str, sub: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).font(bold(28.0)));
    if !sub.is_empty() {
        ui.label(muted(sub, 13.5));
    }
    ui.add_space(10.0);
}

/// Centered icon, title and hint for empty or failed pages.
pub fn empty_state(ui: &mut Ui, icon: Icon, title: &str, sub: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        let (rect, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 36.0, PANEL2);
        paint_icon(ui.painter(), Rect::from_center_size(rect.center(), vec2(32.0, 32.0)), icon, MUTED);
        ui.add_space(10.0);
        ui.label(strong(title, 17.0));
        if !sub.is_empty() {
            ui.label(muted(sub, 13.5));
        }
    });
}

/// Error text with a retry button; returns true when retry is clicked.
pub fn error_state(ui: &mut Ui, err: &str) -> bool {
    let mut retry = false;
    ui.vertical_centered(|ui| {
        ui.add_space(40.0);
        ui.label(strong("Something went wrong", 17.0));
        ui.label(RichText::new(err).size(13.0).color(ERR));
        ui.add_space(6.0);
        retry = button(ui, Kind::Secondary, None, "Try again").clicked();
    });
    retry
}

/// Text wrapped to at most `rows` lines, ending in "…".
pub fn clamped(ui: &mut Ui, text: &str, size: f32, color: Color32, rows: usize) {
    clamped_font(ui, text, FontId::proportional(size), color, rows);
}

pub fn clamped_font(ui: &mut Ui, text: &str, font: FontId, color: Color32, rows: usize) {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font, color, ui.available_width());
    job.wrap.max_rows = rows;
    job.wrap.break_anywhere = false;
    ui.label(job);
}

/// Thin progress bar along the bottom of `rect`.
pub fn progress_bar(p: &Painter, rect: Rect, fraction: f32) {
    let track = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 4.0), rect.right_bottom());
    p.rect_filled(track, 2, Color32::from_black_alpha(160));
    p.rect_filled(track.with_max_x(track.left() + track.width() * fraction.clamp(0.0, 1.0)), 2, ACCENT_HI);
}
