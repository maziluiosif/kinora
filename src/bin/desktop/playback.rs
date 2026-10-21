//! Full-window playback: waits for the stream, then plays it in the embedded player with
//! auto-hiding controls, resume and "up next".

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, Key, Layout, Popup, Rect, RectAlign, RichText, Sense, Ui, UiBuilder, pos2,
    vec2,
};
use serde_json::Value;
use kinora::{Core, PlayReq as CoreReq, player};

use crate::kit::*;
use crate::model::Video;
use crate::task::Task;
use crate::{Env, PlayReq, video};

const CONTROLS_TIMEOUT: Duration = Duration::from_millis(2500);
const SAVE_EVERY: Duration = Duration::from_secs(10);
/// "Up next" shows this long before the end.
const UP_NEXT: f64 = 90.0;

pub enum Exit {
    Stay,
    Close,
    /// Close and open the streams of this video.
    Next(String),
}

pub struct PlayerView {
    title: String,
    torrent: bool,
    started: Task<Value>,
    video: Option<video::Player>,
    url: Option<String>,
    stats: Option<Value>,
    polled: Option<Instant>,
    /// Last mouse activity; the controls hide a while after it.
    active: Instant,
    /// Seek bar position while it is being dragged.
    scrub: Option<f64>,
    /// Volume before muting.
    unmuted: Option<f32>,
    lib_id: String,
    video_id: Option<String>,
    next: Option<Video>,
    next_dismissed: bool,
    /// Position to seek to once the stream is open.
    resume: Option<f64>,
    /// When playback was resumed, to offer "Start over" for a while.
    resumed: Option<Instant>,
    saved: Instant,
    /// Short on-screen message (seek, volume).
    osd: Option<(String, Instant)>,
    /// Play/pause pulse in the middle of the screen.
    flash: Option<(bool, Instant)>,
}

impl PlayerView {
    pub fn start(env: &Env, req: PlayReq, resume: Option<f64>) -> Self {
        let s = &req.stream;
        let core_req = CoreReq {
            info_hash: s.info_hash.clone(),
            file_idx: s.file_idx,
            filename: s.behavior_hints.filename.clone(),
            sources: s.sources.clone(),
            url: s.url.clone(),
            title: req.title.clone(),
            external: false,
        };
        let started = env.spawn(move |core| async move { core.play(core_req).await });
        PlayerView {
            title: req.title,
            torrent: s.info_hash.is_some(),
            started,
            video: None,
            url: None,
            stats: None,
            polled: None,
            active: Instant::now(),
            scrub: None,
            unmuted: None,
            lib_id: req.lib.id,
            video_id: req.lib.video,
            next: req.next,
            next_dismissed: false,
            resume,
            resumed: None,
            saved: Instant::now(),
            osd: None,
            flash: None,
        }
    }

    /// Stores the playback position in the library.
    pub fn save_progress(&self, core: &Core) {
        let (Some(v), Some(vid)) = (&self.video, &self.video_id) else { return };
        let Some(d) = v.duration().filter(|d| *d > 0.0) else { return };
        let pos = if v.ended() { d } else { v.position() };
        if pos > 5.0
            && let Err(e) = core.library.progress(&self.lib_id, vid, pos, d)
        {
            tracing::warn!("saving progress failed: {e:#}");
        }
    }

    fn osd(&mut self, msg: impl Into<String>) {
        self.osd = Some((msg.into(), Instant::now()));
        self.active = Instant::now();
    }

    fn toggle_pause(&mut self) {
        if let Some(v) = &mut self.video {
            toggle_pause(v);
            self.flash = Some((v.paused(), Instant::now()));
        }
    }

    fn seek_by(&mut self, delta: f64) {
        if let Some(v) = &mut self.video {
            v.seek(v.position() + delta);
            self.osd(if delta < 0.0 { format!("−{} s", -delta) } else { format!("+{delta} s") });
        }
    }

    fn change_volume(&mut self, delta: f32) {
        if let Some(v) = &self.video {
            v.set_volume(v.volume() + delta);
            self.unmuted = None;
            let pct = (v.volume() * 100.0).round();
            self.osd(format!("Volume {pct}%"));
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, env: &Env) -> Exit {
        let ctx = ui.ctx().clone();
        let rect = ui.max_rect();
        let mut exit = Exit::Stay;

        let mut torrent_id = None;
        match self.started.get() {
            Some(Ok(res)) => {
                torrent_id = res["torrent_id"].as_u64().map(|t| t as usize);
                if self.video.is_none() {
                    let url = res["url"].as_str().unwrap_or_default().to_string();
                    self.video = Some(video::Player::open(url.clone(), &ctx));
                    self.url = Some(url);
                }
            }
            Some(Err(e)) => {
                let e = e.clone();
                return self.failure_ui(ui, rect, "Couldn't start this stream", &e);
            }
            None => {}
        }
        if let Some(tid) = torrent_id {
            if self.polled.is_none_or(|t| t.elapsed() >= Duration::from_millis(1500)) {
                self.stats = env.core.status(tid).ok();
                self.polled = Some(Instant::now());
            }
            ctx.request_repaint_after(Duration::from_millis(1500));
        }

        // Resume where the last session stopped, once the duration is known.
        if let Some(v) = &mut self.video
            && let Some(Ok(_)) = &v.info
            && let Some(t) = self.resume.take()
            && v.duration().is_none_or(|d| t < d - 30.0)
        {
            v.seek(t);
            self.resumed = Some(Instant::now());
        }
        if self.saved.elapsed() >= SAVE_EVERY {
            self.save_progress(env.core);
            self.saved = Instant::now();
        }

        // Video frame.
        let mut has_frame = false;
        if let Some(v) = &mut self.video
            && let Some(tex) = v.update(&ctx)
        {
            has_frame = true;
            let size = tex.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let r = Rect::from_center_size(rect.center(), size * scale);
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            ui.painter().image(tex.id(), r, uv, Color32::WHITE);
        }
        if let Some(Err(e)) = self.video.as_ref().and_then(|v| v.info.as_ref()) {
            let e = e.clone();
            return self.failure_ui(ui, rect, "This stream can't be played here", &e);
        }

        // Input on the video area; the controls drawn later sit on top of it.
        let area = ui.interact(rect, ui.id().with("video_area"), Sense::click());
        let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down()) {
            self.active = Instant::now();
        }
        if self.video.is_some() {
            if area.double_clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                self.toggle_pause();
            } else if area.clicked() {
                self.toggle_pause();
            }
            let keys = ctx.input(|i| {
                let p = |k| i.key_pressed(k);
                [p(Key::Space), p(Key::ArrowLeft), p(Key::ArrowRight), p(Key::ArrowUp), p(Key::ArrowDown), p(Key::F), p(Key::M), p(Key::N)]
            });
            let [space, left, right, up, down, f, m, n] = keys;
            if space {
                self.toggle_pause();
            }
            if left || right {
                self.seek_by(if left { -10.0 } else { 10.0 });
            }
            if up || down {
                self.change_volume(if up { 0.1 } else { -0.1 });
            }
            if f {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
            }
            if m {
                self.toggle_mute();
            }
            if n && let Some(next) = &self.next {
                exit = Exit::Next(next.id.clone());
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if fullscreen {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            } else {
                exit = Exit::Close;
            }
        }

        let idle = self.active.elapsed();
        let paused = self.video.as_ref().is_none_or(|v| v.paused() || v.ended());
        let show = idle < CONTROLS_TIMEOUT || paused || !has_frame;
        let fade = ctx.animate_bool_with_time(ui.id().with("controls"), show, 0.2);

        // Subtitles, above the controls when they are shown.
        if let Some(text) = self.video.as_ref().and_then(|v| v.subtitle()) {
            let bottom = rect.bottom() - 40.0 - 90.0 * fade;
            subtitle_ui(ui, rect, bottom, &text);
        }

        // Loading / buffering.
        let waiting = !has_frame || self.video.as_ref().is_some_and(|v| v.buffering());
        if waiting {
            self.waiting_ui(ui, rect);
        }
        self.flash_ui(ui, rect);

        if fade > 0.0 {
            ui.scope(|ui| {
                ui.multiply_opacity(fade);
                if self.top_bar(ui, rect) {
                    exit = Exit::Close;
                }
                if self.video.is_some() {
                    self.bottom_bar(ui, rect, fullscreen);
                }
            });
            if !paused {
                ctx.request_repaint_after(CONTROLS_TIMEOUT.saturating_sub(idle) + Duration::from_millis(50));
            }
        } else if area.contains_pointer() {
            ctx.set_cursor_icon(CursorIcon::None);
        }
        if fade > 0.0 && fade < 1.0 {
            ctx.request_repaint();
        }
        self.osd_ui(ui, rect);
        if let Some(e) = self.up_next_ui(ui, rect, fade) {
            exit = e;
        }
        self.resumed_ui(ui, rect, fade);

        if !matches!(exit, Exit::Stay) && fullscreen {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        exit
    }

    fn failure_ui(&mut self, ui: &mut Ui, rect: Rect, title: &str, err: &str) -> Exit {
        let mut exit = Exit::Stay;
        let inner = rect.shrink(60.0);
        ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(inner.height() * 0.3);
                ui.label(RichText::new(title).font(bold(22.0)).color(Color32::WHITE));
                ui.add_space(4.0);
                ui.label(RichText::new(err).size(13.0).color(MUTED));
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    let w = 300.0;
                    ui.add_space((ui.available_width() - w).max(0.0) / 2.0);
                    if button(ui, Kind::Primary, Some(Icon::ChevronLeft), "Choose another stream").clicked() {
                        exit = Exit::Close;
                    }
                    if let Some(url) = &self.url
                        && button(ui, Kind::Secondary, Some(Icon::External), "External player").clicked()
                        && let Err(e) = player::launch(url, &self.title)
                    {
                        tracing::warn!("external player: {e:#}");
                    }
                });
            });
        });
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            exit = Exit::Close;
        }
        exit
    }

    fn waiting_ui(&self, ui: &mut Ui, rect: Rect) {
        let msg = match (&self.video, self.torrent) {
            (None, true) => "Connecting to peers…",
            (None, false) => "Starting…",
            (Some(v), _) if v.info.is_none() => "Opening stream…",
            (Some(_), _) if self.resume.is_some() || self.resumed.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) => {
                "Resuming…"
            }
            _ => "Buffering…",
        };
        let c = rect.center();
        ui.painter().circle_filled(c, 44.0, Color32::from_black_alpha(120));
        ui.put(Rect::from_center_size(c, vec2(44.0, 44.0)), egui::Spinner::new().size(40.0).color(ACCENT2));
        ui.painter().text(c + vec2(0.0, 60.0), Align2::CENTER_TOP, msg, semibold(16.0), TEXT);
        if let Some(line) = self.stats_line() {
            ui.painter().text(c + vec2(0.0, 86.0), Align2::CENTER_TOP, line, egui::FontId::proportional(13.0), MUTED);
        }
    }

    fn flash_ui(&mut self, ui: &Ui, rect: Rect) {
        let Some((paused, t)) = self.flash else { return };
        let age = t.elapsed().as_secs_f32();
        if age > 0.6 {
            self.flash = None;
            return;
        }
        let k = age / 0.6;
        let alpha = 1.0 - k;
        let r = 36.0 + 18.0 * k;
        ui.painter().circle_filled(rect.center(), r, Color32::from_black_alpha((150.0 * alpha) as u8));
        let icon = if paused { Icon::Pause } else { Icon::Play };
        paint_icon(ui.painter(), Rect::from_center_size(rect.center(), vec2(r, r)), icon, Color32::WHITE.gamma_multiply(alpha));
        ui.ctx().request_repaint();
    }

    fn osd_ui(&mut self, ui: &Ui, rect: Rect) {
        let Some((msg, t)) = &self.osd else { return };
        let age = t.elapsed().as_secs_f32();
        if age > 1.2 {
            self.osd = None;
            return;
        }
        let alpha = (1.0 - (age - 0.8).max(0.0) / 0.4).clamp(0.0, 1.0);
        let p = ui.painter();
        let galley = p.layout_no_wrap(msg.clone(), semibold(18.0), Color32::WHITE.gamma_multiply(alpha));
        let r = Rect::from_center_size(pos2(rect.center().x, rect.top() + 100.0), galley.size() + vec2(32.0, 16.0));
        p.rect_filled(r, 10, Color32::from_black_alpha((170.0 * alpha) as u8));
        p.galley(r.center() - galley.size() / 2.0, galley, Color32::WHITE);
        ui.ctx().request_repaint();
    }

    /// "Up next" card near the end of an episode.
    fn up_next_ui(&mut self, ui: &mut Ui, rect: Rect, fade: f32) -> Option<Exit> {
        let next = self.next.as_ref()?;
        let v = self.video.as_ref()?;
        let d = v.duration()?;
        if self.next_dismissed || !(v.ended() || v.position() > d - UP_NEXT) {
            return None;
        }
        let size = vec2(340.0, 96.0);
        let r = Rect::from_min_size(rect.right_bottom() - size - vec2(28.0, 40.0 + 100.0 * fade), size);
        let mut exit = None;
        ui.painter().rect_filled(r, 14, Color32::from_rgba_unmultiplied(20, 18, 28, 235));
        ui.scope_builder(UiBuilder::new().max_rect(r.shrink(14.0)), |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(muted("UP NEXT", 11.0));
                    ui.add(egui::Label::new(strong(format!("{} · {}", next.code(), next.label()), 14.0)).truncate());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if button_sized(ui, Kind::Primary, Some(Icon::Next), "Next episode", 32.0).on_hover_text("N").clicked() {
                            exit = Some(Exit::Next(next.id.clone()));
                        }
                        if button_sized(ui, Kind::Ghost, None, "Dismiss", 32.0).clicked() {
                            self.next_dismissed = true;
                        }
                    });
                });
            });
        });
        exit
    }

    fn resumed_ui(&mut self, ui: &mut Ui, rect: Rect, fade: f32) {
        let Some(t) = self.resumed else { return };
        if t.elapsed() > Duration::from_secs(8) {
            self.resumed = None;
            return;
        }
        let Some(v) = &mut self.video else { return };
        let size = vec2(300.0, 48.0);
        let r = Rect::from_min_size(pos2(rect.left() + 28.0, rect.bottom() - size.y - 40.0 - 100.0 * fade), size);
        ui.painter().rect_filled(r, 12, Color32::from_rgba_unmultiplied(20, 18, 28, 235));
        let mut over = false;
        ui.scope_builder(UiBuilder::new().max_rect(r.shrink2(vec2(14.0, 8.0))).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.label(RichText::new("Resumed where you left off").size(13.0).color(SOFT));
            over = button_sized(ui, Kind::Secondary, None, "Start over", 30.0).clicked();
        });
        if over {
            v.seek(0.0);
            self.resumed = None;
        }
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }

    fn toggle_mute(&mut self) {
        let Some(v) = &self.video else { return };
        match self.unmuted.take() {
            Some(vol) => {
                v.set_volume(vol);
                self.osd(format!("Volume {}%", (vol * 100.0).round()));
            }
            None => {
                self.unmuted = Some(v.volume());
                v.set_volume(0.0);
                self.osd("Muted");
            }
        }
    }

    fn stats_line(&self) -> Option<String> {
        let s = self.stats.as_ref()?;
        let speed = s["live"]["download_speed"]["human_readable"].as_str().unwrap_or("0");
        let peers = &s["live"]["snapshot"]["peer_stats"];
        let peers = match (peers["live"].as_u64(), peers["seen"].as_u64()) {
            (Some(l), Some(n)) => format!("{l} of {n} peers"),
            _ => "no peers yet".into(),
        };
        Some(format!("{speed}  ·  {peers}"))
    }

    /// Returns true when Back was clicked.
    fn top_bar(&mut self, ui: &mut Ui, rect: Rect) -> bool {
        let bar = rect.with_max_y(rect.top() + 110.0);
        gradient_v(ui.painter(), bar, Color32::from_black_alpha(190), Color32::TRANSPARENT);
        let mut back = false;
        let row = Rect::from_min_max(rect.min + vec2(20.0, 14.0), pos2(rect.right() - 20.0, rect.top() + 62.0));
        ui.scope_builder(UiBuilder::new().max_rect(row).layout(Layout::left_to_right(Align::Center)), |ui| {
            back = icon_button_with(ui, Icon::ChevronLeft, 42.0, "Back (Esc)", Color32::from_white_alpha(20)).clicked();
            ui.add_space(6.0);
            let stats = self.stats_line();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(line) = stats {
                    ui.label(RichText::new(line).size(12.5).color(SOFT));
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(&self.title).font(semibold(17.0)).color(Color32::WHITE)).truncate());
                });
            });
        });
        back
    }

    fn bottom_bar(&mut self, ui: &mut Ui, rect: Rect, fullscreen: bool) {
        let bar = rect.with_min_y(rect.bottom() - 170.0);
        gradient_v(ui.painter(), bar, Color32::TRANSPARENT, Color32::from_black_alpha(215));
        let inner = Rect::from_min_max(pos2(rect.left() + 24.0, rect.bottom() - 96.0), pos2(rect.right() - 24.0, rect.bottom() - 14.0));
        let mut audio_choice = None;
        let mut sub_choice = None;
        let (mut mute, mut external, mut pause, mut seek_by) = (false, false, false, 0.0);
        let Some(v) = &mut self.video else { return };
        ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
            let pos = v.position();
            let duration = v.duration();
            ui.horizontal(|ui| {
                let shown = self.scrub.unwrap_or(pos);
                ui.label(RichText::new(fmt_time(shown)).monospace().size(12.5).color(Color32::WHITE));
                let right = duration.map(|d| format!("-{}", fmt_time(d - shown))).unwrap_or_default();
                let right_w = 64.0;
                if let Some(d) = duration.filter(|d| *d > 0.0) {
                    ui.allocate_ui(vec2(ui.available_width() - right_w, 22.0), |ui| {
                        if let Some(t) = seek_bar(ui, pos, d, &mut self.scrub) {
                            v.seek(t);
                        }
                    });
                } else {
                    ui.add_space(ui.available_width() - right_w);
                }
                ui.label(RichText::new(right).monospace().size(12.5).color(SOFT));
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let icon = if v.paused() || v.ended() { Icon::Play } else { Icon::Pause };
                pause = icon_button_with(ui, icon, 46.0, "Play / pause (Space)", Color32::from_white_alpha(28)).clicked();
                if icon_button(ui, Icon::Replay10, 40.0, "Back 10 s (←)").clicked() {
                    seek_by = -10.0;
                }
                if icon_button(ui, Icon::Forward10, 40.0, "Forward 10 s (→)").clicked() {
                    seek_by = 10.0;
                }
                let vol = v.volume();
                let icon = if vol == 0.0 { Icon::Mute } else { Icon::Volume };
                mute = icon_button(ui, icon, 40.0, "Mute (M)").clicked();
                if let Some(vol) = volume_bar(ui, vol) {
                    v.set_volume(vol);
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (icon, tip) = if fullscreen {
                        (Icon::ExitFullscreen, "Exit full screen (F)")
                    } else {
                        (Icon::Fullscreen, "Full screen (F)")
                    };
                    if icon_button(ui, icon, 40.0, tip).clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                    }
                    external = icon_button(ui, Icon::External, 40.0, "Open in an external player").clicked();
                    if let Some(Ok(info)) = &v.info {
                        if !info.sub_tracks.is_empty() {
                            let resp = icon_button(ui, Icon::Subtitles, 40.0, "Subtitles");
                            if info.sub.is_some() {
                                ui.painter().rect_filled(
                                    Rect::from_center_size(resp.rect.center_bottom() - vec2(0.0, 4.0), vec2(14.0, 2.5)),
                                    1,
                                    ACCENT2,
                                );
                            }
                            Popup::menu(&resp).align(RectAlign::TOP_END).gap(8.0).show(|ui| {
                                ui.set_min_width(200.0);
                                ui.label(muted("SUBTITLES", 11.0));
                                if ui.selectable_label(info.sub.is_none(), "Off").clicked() {
                                    sub_choice = Some(None);
                                }
                                for t in &info.sub_tracks {
                                    if ui.selectable_label(info.sub == Some(t.index), &t.label).clicked() {
                                        sub_choice = Some(Some(t.index));
                                    }
                                }
                            });
                        }
                        if info.audio_tracks.len() > 1 {
                            let resp = icon_button(ui, Icon::Audio, 40.0, "Audio track");
                            Popup::menu(&resp).align(RectAlign::TOP_END).gap(8.0).show(|ui| {
                                ui.set_min_width(200.0);
                                ui.label(muted("AUDIO", 11.0));
                                for t in &info.audio_tracks {
                                    if ui.selectable_label(info.audio == Some(t.index), &t.label).clicked() {
                                        audio_choice = Some(t.index);
                                    }
                                }
                            });
                        }
                    }
                });
            });
        });
        if let Some(i) = audio_choice {
            v.set_audio_track(i);
        }
        if let Some(i) = sub_choice {
            v.set_sub_track(i);
        }
        if external && let Some(url) = &self.url {
            v.set_paused(true);
            if let Err(e) = player::launch(url, &self.title) {
                tracing::warn!("external player: {e:#}");
            }
        }
        if pause {
            self.toggle_pause();
        }
        if seek_by != 0.0 {
            self.seek_by(seek_by);
        }
        if mute {
            self.toggle_mute();
        }
    }
}

fn toggle_pause(v: &mut video::Player) {
    if v.ended() {
        v.seek(0.0);
        v.set_paused(false);
    } else {
        v.set_paused(!v.paused());
    }
}

fn fmt_time(t: f64) -> String {
    let t = t.max(0.0) as u64;
    let (h, m, s) = (t / 3600, t / 60 % 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Returns the time to seek to once the bar is released.
fn seek_bar(ui: &mut Ui, pos: f64, duration: f64, scrub: &mut Option<f64>) -> Option<f64> {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click_and_drag());
    let at = |x: f32| ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * duration;
    let mut seek = None;
    if resp.is_pointer_button_down_on() {
        if let Some(p) = resp.interact_pointer_pos() {
            *scrub = Some(at(p.x));
        }
    } else if let Some(t) = scrub.take() {
        seek = Some(t);
    }
    let shown = scrub.unwrap_or(pos);
    let hot = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || scrub.is_some(), 0.12);
    let painter = ui.painter();
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0 + 2.0 * hot));
    painter.rect_filled(track, 3, Color32::from_white_alpha(50));
    if let Some(h) = resp.hover_pos() {
        painter.rect_filled(track.with_max_x(h.x), 3, Color32::from_white_alpha(40));
    }
    let played = track.with_max_x(track.left() + track.width() * (shown / duration).clamp(0.0, 1.0) as f32);
    painter.rect_filled(played, 3, ACCENT_HI);
    painter.circle_filled(pos2(played.right(), track.center().y), 4.0 + 3.0 * hot, Color32::WHITE);
    if let Some(h) = resp.hover_pos() {
        let label = fmt_time(at(h.x));
        let galley = painter.layout_no_wrap(label, egui::FontId::monospace(12.0), Color32::WHITE);
        let r = Rect::from_center_size(pos2(h.x, rect.top() - 16.0), galley.size() + vec2(12.0, 6.0));
        painter.rect_filled(r, 6, Color32::from_black_alpha(200));
        painter.galley(r.center() - galley.size() / 2.0, galley, Color32::WHITE);
    }
    if hot > 0.5 {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    seek
}

/// Returns the new volume while the bar is clicked or dragged.
fn volume_bar(ui: &mut Ui, volume: f32) -> Option<f32> {
    let (rect, resp) = ui.allocate_exact_size(vec2(96.0, 30.0), Sense::click_and_drag());
    let hot = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || resp.dragged(), 0.12);
    let track = Rect::from_center_size(rect.center(), vec2(rect.width() - 8.0, 4.0));
    let p = ui.painter();
    p.rect_filled(track, 2, Color32::from_white_alpha(50));
    let level = track.with_max_x(track.left() + track.width() * volume);
    p.rect_filled(level, 2, Color32::WHITE);
    p.circle_filled(pos2(level.right(), track.center().y), 4.0 + 2.0 * hot, Color32::WHITE);
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(format!("Volume {}%", (volume * 100.0).round()));
    if resp.is_pointer_button_down_on() {
        let x = resp.interact_pointer_pos()?.x;
        return Some(((x - track.left()) / track.width()).clamp(0.0, 1.0));
    }
    None
}

fn subtitle_ui(ui: &Ui, rect: Rect, bottom: f32, text: &str) {
    let font = egui::FontId::new((rect.height() * 0.042).clamp(18.0, 44.0), egui::FontFamily::Name(SEMIBOLD.into()));
    let painter = ui.painter();
    let galleys: Vec<_> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| painter.layout(l.trim().to_string(), font.clone(), Color32::WHITE, rect.width() * 0.85))
        .collect();
    let mut y = bottom - galleys.iter().map(|g| g.size().y + 2.0).sum::<f32>();
    for g in galleys {
        let r = Rect::from_min_size(pos2(rect.center().x - g.size().x / 2.0, y), g.size());
        painter.rect_filled(r.expand2(vec2(10.0, 2.0)), 6, Color32::from_black_alpha(160));
        y += g.size().y + 2.0;
        painter.galley(r.min, g, Color32::WHITE);
    }
}
