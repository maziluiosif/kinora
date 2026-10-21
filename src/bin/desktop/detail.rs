//! The details page: artwork and info on the left, episodes or streams on the right.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{
    self, Align, Color32, CursorIcon, Frame, Image, Label, Margin, Panel, Rect, RichText, ScrollArea, Sense, Stroke,
    TextEdit, Ui, pos2, vec2,
};
use kinora::library::{LibItem, LibUpdate};

use crate::kit::*;
use crate::model::{Meta, Stream, StreamInfo, Tier, Video, parse_streams};
use crate::pages::kind_name;
use crate::task::Task;
use crate::{Action, Env, PlayReq};

struct StreamList {
    video: Option<Video>,
    video_id: String,
    streams: Task<Vec<Stream>>,
    filter: String,
    tier: Option<Tier>,
    by_seeders: bool,
    /// Play the best stream as soon as the list arrives.
    autoplay: bool,
    /// Streams already played from this list; "Try another source" skips them.
    tried: HashSet<String>,
}

pub struct Detail {
    kind: String,
    id: String,
    meta: Task<Meta>,
    initialized: bool,
    season: Option<u32>,
    streams: Option<StreamList>,
    /// Scroll the episode list to the last watched episode on the next frame.
    reveal_last: bool,
    /// Start playing (or continue) once the metadata is in.
    play: bool,
}

impl Detail {
    pub fn new(env: &Env, kind: String, id: String, play: bool) -> Self {
        let meta = load_meta(env, &kind, &id);
        Detail { kind, id, meta, initialized: false, season: None, streams: None, reveal_last: true, play }
    }

    fn is_series(&self, meta: &Meta) -> bool {
        self.kind == "series" || meta.videos.len() > 1
    }

    fn open_streams(&mut self, env: &Env, video: Option<&Video>, video_id: String, autoplay: bool) {
        let (kind, vid) = (self.kind.clone(), video_id.clone());
        let streams = env.spawn(move |core| async move { Ok(parse_streams(core.streams(&kind, &vid).await)) });
        if let Some(v) = video {
            self.season = Some(v.season);
        }
        self.streams = Some(StreamList {
            video: video.cloned(),
            video_id,
            streams,
            filter: String::new(),
            tier: None,
            by_seeders: true,
            autoplay,
            tried: HashSet::new(),
        });
    }

    /// Plays the best stream not tried yet (after one failed). Returns false when none is left.
    pub fn play_next_source(&mut self, env: &mut Env) -> bool {
        let Some(Ok(meta)) = self.meta.get().map(|m| m.clone()) else { return false };
        let Some(list) = &mut self.streams else { return false };
        let Some(Ok(streams)) = list.streams.get() else { return false };
        let Some(s) = best_stream(streams, &list.tried, None).cloned() else { return false };
        list.tried.insert(s.key());
        let (video, video_id) = (list.video.clone(), list.video_id.clone());
        env.actions.push(play_action(&self.kind, &self.id, &meta, s, video.as_ref(), video_id));
        true
    }

    /// Shows the streams of `video_id` (used by the player's "Next episode").
    pub fn open_episode(&mut self, env: &Env, video_id: &str) {
        let video = match self.meta.get() {
            Some(Ok(meta)) => meta.videos.iter().find(|v| v.id == video_id).cloned(),
            _ => None,
        };
        if let Some(v) = video {
            self.open_streams(env, Some(&v), v.id.clone(), true);
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, env: &mut Env) {
        let meta = match self.meta.get() {
            Some(Ok(m)) => m.clone(),
            Some(Err(e)) => {
                let e = e.clone();
                ui.add_space(40.0);
                if error_state(ui, &e) {
                    self.meta = load_meta(env, &self.kind, &self.id);
                }
                return;
            }
            None => {
                loading_ui(ui);
                return;
            }
        };
        let lib = env.lib_item(&self.id).cloned();
        if !self.initialized {
            self.initialized = true;
            if self.is_series(&meta) && !meta.videos.is_empty() {
                let last = lib.as_ref().and_then(|l| l.last_video.as_deref());
                self.season = meta.videos.iter().find(|v| Some(v.id.as_str()) == last).map(|v| v.season);
                if self.play
                    && let Some((video, _)) = up_next(&meta, lib.as_ref())
                {
                    self.open_streams(env, Some(&video), video.id.clone(), true);
                }
            } else {
                let vid = meta.behavior_hints.default_video_id.clone().unwrap_or_else(|| self.id.clone());
                self.open_streams(env, None, vid, self.play);
            }
        }

        let full = ui.max_rect();
        backdrop(ui, full, &meta);

        let side_w = (full.width() * 0.36).clamp(380.0, 520.0);
        Panel::right("side")
            .resizable(false)
            .show_separator_line(false)
            .exact_size(side_w)
            .frame(
                Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(0x14, 0x12, 0x1c, 235))
                    .stroke(Stroke::new(1.0, Color32::from_white_alpha(16)))
                    .corner_radius(16)
                    .inner_margin(16)
                    .outer_margin(Margin { left: 0, right: 16, top: 4, bottom: 16 }),
            )
            .show(ui, |ui| self.side_ui(ui, env, &meta, lib.as_ref()));

        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            Frame::new().inner_margin(Margin { left: 40, right: 32, top: 0, bottom: 32 }).show(ui, |ui| {
                ui.set_max_width(ui.available_width().min(720.0));
                self.info_ui(ui, env, &meta, lib.as_ref(), full.height());
            });
        });
    }

    fn info_ui(&mut self, ui: &mut Ui, env: &mut Env, meta: &Meta, lib: Option<&LibItem>, height: f32) {
        ui.add_space((height * 0.22).clamp(60.0, 220.0));
        match meta.logo.as_deref().filter(|l| !l.is_empty()) {
            Some(l) => {
                ui.add(Image::new(l).max_height(120.0).max_width(420.0).fit_to_original_size(1.0)).on_hover_text(&meta.name);
            }
            None => {
                ui.add(Label::new(RichText::new(&meta.name).font(bold(44.0))).wrap());
            }
        }
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if let Some(r) = &meta.imdb_rating {
                badge(ui, &format!("IMDb {r}"), GOLD);
            }
            let facts: Vec<&str> = [meta.release_info.as_deref(), meta.runtime.as_deref(), Some(kind_name(&self.kind, false))]
                .into_iter()
                .flatten()
                .collect();
            ui.label(RichText::new(facts.join("  ·  ")).size(14.0).color(SOFT));
        });
        if !meta.genres.is_empty() {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for g in &meta.genres {
                    Frame::new().fill(Color32::from_white_alpha(16)).corner_radius(99).inner_margin(Margin::symmetric(10, 3)).show(
                        ui,
                        |ui| ui.label(RichText::new(g).size(12.5).color(SOFT)),
                    );
                }
            });
        }
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            self.play_button(ui, env, meta, lib);
            let saved = lib.is_some_and(|l| l.saved);
            let (icon, text) = if saved { (Icon::Check, "In library") } else { (Icon::Plus, "Add to library") };
            if button(ui, Kind::Secondary, Some(icon), text).clicked() {
                env.toggle_saved(lib_update(&self.kind, &self.id, meta, Some(!saved), None));
            }
            if let Some(yt) = meta.trailer_streams.iter().find_map(|t| t.yt_id.as_deref())
                && button(ui, Kind::Ghost, Some(Icon::External), "Trailer").on_hover_text("Opens YouTube").clicked()
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(format!("https://www.youtube.com/watch?v={yt}")));
            }
        });
        if let Some(d) = meta.description.as_deref().filter(|d| !d.is_empty()) {
            ui.add_space(20.0);
            ui.label(RichText::new(d).size(15.0).color(SOFT).line_height(Some(22.0)));
        }
        ui.add_space(14.0);
        for (label, names) in [("Starring", &meta.cast), ("Director", &meta.director)] {
            if names.is_empty() {
                continue;
            }
            ui.horizontal_top(|ui| {
                ui.add_sized([72.0, 18.0], Label::new(muted(label, 13.0)));
                let list: Vec<&str> = names.iter().take(8).map(String::as_str).collect();
                ui.add(Label::new(RichText::new(list.join(", ")).size(13.5).color(SOFT)).wrap());
            });
        }
    }

    /// Movies: plays the top stream. Series: opens the episode to continue (or the first one).
    fn play_button(&mut self, ui: &mut Ui, env: &mut Env, meta: &Meta, lib: Option<&LibItem>) {
        if self.is_series(meta) {
            let Some((video, label)) = up_next(meta, lib) else { return };
            if button(ui, Kind::Primary, Some(Icon::Play), &label)
                .on_hover_text("Starts the best source right away (1080p with the most seeders)")
                .clicked()
            {
                self.open_streams(env, Some(&video), video.id.clone(), true);
            }
            return;
        }
        let Some(list) = &mut self.streams else { return };
        let best = match list.streams.get() {
            Some(Ok(s)) => best_stream(s, &HashSet::new(), last_stream(lib, &list.video_id)).cloned(),
            _ => None,
        };
        let loading = list.streams.get().is_none();
        let label = if loading { "Finding streams…" } else if best.is_none() { "No streams" } else { "Play" };
        let resp = ui.add_enabled_ui(best.is_some(), |ui| button(ui, Kind::Primary, Some(Icon::Play), label)).inner;
        if let Some(s) = best
            && resp.on_hover_text("Plays the best-seeded 1080p stream (or the closest); pick another one on the right").clicked()
        {
            let video_id = list.video_id.clone();
            list.tried.insert(s.key());
            env.actions.push(play_action(&self.kind, &self.id, meta, s, None, video_id));
        }
    }

    fn side_ui(&mut self, ui: &mut Ui, env: &mut Env, meta: &Meta, lib: Option<&LibItem>) {
        if self.streams.is_some() {
            self.streams_ui(ui, env, meta);
        } else {
            self.episodes_ui(ui, env, meta, lib);
        }
    }

    fn streams_ui(&mut self, ui: &mut Ui, env: &mut Env, meta: &Meta) {
        let series = self.is_series(meta);
        let Some(list) = &mut self.streams else { return };
        let mut back = false;
        ui.horizontal(|ui| {
            if series && icon_button_with(ui, Icon::ChevronLeft, 32.0, "Back to episodes", PANEL2).clicked() {
                back = true;
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                match &list.video {
                    Some(v) => {
                        ui.add(Label::new(muted(format!("{} · {}", v.code(), meta.name), 12.0)).truncate());
                        ui.add(Label::new(strong(v.label(), 16.0)).truncate());
                    }
                    None => {
                        ui.label(strong("Streams", 17.0));
                    }
                }
            });
        });
        if back {
            self.streams = None;
            return;
        }
        ui.add_space(6.0);
        let video = list.video.clone();
        let video_id = list.video_id.clone();
        match list.streams.get() {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(muted("Asking your addons for streams…", 13.0));
                });
                for _ in 0..5 {
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
                    skeleton(ui, rect, 12);
                }
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e.as_str()).color(ERR));
            }
            Some(Ok(streams)) if streams.is_empty() => {
                empty_state(ui, Icon::Search, "No streams found", "None of your addons has a stream for this yet.");
            }
            Some(Ok(streams)) => {
                let last = last_stream(env.lib_item(&self.id), &video_id).map(String::from);
                if std::mem::take(&mut list.autoplay)
                    && let Some(s) = best_stream(streams, &list.tried, last.as_deref()).cloned()
                {
                    list.tried.insert(s.key());
                    env.actions.push(play_action(&self.kind, &self.id, meta, s, video.as_ref(), video_id.clone()));
                }
                let infos: Vec<StreamInfo> = streams.iter().map(Stream::info).collect();
                let mut tiers: Vec<Tier> = infos.iter().map(|i| i.tier).collect();
                tiers.sort();
                tiers.dedup();
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    if chip(ui, &format!("All {}", streams.len()), list.tier.is_none()).clicked() {
                        list.tier = None;
                    }
                    for t in &tiers {
                        let n = infos.iter().filter(|i| i.tier == *t).count();
                        if chip(ui, &format!("{} {n}", t.label()), list.tier == Some(*t)).clicked() {
                            list.tier = if list.tier == Some(*t) { None } else { Some(*t) };
                        }
                    }
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.add(
                        TextEdit::singleline(&mut list.filter)
                            .hint_text("Filter: x265, HDR, RO…")
                            .desired_width(ui.available_width() - 120.0)
                            .margin(Margin::symmetric(10, 7)),
                    );
                    let label = if list.by_seeders { "Most seeds" } else { "Addon order" };
                    if button_sized(ui, Kind::Ghost, None, label, 32.0).on_hover_text("Change the order").clicked() {
                        list.by_seeders = !list.by_seeders;
                    }
                });
                ui.add_space(4.0);
                let terms: Vec<String> = list.filter.to_lowercase().split_whitespace().map(String::from).collect();
                let mut order: Vec<usize> = (0..streams.len())
                    .filter(|&i| list.tier.is_none_or(|t| infos[i].tier == t))
                    .filter(|&i| {
                        let hay = streams[i].haystack();
                        terms.iter().all(|t| hay.contains(t.as_str()))
                    })
                    .collect();
                if list.by_seeders {
                    order.sort_by_key(|&i| std::cmp::Reverse(infos[i].seeders.unwrap_or(0)));
                }
                let many_addons = streams.iter().any(|s| s.addon != streams[0].addon);
                let mut play = None;
                ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    if order.is_empty() {
                        ui.add_space(12.0);
                        ui.label(muted("No stream matches the filter.", 13.0));
                    }
                    for &i in &order {
                        let addon = if many_addons { Some(streams[i].addon.as_str()) } else { None };
                        if stream_row(ui, i, &infos[i], addon) {
                            play = Some(streams[i].clone());
                        }
                    }
                });
                if let Some(s) = play {
                    list.tried.insert(s.key());
                    env.actions.push(play_action(&self.kind, &self.id, meta, s, video.as_ref(), video_id));
                }
            }
        }
    }

    fn episodes_ui(&mut self, ui: &mut Ui, env: &mut Env, meta: &Meta, lib: Option<&LibItem>) {
        let mut seasons: Vec<u32> = meta.videos.iter().map(|v| v.season).collect();
        seasons.sort_by_key(|s| (*s == 0, *s));
        seasons.dedup();
        let Some(&first) = seasons.first() else {
            empty_state(ui, Icon::Search, "No episodes", "");
            return;
        };
        let season = *self.season.get_or_insert(first);
        ui.label(strong("Episodes", 17.0));
        ui.add_space(4.0);
        ScrollArea::horizontal().id_salt("seasons").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(
            ui,
            |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for s in &seasons {
                        let name = if *s == 0 { "Specials".to_string() } else { format!("Season {s}") };
                        let resp = chip(ui, &name, *s == season);
                        if *s == season && self.reveal_last {
                            resp.scroll_to_me(Some(Align::Center));
                        }
                        if resp.clicked() {
                            self.season = Some(*s);
                        }
                    }
                });
            },
        );
        ui.add_space(6.0);
        let last = lib.and_then(|l| l.last_video.as_deref());
        let today = today();
        let mut picked = None;
        let reveal = std::mem::take(&mut self.reveal_last);
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for v in meta.videos.iter().filter(|v| v.season == season) {
                let is_last = Some(v.id.as_str()) == last;
                let state = EpisodeState {
                    last: is_last,
                    progress: lib.filter(|_| is_last).map(LibItem::progress).filter(|p| *p > 0.0),
                    watched: lib.is_some_and(|l| l.watched.contains(&v.id)),
                    upcoming: !v.date().is_empty() && v.date() > today.as_str(),
                };
                let resp = episode_row(ui, v, &state);
                if is_last && reveal {
                    resp.scroll_to_me(Some(Align::Center));
                }
                if resp.clicked() {
                    picked = Some(v.clone());
                }
            }
        });
        if let Some(v) = picked {
            self.open_streams(env, Some(&v), v.id.clone(), true);
        }
    }
}

fn load_meta(env: &Env, kind: &str, id: &str) -> Task<Meta> {
    let (k, i) = (kind.to_string(), id.to_string());
    env.spawn(move |core| async move {
        let v = core.meta(&k, &i).await?;
        let mut meta: Meta = serde_json::from_value(v["meta"].clone())?;
        meta.videos.sort_by_key(|v| (v.season, v.episode));
        Ok(meta)
    })
}

fn loading_ui(ui: &mut Ui) {
    let full = ui.max_rect();
    skeleton(ui, full.with_max_y(full.top() + full.height() * 0.55).shrink(16.0), 16);
    let left = full.left() + 40.0;
    for (i, w) in [380.0, 260.0, 520.0].into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(left, full.top() + full.height() * 0.62 + i as f32 * 34.0), vec2(w, 22.0));
        skeleton(ui, r, 6);
    }
}

/// Background art across the top of the page, fading into the page color.
fn backdrop(ui: &Ui, full: Rect, meta: &Meta) {
    let Some(bg) = meta.background.as_deref().filter(|b| !b.is_empty()) else { return };
    let h = (full.width() * 0.56).min(full.height() * 0.85);
    let rect = full.with_max_y(full.top() + h);
    paint_cover(ui, bg, rect, 0, Color32::WHITE.gamma_multiply(0.6));
    let p = ui.painter();
    // Overlap the image's bottom edge so no seam shows.
    gradient_v(p, Rect::from_min_max(pos2(rect.left(), rect.top() + h * 0.35), rect.right_bottom() - vec2(0.0, 2.0)), Color32::TRANSPARENT, BG);
    p.rect_filled(Rect::from_min_max(rect.left_bottom() - vec2(0.0, 3.0), rect.right_bottom() + vec2(0.0, 2.0)), 0, BG);
    gradient_h(p, rect.with_max_x(rect.left() + full.width() * 0.6), BG.gamma_multiply(0.9), Color32::TRANSPARENT);
    gradient_v(p, rect.with_max_y(rect.top() + 60.0), BG.gamma_multiply(0.7), Color32::TRANSPARENT);
}

/// The stream `video_id` was last played from, if it is the one being continued.
fn last_stream<'a>(lib: Option<&'a LibItem>, video_id: &str) -> Option<&'a str> {
    let lib = lib?;
    if lib.last_video.as_deref() == Some(video_id) { lib.last_stream.as_deref() } else { None }
}

/// The stream to start automatically: the one `video` was last played from (already cached,
/// and the saved position belongs to it), otherwise well seeded first, then 1080p, 720p, 4K
/// (the player shows at most 1080p, so 4K only costs bandwidth), SD; then the most seeders.
/// `tried` are skipped.
fn best_stream<'a>(streams: &'a [Stream], tried: &HashSet<String>, last: Option<&str>) -> Option<&'a Stream> {
    if let Some(s) = last.and_then(|k| streams.iter().find(|s| s.key() == k)).filter(|s| !tried.contains(&s.key())) {
        return Some(s);
    }
    streams
        .iter()
        .enumerate()
        .filter(|(_, s)| !tried.contains(&s.key()))
        .max_by_key(|(i, s)| {
            let info = s.info();
            let healthy = info.seeders.is_none_or(|n| n >= 5);
            let tier = match info.tier {
                Tier::FullHd => 3,
                Tier::Hd => 2,
                Tier::Uhd => 1,
                Tier::Sd => 0,
            };
            // Ties keep the addon's order.
            (healthy, tier, info.seeders.unwrap_or(0), std::cmp::Reverse(*i))
        })
        .map(|(_, s)| s)
}

/// The episode to continue with and the button label for it.
fn up_next(meta: &Meta, lib: Option<&LibItem>) -> Option<(Video, String)> {
    let last = lib.and_then(|l| Some((l, l.last_video.as_deref()?)));
    if let Some((l, last)) = last
        && let Some(v) = meta.videos.iter().find(|v| v.id == last)
    {
        let finished = l.watched.iter().any(|w| w == last) || l.progress() > 0.9;
        if !finished {
            return Some((v.clone(), format!("Continue {}", v.code())));
        }
        if let Some(n) = next_video(meta, v) {
            return Some((n.clone(), format!("Play {}", n.code())));
        }
    }
    let first = meta.videos.iter().find(|v| v.season > 0).or(meta.videos.first())?;
    Some((first.clone(), format!("Play {}", first.code()).trim().to_string()))
}

/// The episode after `v`, skipping specials unless `v` is one.
pub fn next_video<'a>(meta: &'a Meta, v: &Video) -> Option<&'a Video> {
    let pos = meta.videos.iter().position(|x| x.id == v.id)?;
    meta.videos[pos + 1..].iter().find(|x| (x.season == 0) == (v.season == 0))
}

fn lib_update(kind: &str, id: &str, meta: &Meta, saved: Option<bool>, video: Option<String>) -> LibUpdate {
    LibUpdate { id: id.into(), kind: kind.into(), name: meta.name.clone(), poster: meta.poster.clone(), saved, video, stream: None }
}

fn play_action(kind: &str, id: &str, meta: &Meta, stream: Stream, video: Option<&Video>, video_id: String) -> Action {
    let title = match video {
        Some(v) => format!("{} · {} {}", meta.name, v.code(), v.label()),
        None => meta.name.clone(),
    };
    let lib = LibUpdate { stream: Some(stream.key()), ..lib_update(kind, id, meta, None, Some(video_id)) };
    Action::Play(Box::new(PlayReq {
        stream,
        title,
        lib,
        video: video.cloned(),
        next: video.and_then(|v| next_video(meta, v)).cloned(),
    }))
}

/// YYYY-MM-DD in UTC.
fn today() -> String {
    let days = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86400) as i64;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

struct EpisodeState {
    last: bool,
    progress: Option<f32>,
    watched: bool,
    upcoming: bool,
}

/// Clickable row with the episode's thumbnail, name and synopsis.
fn episode_row(ui: &mut Ui, v: &Video, s: &EpisodeState) -> egui::Response {
    let resp = Frame::new()
        .fill(if s.last { ACCENT.gamma_multiply(0.14) } else { Color32::TRANSPARENT })
        .corner_radius(12)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if s.upcoming {
                ui.multiply_opacity(0.55);
            }
            ui.horizontal_top(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(136.0, 77.0), Sense::hover());
                let thumb = v.thumbnail.as_deref().filter(|t| !t.is_empty());
                if !thumb.is_some_and(|t| paint_cover(ui, t, rect, 8, Color32::WHITE)) {
                    ui.painter().rect_filled(rect, 8, PANEL2);
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, v.code(), semibold(14.0), MUTED);
                }
                if let Some(p) = s.progress {
                    progress_bar(ui.painter(), rect.shrink2(vec2(4.0, 3.0)), p);
                }
                if s.watched {
                    let c = rect.right_top() + vec2(-14.0, 14.0);
                    ui.painter().circle_filled(c, 10.0, OK);
                    paint_icon(ui.painter(), Rect::from_center_size(c, vec2(13.0, 13.0)), Icon::Check, BG);
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(Label::new(strong(format!("{}. {}", v.episode, v.label()), 14.0)).truncate());
                    let mut facts = vec![v.date().to_string()];
                    if s.upcoming {
                        facts.insert(0, "Upcoming".into());
                    } else if s.last && !s.watched {
                        facts.insert(0, "Continue watching".into());
                    }
                    let facts = facts.into_iter().filter(|f| !f.is_empty()).collect::<Vec<_>>().join(" · ");
                    ui.label(RichText::new(facts).size(12.0).color(if s.last { ACCENT2 } else { MUTED }));
                    if !v.overview().is_empty() {
                        clamped(ui, v.overview(), 12.5, MUTED, 2);
                    }
                });
            });
        })
        .response
        .interact(Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    if resp.hovered() && !s.last {
        ui.painter().rect_filled(resp.rect, 12, Color32::from_white_alpha(8));
    }
    resp
}

/// One stream; returns true when clicked.
fn stream_row(ui: &mut Ui, i: usize, s: &StreamInfo, addon: Option<&str>) -> bool {
    let resp = ui
        .push_id(i, |ui| {
            Frame::new().corner_radius(12).inner_margin(Margin { left: 10, right: 40, top: 10, bottom: 10 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if s.seeders == Some(0) {
                    ui.multiply_opacity(0.5);
                }
                ui.horizontal_top(|ui| {
                    let color = match s.tier {
                        Tier::Uhd => GOLD,
                        Tier::FullHd => ACCENT2,
                        Tier::Hd => TEAL,
                        Tier::Sd => MUTED,
                    };
                    let (rect, _) = ui.allocate_exact_size(vec2(58.0, 40.0), Sense::hover());
                    ui.painter().rect_filled(rect, 8, color.gamma_multiply(0.16));
                    ui.painter().text(rect.center() - vec2(0.0, 6.0), egui::Align2::CENTER_CENTER, s.tier.label(), bold(14.0), color);
                    let extra = s.quality.split_whitespace().skip(1).collect::<Vec<_>>().join(" ");
                    let extra = if extra.is_empty() && !s.quality.eq_ignore_ascii_case(s.tier.label()) {
                        s.quality.clone()
                    } else {
                        extra
                    };
                    if !extra.is_empty() {
                        let galley = ui.painter().layout(extra, semibold(9.5), color, rect.width() - 4.0);
                        ui.painter().galley(pos2(rect.center().x - galley.size().x / 2.0, rect.center().y + 3.0), galley, color);
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        let release = if s.release.is_empty() { "Stream" } else { &s.release };
                        clamped_font(ui, release, semibold(13.0), TEXT, 2);
                        let mut stats = Vec::new();
                        if let Some(n) = s.seeders {
                            stats.push(format!("{n} seeders"));
                        }
                        stats.extend(s.size.clone());
                        stats.extend(s.source.clone());
                        stats.extend(addon.map(String::from));
                        if !stats.is_empty() {
                            ui.horizontal(|ui| {
                                if let Some(n) = s.seeders {
                                    let health = if n >= 50 { OK } else if n >= 10 { GOLD } else { ERR };
                                    let (r, _) = ui.allocate_exact_size(vec2(8.0, 14.0), Sense::hover());
                                    ui.painter().circle_filled(r.center(), 3.5, health);
                                }
                                ui.label(RichText::new(stats.join("  ·  ")).size(12.0).color(SOFT));
                            });
                        }
                        if !s.notes.is_empty() {
                            clamped(ui, &s.notes, 11.5, MUTED, 2);
                        }
                    });
                });
            })
        })
        .inner
        .response
        .interact(Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    if t > 0.0 {
        let p = ui.painter();
        p.rect_filled(resp.rect, 12, Color32::from_white_alpha((10.0 * t) as u8));
        let c = pos2(resp.rect.right() - 22.0, resp.rect.center().y);
        p.circle_filled(c, 15.0, ACCENT.gamma_multiply(t));
        paint_icon(p, Rect::from_center_size(c + vec2(1.0, 0.0), vec2(13.0, 13.0)), Icon::Play, Color32::WHITE.gamma_multiply(t));
    }
    ui.add_space(2.0);
    resp.clicked()
}
