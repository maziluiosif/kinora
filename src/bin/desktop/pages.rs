//! Board, Discover, Search, Library and Addons pages, and the poster cards they share.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, CursorIcon, Frame, Id, Image, Key, Label, Layout, Margin, Rect, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, TextEdit, Ui, UiBuilder, Vec2, pos2, vec2,
};
use egui::scroll_area::ScrollBarVisibility;
use egui::style::ScrollAnimation;
use kinora::CatalogRef;
use kinora::addons::{Addon, Resource};
use kinora::library::{LibItem, LibUpdate};

use crate::kit::*;
use crate::model::{MetaPreview, SearchRow, parse_metas};
use crate::task::Task;
use crate::{Env, HEADER_H, Route};

const CARD_W: f32 = 150.0;
const GAP: f32 = 16.0;

// ---- Poster cards ----

/// Everything a poster card shows.
pub struct Card<'a> {
    pub kind: &'a str,
    pub id: &'a str,
    pub name: &'a str,
    pub poster: Option<&'a str>,
    pub sub: String,
    pub rating: Option<&'a str>,
    /// Playback progress of the last watched video.
    pub progress: Option<f32>,
    /// Shows a remove button on hover (continue watching).
    pub dismissable: bool,
}

impl<'a> Card<'a> {
    pub fn meta(m: &'a MetaPreview) -> Self {
        let sub = [m.release_info.as_deref(), Some(kind_name(&m.kind, false))]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        Card {
            kind: &m.kind,
            id: &m.id,
            name: &m.name,
            poster: m.poster.as_deref(),
            sub,
            rating: m.imdb_rating.as_deref(),
            progress: None,
            dismissable: false,
        }
    }

    pub fn lib(i: &'a LibItem) -> Self {
        Card {
            kind: &i.kind,
            id: &i.id,
            name: &i.name,
            poster: i.poster.as_deref(),
            sub: kind_name(&i.kind, false).to_string(),
            rating: None,
            progress: (i.progress() > 0.0).then(|| i.progress()),
            dismissable: false,
        }
    }

    /// A continue-watching card: episode code and time left.
    pub fn resume(i: &'a LibItem) -> Self {
        let mut parts = Vec::new();
        if let Some(code) = i.last_video.as_deref().and_then(episode_code) {
            parts.push(code);
        }
        if i.duration > 0.0 {
            let left = ((i.duration - i.time_offset) / 60.0).round().max(1.0);
            parts.push(format!("{left} min left"));
        }
        let sub = if parts.is_empty() { kind_name(&i.kind, false).to_string() } else { parts.join(" · ") };
        Card { sub, dismissable: true, ..Card::lib(i) }
    }
}

/// "tt0903747:2:5" → "S2E5".
pub fn episode_code(video_id: &str) -> Option<String> {
    let mut parts = video_id.rsplit(':');
    let (e, s) = (parts.next()?.parse::<u32>().ok()?, parts.next()?.parse::<u32>().ok()?);
    Some(format!("S{s}E{e}"))
}

pub fn kind_name(kind: &str, plural: bool) -> &str {
    match (kind, plural) {
        ("movie", false) => "Movie",
        ("movie", true) => "Movies",
        ("series", _) => "Series",
        ("channel", false) => "Channel",
        ("channel", true) => "Channels",
        ("tv", _) => "TV",
        _ => kind,
    }
}

pub fn catalog_title(c: &CatalogRef) -> String {
    format!("{} {}", c.name, kind_name(&c.kind, true))
}

/// Draws a card; returns true when its remove button was clicked.
pub fn poster_card(ui: &mut Ui, env: &mut Env, c: &Card, width: f32) -> bool {
    let size = vec2(width, width * 1.5);
    let mut dismiss = false;
    ui.allocate_ui_with_layout(vec2(width, size.y + 52.0), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 2.0;
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        // The play and remove buttons sit on top of the poster. They are always registered (and
        // only drawn on hover): hovering them takes the hover from the poster, so tying them to
        // the poster's `hovered()` made them flicker in and out and swallowed the click.
        let full = rect.expand(4.0);
        let play_resp = ui
            .interact(Rect::from_center_size(full.center(), vec2(56.0, 56.0)), resp.id.with("play"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(if c.progress.is_some() { "Continue" } else { "Play" });
        let dismiss_rect = Rect::from_center_size(full.right_top() + vec2(-18.0, 18.0), vec2(26.0, 26.0));
        let dismiss_resp = c.dismissable.then(|| {
            ui.interact(dismiss_rect, resp.id.with("dismiss"), Sense::click()).on_hover_text("Remove from Continue watching")
        });
        let hovered = resp.contains_pointer();
        let hover = ui.ctx().animate_bool_with_time(resp.id, hovered, 0.15);
        let r = rect.expand(hover * 4.0);
        let p = ui.painter();
        if hover > 0.0 {
            p.rect_filled(r.translate(vec2(0.0, 6.0)), 14, Color32::from_black_alpha((110.0 * hover) as u8));
        }
        let loaded = c.poster.filter(|u| !u.is_empty()).is_some_and(|url| paint_cover(ui, url, r, 12, Color32::WHITE));
        if !loaded {
            if c.poster.is_some_and(|u| !u.is_empty() && !image_failed(ui, u, r.size())) {
                skeleton(ui, r, 12);
            } else {
                p.rect_filled(r, 12, PANEL2);
                let galley = p.layout(c.name.to_string(), bold(17.0), MUTED, r.width() - 24.0);
                p.galley(r.center() - galley.size() / 2.0, galley, MUTED);
            }
        }
        if let Some(rating) = c.rating.filter(|r| !r.is_empty()) {
            let galley = p.layout_no_wrap(rating.to_string(), semibold(11.5), Color32::WHITE);
            let chip = Rect::from_min_size(r.min + vec2(8.0, 8.0), vec2(galley.size().x + 26.0, 20.0));
            p.rect_filled(chip, 6, Color32::from_black_alpha(170));
            paint_icon(p, Rect::from_center_size(chip.left_center() + vec2(11.0, 0.0), vec2(11.0, 11.0)), Icon::Star, GOLD);
            p.galley(pos2(chip.left() + 19.0, chip.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
        }
        // The play button starts playback; the rest of the poster opens the details.
        if hover > 0.0 {
            let big = ui.ctx().animate_bool_with_time(play_resp.id, play_resp.hovered(), 0.12);
            p.rect_filled(r, 12, Color32::from_black_alpha((70.0 * hover) as u8));
            p.circle_filled(r.center(), 24.0 + 4.0 * big, ACCENT.lerp_to_gamma(ACCENT_HI, big).gamma_multiply(hover));
            paint_icon(p, Rect::from_center_size(r.center() + vec2(2.0, 0.0), vec2(20.0, 20.0)), Icon::Play, Color32::WHITE.gamma_multiply(hover));
            p.rect_stroke(r, 12, Stroke::new(2.0, ACCENT_HI.gamma_multiply(hover)), StrokeKind::Inside);
        }
        let play = play_resp.clicked();
        if let Some(f) = c.progress {
            progress_bar(&p.with_clip_rect(r.shrink2(vec2(6.0, 0.0))), r.shrink2(vec2(10.0, 6.0)), f);
        }
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        let clicked = resp.clicked();
        if let Some(xr) = dismiss_resp {
            if hover > 0.0 {
                let bg = if xr.hovered() { ERR } else { Color32::from_black_alpha(190) };
                p.circle_filled(dismiss_rect.center(), 13.0, bg.gamma_multiply(hover));
                paint_icon(p, dismiss_rect.shrink(7.0), Icon::Close, Color32::WHITE.gamma_multiply(hover));
            }
            dismiss = xr.clicked();
        }
        ui.add_space(6.0);
        ui.add(Label::new(RichText::new(c.name).font(semibold(13.5))).truncate());
        ui.add(Label::new(muted(&c.sub, 12.0)).truncate());
        if clicked || play {
            env.go(Route::Detail { kind: c.kind.into(), id: c.id.into(), play });
        }
    });
    dismiss
}

fn skeleton_card(ui: &mut Ui, width: f32) {
    ui.allocate_ui_with_layout(vec2(width, width * 1.5 + 52.0), Layout::top_down(Align::Min), |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(width, width * 1.5), Sense::hover());
        skeleton(ui, rect, 12);
        ui.add_space(8.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width * 0.7, 12.0), Sense::hover());
        skeleton(ui, rect, 4);
    });
}

/// A horizontally scrolling row with arrow buttons on its edges while hovered.
pub fn scroll_row(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, add: impl FnOnce(&mut Ui)) {
    let id = ui.make_persistent_id(salt);
    let pending = ui.data_mut(|d| d.remove_temp::<f32>(id)).unwrap_or(0.0);
    let out = ScrollArea::horizontal()
        .id_salt(id)
        .scroll_bar_visibility(ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            if pending != 0.0 {
                ui.scroll_with_delta_animation(vec2(pending, 0.0), ScrollAnimation::duration(0.35));
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                ui.add_space(4.0);
                add(ui);
                ui.add_space(4.0);
            });
        });
    let rect = out.inner_rect;
    let off = out.state.offset.x;
    let max = (out.content_size.x - rect.width()).max(0.0);
    if !ui.rect_contains_pointer(rect) || max <= 0.0 {
        return;
    }
    let poster_mid = rect.top() + (rect.height() - 52.0) / 2.0;
    let arrow = |x: f32, icon: Icon, dir: f32, salt: &str| {
        let r = Rect::from_center_size(pos2(x, poster_mid), vec2(44.0, 44.0));
        let resp = ui.interact(r, id.with(salt), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        let p = ui.painter();
        p.circle_filled(r.center(), 22.0, if resp.hovered() { TEXT } else { Color32::from_rgba_unmultiplied(20, 18, 28, 225) });
        p.circle_stroke(r.center(), 22.0, Stroke::new(1.0, Color32::from_white_alpha(40)));
        paint_icon(p, r.shrink(12.0), icon, if resp.hovered() { BG } else { TEXT });
        if resp.clicked() {
            ui.data_mut(|d| d.insert_temp(id, dir * rect.width() * 0.8));
            ui.ctx().request_repaint();
        }
    };
    if off > 1.0 {
        arrow(rect.left() + 28.0, Icon::ChevronLeft, 1.0, "left");
    }
    if off < max - 1.0 {
        arrow(rect.right() - 28.0, Icon::ChevronRight, -1.0, "right");
    }
}

fn meta_row(ui: &mut Ui, env: &mut Env, salt: impl std::hash::Hash + std::fmt::Debug, metas: &[MetaPreview]) {
    scroll_row(ui, salt, |ui| {
        for m in metas.iter().take(24) {
            poster_card(ui, env, &Card::meta(m), CARD_W);
        }
    });
}

fn skeleton_row(ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        ui.add_space(4.0);
        let n = (ui.available_width() / (CARD_W + GAP)).ceil() as usize;
        for _ in 0..n {
            skeleton_card(ui, CARD_W);
        }
    });
}

/// Cards in as many even columns as fit.
pub fn poster_grid<'a>(ui: &mut Ui, env: &mut Env, cards: impl IntoIterator<Item = Card<'a>>) {
    let avail = ui.available_width() - 12.0;
    let cols = ((avail + GAP) / (CARD_W + GAP)).floor().max(2.0);
    let width = ((avail - GAP * (cols - 1.0)) / cols).min(220.0);
    let cards: Vec<Card> = cards.into_iter().collect();
    for chunk in cards.chunks(cols as usize) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for c in chunk {
                poster_card(ui, env, c, width);
            }
        });
        ui.add_space(GAP / 2.0);
    }
}

fn skeleton_grid(ui: &mut Ui, rows: usize) {
    let avail = ui.available_width() - 12.0;
    let cols = ((avail + GAP) / (CARD_W + GAP)).floor().max(2.0);
    let width = ((avail - GAP * (cols - 1.0)) / cols).min(220.0);
    for _ in 0..rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for _ in 0..cols as usize {
                skeleton_card(ui, width);
            }
        });
        ui.add_space(GAP / 2.0);
    }
}

/// Section title with a muted caption and an optional "See all" link.
fn row_header(ui: &mut Ui, title: &str, caption: &str, see_all: bool) -> bool {
    ui.add_space(22.0);
    let mut clicked = false;
    ui.horizontal(|ui| {
        section_title(ui, title);
        if !caption.is_empty() {
            ui.label(muted(caption, 12.5));
        }
        if see_all {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(14.0);
                clicked = button_sized(ui, Kind::Ghost, None, "See all  ›", 30.0).clicked();
            });
        }
    });
    ui.add_space(4.0);
    clicked
}

fn fetch_catalog(env: &Env, c: &CatalogRef, skip: usize) -> Task<Vec<MetaPreview>> {
    let (addon, kind, id) = (c.addon.clone(), c.kind.clone(), c.id.clone());
    env.spawn(move |core| async move {
        let extra = if skip > 0 { vec![("skip".to_string(), skip.to_string())] } else { Vec::new() };
        let v = core.catalog(&addon, &kind, &id, &extra).await?;
        Ok(parse_metas(v["metas"].clone()))
    })
}

fn lib_update(m: &MetaPreview, saved: bool) -> LibUpdate {
    LibUpdate {
        id: m.id.clone(),
        kind: m.kind.clone(),
        name: m.name.clone(),
        poster: m.poster.clone(),
        saved: Some(saved),
        video: None,
        stream: None,
    }
}

// ---- Board ----

const HERO_EVERY: Duration = Duration::from_secs(9);

pub struct Board {
    rows: Vec<(CatalogRef, Task<Vec<MetaPreview>>)>,
    hero: usize,
    hero_since: Instant,
}

impl Board {
    pub fn new(env: &Env) -> Self {
        let cats = env.rt.block_on(env.core.catalogs());
        let rows = cats.into_iter().filter(|c| c.board).map(|c| {
            let t = fetch_catalog(env, &c, 0);
            (c, t)
        });
        Board { rows: rows.collect(), hero: 0, hero_since: Instant::now() }
    }

    pub fn ui(&mut self, ui: &mut Ui, env: &mut Env) {
        let out = ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if !self.hero_ui(ui, env) {
                ui.add_space(HEADER_H + 6.0);
            }
            Frame::new().inner_margin(Margin { left: 24, right: 28, top: 0, bottom: 0 }).show(ui, |ui| self.rows_ui(ui, env));
        });
        // The header stays see-through over the hero and fills in once the rows scroll under it.
        env.header_solid = ((out.state.offset.y - 40.0) / 120.0).clamp(0.0, 1.0);
    }

    fn rows_ui(&mut self, ui: &mut Ui, env: &mut Env) {
        {
            let mut cw: Vec<LibItem> = env.library.iter().filter(|i| i.last_video.is_some()).cloned().collect();
            cw.sort_by_key(|i| std::cmp::Reverse(i.last_watched));
            if !cw.is_empty() {
                row_header(ui, "Continue watching", "", false);
                let mut dismissed = None;
                scroll_row(ui, "cw", |ui| {
                    for i in &cw {
                        if poster_card(ui, env, &Card::resume(i), CARD_W) {
                            dismissed = Some((i.id.clone(), i.name.clone()));
                        }
                    }
                });
                if let Some((id, name)) = dismissed {
                    if let Err(e) = env.core.library.dismiss(&id) {
                        tracing::warn!("library update failed: {e:#}");
                    }
                    env.toast(format!("Removed “{name}” from Continue watching"));
                }
            }
            if self.rows.is_empty() {
                empty_state(ui, Icon::Addons, "No catalogs yet", "Install an addon from the Addons page to fill your board.");
            }
            let mut retry = None;
            for (i, (cat, task)) in self.rows.iter_mut().enumerate() {
                if row_header(ui, &catalog_title(cat), &cat.addon_name, true) {
                    env.go(Route::Discover(Some(cat.clone())));
                }
                match task.get() {
                    None => skeleton_row(ui),
                    Some(Err(e)) => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("Couldn't load this catalog: {e}")).color(ERR));
                            if button_sized(ui, Kind::Secondary, None, "Retry", 30.0).clicked() {
                                retry = Some(i);
                            }
                        });
                    }
                    Some(Ok(metas)) if metas.is_empty() => {
                        ui.label(muted("Nothing here yet.", 13.0));
                    }
                    Some(Ok(metas)) => meta_row(ui, env, ("row", i), metas),
                }
            }
            if let Some(i) = retry {
                self.rows[i].1 = fetch_catalog(env, &self.rows[i].0, 0);
            }
            ui.add_space(32.0);
        }
    }

    /// The rotating spotlight of the first catalog's titles that have artwork, edge to edge and
    /// up under the header. Returns whether it took any space.
    fn hero_ui(&mut self, ui: &mut Ui, env: &mut Env) -> bool {
        let Some((_, task)) = self.rows.first_mut() else { return false };
        let width = ui.available_width();
        let height = (width * 0.42).clamp(380.0, 560.0);
        let picks: Vec<MetaPreview> = match task.get() {
            None => {
                let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
                skeleton(ui, rect, 0);
                return true;
            }
            Some(Ok(metas)) => metas.iter().filter(|m| m.background.is_some()).take(6).cloned().collect(),
            Some(Err(_)) => Vec::new(),
        };
        if picks.is_empty() {
            return false;
        }
        let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        if self.hero_since.elapsed() >= HERO_EVERY && !resp.hovered() {
            self.hero = (self.hero + 1) % picks.len();
            self.hero_since = Instant::now();
        }
        ui.ctx().request_repaint_after(HERO_EVERY.saturating_sub(self.hero_since.elapsed()));
        let m = &picks[self.hero % picks.len()];
        let fade = (self.hero_since.elapsed().as_secs_f32() / 0.6).min(1.0);
        if fade < 1.0 {
            ui.ctx().request_repaint();
        }

        let p = ui.painter_at(rect);
        p.rect_filled(rect, 0, PANEL);
        if let Some(bg) = &m.background {
            paint_cover(ui, bg, rect, 0, Color32::WHITE.gamma_multiply(fade));
        }
        gradient_h(&p, rect.with_max_x(rect.left() + rect.width() * 0.7), BG.gamma_multiply(0.95), Color32::TRANSPARENT);
        // Fades into the page at the bottom, so the first row sits on the art's tail.
        gradient_v(&p, rect.with_min_y(rect.top() + rect.height() * 0.45), Color32::TRANSPARENT, BG);

        let inner =
            Rect::from_min_max(rect.min + vec2(44.0, HEADER_H + 24.0), pos2(rect.left() + width * 0.5, rect.bottom() - 36.0));
        ui.scope_builder(UiBuilder::new().max_rect(inner).layout(Layout::bottom_up(Align::Min)), |ui| {
            ui.multiply_opacity(fade.max(0.3));
            let saved = env.lib_item(&m.id).is_some_and(|i| i.saved);
            ui.horizontal(|ui| {
                if button(ui, Kind::Primary, Some(Icon::Play), "Watch now").on_hover_text("Starts the best source right away").clicked() {
                    env.go(Route::Detail { kind: m.kind.clone(), id: m.id.clone(), play: true });
                }
                let (icon, text) = if saved { (Icon::Check, "In library") } else { (Icon::Plus, "My library") };
                if button(ui, Kind::Secondary, Some(icon), text).clicked() {
                    env.toggle_saved(lib_update(m, !saved));
                }
                if button(ui, Kind::Ghost, None, "Details").clicked() {
                    env.go(Route::Detail { kind: m.kind.clone(), id: m.id.clone(), play: false });
                }
            });
            ui.add_space(14.0);
            if let Some(d) = &m.description {
                clamped(ui, d, 14.5, SOFT, 3);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if let Some(r) = &m.imdb_rating {
                    badge(ui, &format!("IMDb {r}"), GOLD);
                }
                let line = [m.release_info.clone(), Some(m.genres.iter().take(3).cloned().collect::<Vec<_>>().join(", "))]
                    .into_iter()
                    .flatten()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("  ·  ");
                ui.label(RichText::new(line).size(13.5).color(SOFT));
            });
            ui.add_space(6.0);
            match m.logo.as_deref().filter(|l| !l.is_empty()) {
                Some(l) => {
                    ui.add(Image::new(l).max_height(100.0).max_width(360.0).fit_to_original_size(1.0));
                }
                None => {
                    ui.add(Label::new(RichText::new(&m.name).font(bold(40.0))).wrap());
                }
            }
        });

        // Page dots.
        let n = picks.len();
        let dots_w = n as f32 * 18.0;
        for i in 0..n {
            let c = pos2(rect.right() - 40.0 - dots_w + i as f32 * 18.0 + 9.0, rect.bottom() - 40.0);
            let r = Rect::from_center_size(c, vec2(16.0, 16.0));
            let dot = ui.interact(r, Id::new(("hero-dot", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            let active = i == self.hero % n;
            let size = if active { vec2(14.0, 6.0) } else { vec2(6.0, 6.0) };
            let color = if active { TEXT } else if dot.hovered() { SOFT } else { Color32::from_white_alpha(80) };
            ui.painter().rect_filled(Rect::from_center_size(c, size), 3, color);
            if dot.clicked() {
                self.hero = i;
                self.hero_since = Instant::now();
            }
        }
        true
    }
}

// ---- Discover ----

pub struct Discover {
    cats: Vec<CatalogRef>,
    sel: usize,
    items: Vec<MetaPreview>,
    loading: Option<Task<Vec<MetaPreview>>>,
    done: bool,
    error: Option<String>,
}

impl Discover {
    pub fn new(env: &Env, cat: Option<CatalogRef>) -> Self {
        let cats: Vec<_> = env.rt.block_on(env.core.catalogs()).into_iter().filter(|c| c.board).collect();
        let sel = cat
            .and_then(|c| cats.iter().position(|x| x.addon == c.addon && x.kind == c.kind && x.id == c.id))
            .unwrap_or(0);
        let mut d = Discover { cats, sel, items: Vec::new(), loading: None, done: false, error: None };
        d.load(env);
        d
    }

    fn load(&mut self, env: &Env) {
        if let Some(c) = self.cats.get(self.sel) {
            self.loading = Some(fetch_catalog(env, c, self.items.len()));
        }
    }

    fn select(&mut self, env: &Env, i: usize) {
        if i != self.sel {
            self.sel = i;
            self.items.clear();
            self.done = false;
            self.error = None;
            self.load(env);
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, env: &mut Env) {
        if let Some(t) = &mut self.loading
            && let Some(res) = t.get()
        {
            match res {
                Ok(v) => {
                    let before = self.items.len();
                    for m in v.drain(..) {
                        if !self.items.iter().any(|x| x.id == m.id) {
                            self.items.push(m);
                        }
                    }
                    self.done = self.items.len() == before;
                }
                Err(e) => self.error = Some(e.clone()),
            }
            self.loading = None;
        }

        if self.cats.is_empty() {
            empty_state(ui, Icon::Compass, "Nothing to discover", "Install an addon with catalogs from the Addons page.");
            return;
        }

        // One filter row: the type, then the catalogs of that type.
        let mut kinds: Vec<&str> = Vec::new();
        for c in &self.cats {
            if !kinds.contains(&c.kind.as_str()) {
                kinds.push(&c.kind);
            }
        }
        let kind = self.cats[self.sel].kind.clone();
        let mut pick = None;
        let addons: Vec<&str> = self.cats.iter().filter(|c| c.kind == kind).map(|c| c.addon.as_str()).collect();
        let many_addons = addons.iter().any(|a| *a != addons[0]);
        ui.horizontal_wrapped(|ui| {
            let names: Vec<&str> = kinds.iter().map(|k| kind_name(k, true)).collect();
            if let Some(k) = segmented(ui, &names, kinds.iter().position(|k| *k == kind)) {
                pick = self.cats.iter().position(|c| c.kind == kinds[k]);
            }
            divider(ui);
            for (i, c) in self.cats.iter().enumerate().filter(|(_, c)| c.kind == kind) {
                let label = if many_addons { format!("{} · {}", c.name, c.addon_name) } else { c.name.clone() };
                if chip(ui, &label, i == self.sel).clicked() {
                    pick = Some(i);
                }
            }
        });
        if let Some(i) = pick {
            self.select(env, i);
        }
        ui.add_space(12.0);

        let out = ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if self.items.is_empty() && self.loading.is_some() {
                skeleton_grid(ui, 3);
            } else {
                poster_grid(ui, env, self.items.iter().map(Card::meta));
            }
            ui.add_space(8.0);
            if let Some(e) = self.error.clone() {
                if error_state(ui, &e) {
                    self.error = None;
                    self.load(env);
                }
            } else if self.loading.is_some() && !self.items.is_empty() {
                ui.vertical_centered(|ui| ui.spinner());
            } else if self.items.is_empty() && self.done {
                empty_state(ui, Icon::Compass, "This catalog is empty", "");
            }
            ui.add_space(32.0);
        });
        // Infinite scroll: fetch the next page when close to the bottom.
        let near_end = out.state.offset.y + out.inner_rect.height() > out.content_size.y - 700.0;
        if near_end && !self.done && self.loading.is_none() && self.error.is_none() && !self.items.is_empty() {
            self.load(env);
        }
    }
}

// ---- Search ----

pub struct Search {
    pub query: String,
    rows: Task<Vec<SearchRow>>,
}

impl Search {
    pub fn new(env: &Env, query: String) -> Self {
        let q = query.clone();
        let rows = env.spawn(move |core| async move {
            let mut rows: Vec<SearchRow> =
                core.search(&q).await.into_iter().filter_map(|r| serde_json::from_value(r).ok()).collect();
            // Movies, then series, then everything else; addon order otherwise.
            rows.sort_by_key(|r| match r.metas.first().map(|m| m.kind.as_str()) {
                Some("movie") => 0,
                Some("series") => 1,
                _ => 2,
            });
            Ok(rows)
        });
        Search { query, rows }
    }

    pub fn ui(&mut self, ui: &mut Ui, env: &mut Env) {
        let query = self.query.clone();
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.set_width(ui.available_width() - 16.0);
            match self.rows.get() {
                None => {
                    page_title(ui, &format!("Searching for “{query}”…"), "");
                    skeleton_row(ui);
                    ui.add_space(24.0);
                    skeleton_row(ui);
                }
                Some(Err(e)) => {
                    let e = e.clone();
                    if error_state(ui, &e) {
                        *self = Search::new(env, query);
                    }
                }
                Some(Ok(rows)) => {
                    let total: usize = rows.iter().map(|r| r.metas.len()).sum();
                    let sub = match total {
                        0 => String::new(),
                        1 => "1 title".into(),
                        n => format!("{n} titles"),
                    };
                    page_title(ui, &format!("Results for “{query}”"), &sub);
                    if total == 0 {
                        empty_state(ui, Icon::Search, "No results", "Check the spelling or try another title.");
                    }
                    for (i, r) in rows.iter().enumerate().filter(|(_, r)| !r.metas.is_empty()) {
                        let (name, kind) = r.name.rsplit_once(" - ").unwrap_or((&r.name, ""));
                        let title = if kind.is_empty() { name.to_string() } else { kind_name(kind, true).to_string() };
                        row_header(ui, &title, &format!("{} · {}", r.addon, r.metas.len()), false);
                        meta_row(ui, env, ("search", i), &r.metas);
                    }
                }
            }
            ui.add_space(32.0);
        });
    }
}

// ---- Library ----

#[derive(Default)]
pub struct LibraryPage {
    kind: Option<String>,
    by_name: bool,
}

impl LibraryPage {
    pub fn ui(&mut self, ui: &mut Ui, env: &mut Env) {
        let mut items: Vec<LibItem> = env.library.iter().filter(|i| i.saved).cloned().collect();
        if items.is_empty() {
            empty_state(ui, Icon::Bookmark, "Your library is empty", "Save titles with “Add to library” to find them here.");
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                if button(ui, Kind::Primary, Some(Icon::Compass), "Discover titles").clicked() {
                    env.go(Route::Discover(None));
                }
            });
            return;
        }
        let mut kinds: Vec<String> = items.iter().map(|i| i.kind.clone()).collect();
        kinds.sort();
        kinds.dedup();
        if let Some(k) = &self.kind {
            items.retain(|i| &i.kind == k);
        }
        ui.horizontal(|ui| {
            let mut names = vec!["All"];
            names.extend(kinds.iter().map(|k| kind_name(k, true)));
            let current = match &self.kind {
                None => Some(0),
                Some(k) => kinds.iter().position(|x| x == k).map(|i| i + 1),
            };
            if let Some(i) = segmented(ui, &names, current) {
                self.kind = if i == 0 { None } else { Some(kinds[i - 1].clone()) };
            }
            let count = match items.len() {
                1 => "1 title".to_string(),
                n => format!("{n} titles"),
            };
            ui.label(muted(count, 13.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(14.0);
                // Laid out right to left, so the options go in reverse.
                if let Some(i) = segmented(ui, &["A–Z", "Recent"], Some(if self.by_name { 0 } else { 1 })) {
                    self.by_name = i == 0;
                }
            });
        });
        ui.add_space(12.0);
        if self.by_name {
            items.sort_by_key(|i| i.name.to_lowercase());
        } else {
            items.sort_by_key(|i| std::cmp::Reverse(i.last_watched));
        }
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            poster_grid(ui, env, items.iter().map(Card::lib));
            ui.add_space(32.0);
        });
    }
}

// ---- Addons ----

pub struct AddonsPage {
    list: Vec<Addon>,
    url: String,
    installing: Option<Task<Addon>>,
    error: Option<String>,
    /// Addon whose Uninstall was clicked once, waiting for the confirmation click.
    confirm: Option<(String, Instant)>,
}

impl AddonsPage {
    pub fn new(env: &Env) -> Self {
        let list = env.rt.block_on(env.core.addons.list());
        AddonsPage { list, url: String::new(), installing: None, error: None, confirm: None }
    }

    pub fn ui(&mut self, ui: &mut Ui, env: &mut Env) {
        if let Some(t) = &mut self.installing
            && let Some(res) = t.get()
        {
            match res {
                Ok(a) => {
                    env.toast(format!("Installed {}", a.manifest.name));
                    self.url.clear();
                }
                Err(e) => self.error = Some(e.clone()),
            }
            self.installing = None;
            self.list = env.rt.block_on(env.core.addons.list());
        }
        if self.confirm.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(4)) {
            self.confirm = None;
        }

        Frame::new().fill(PANEL).stroke(Stroke::new(1.0, LINE)).corner_radius(14).inner_margin(16).show(ui, |ui| {
            ui.set_width(ui.available_width() - 16.0);
            ui.label(strong("Install an addon", 15.0));
            ui.label(muted("Addons provide catalogs, metadata and streams. Paste one's manifest URL (it ends in /manifest.json).", 12.5));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let busy = self.installing.is_some();
                let edit = ui.add(
                    TextEdit::singleline(&mut self.url)
                        .hint_text("https://…/manifest.json")
                        .desired_width(ui.available_width() - 130.0)
                        .margin(Margin::symmetric(12, 9)),
                );
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let ready = !busy && !self.url.trim().is_empty();
                let clicked = ui.add_enabled_ui(ready, |ui| button(ui, Kind::Primary, Some(Icon::Plus), "Install")).inner.clicked();
                if ready && (clicked || enter) {
                    let url = self.url.trim().to_string();
                    self.installing = Some(env.spawn(move |core| async move { core.addons.install(&url).await }));
                    self.error = None;
                }
                if busy {
                    ui.spinner();
                }
            });
            if let Some(e) = &self.error {
                ui.label(RichText::new(e.as_str()).color(ERR));
            }
        });
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            section_title(ui, "Installed");
            ui.label(muted(self.list.len().to_string(), 13.0));
        });
        ui.add_space(4.0);

        let mut remove = None;
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.set_width(ui.available_width() - 16.0);
            for a in &self.list {
                let confirming = self.confirm.as_ref().is_some_and(|(id, _)| *id == a.manifest.id);
                match addon_card(ui, a, confirming) {
                    Some(AddonAction::Uninstall) if confirming => remove = Some(a.manifest.id.clone()),
                    Some(AddonAction::Uninstall) => self.confirm = Some((a.manifest.id.clone(), Instant::now())),
                    Some(AddonAction::Copy) => {
                        ui.ctx().copy_text(a.url.clone());
                        env.toast("Manifest URL copied");
                    }
                    None => {}
                }
                ui.add_space(8.0);
            }
            ui.add_space(24.0);
        });
        if let Some(id) = remove {
            self.confirm = None;
            match env.rt.block_on(env.core.addons.uninstall(&id)) {
                Ok(_) => env.toast("Addon removed"),
                Err(e) => self.error = Some(format!("{e:#}")),
            }
            self.list = env.rt.block_on(env.core.addons.list());
        }
    }
}

enum AddonAction {
    Uninstall,
    Copy,
}

fn addon_card(ui: &mut Ui, a: &Addon, confirming: bool) -> Option<AddonAction> {
    let mut action = None;
    Frame::new().fill(PANEL).stroke(Stroke::new(1.0, LINE)).corner_radius(14).inner_margin(16).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
            let logo = a.manifest.logo.as_deref().filter(|l| !l.is_empty());
            if !logo.is_some_and(|l| paint_cover(ui, l, rect, 12, Color32::WHITE)) {
                ui.painter().rect_filled(rect, 12, ACCENT.gamma_multiply(0.3));
                let initial = a.manifest.name.chars().next().unwrap_or('?').to_uppercase().to_string();
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initial, bold(24.0), ACCENT2);
            }
            ui.add_space(6.0);
            let text_w = ui.available_width() - 230.0;
            ui.vertical(|ui| {
                ui.set_width(text_w);
                ui.horizontal(|ui| {
                    ui.label(strong(&a.manifest.name, 16.0));
                    if !a.manifest.version.is_empty() {
                        badge(ui, &format!("v{}", a.manifest.version), MUTED);
                    }
                });
                if let Some(d) = a.manifest.description.as_deref().filter(|d| !d.is_empty()) {
                    clamped(ui, d, 13.0, SOFT, 2);
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for r in &a.manifest.resources {
                        let name = match r {
                            Resource::Name(n) | Resource::Full { name: n, .. } => n,
                        };
                        badge(ui, name, ACCENT2);
                    }
                    for t in &a.manifest.types {
                        badge(ui, kind_name(t, true), TEAL);
                    }
                });
            });
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                let text = if confirming { "Click to confirm" } else { "Uninstall" };
                let kind = if confirming { Kind::Danger } else { Kind::Secondary };
                if button_sized(ui, kind, None, text, 34.0).clicked() {
                    action = Some(AddonAction::Uninstall);
                }
                if icon_button(ui, Icon::Copy, 34.0, "Copy manifest URL").clicked() {
                    action = Some(AddonAction::Copy);
                }
            });
        });
    });
    action
}
