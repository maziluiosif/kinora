//! Native desktop client (egui). Runs the same core as the web version in-process and plays
//! streams in the embedded player (`video.rs`); the local HTTP server hands torrent streams to
//! FFmpeg (or to an external player).

mod detail;
mod kit;
mod model;
mod pages;
mod playback;
mod task;
mod video;

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CursorIcon, Frame, Id, Key, Margin, Modifiers, Panel, Rect, RichText, Sense, TextEdit, Ui,
    pos2, vec2,
};
use kinora::library::{LibItem, LibUpdate};
use kinora::{CatalogRef, Core, server};
use tokio::runtime::{Handle, Runtime};

use detail::Detail;
use kit::*;
use model::{Stream, Video};
use pages::{AddonsPage, Board, Discover, LibraryPage, Search};
use playback::{Exit, PlayerView};
use task::Task;

/// The web version uses 11471.
const PORT: u16 = 11472;
const SEARCH_ID: &str = "search";

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,librqbit=warn,librqbit_dht=warn".into()),
        )
        .init();

    let rt = Runtime::new().expect("failed to start the tokio runtime");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Kinora")
            .with_inner_size([1320.0, 860.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Kinora",
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(DesktopApp::new(rt, &cc.egui_ctx)))
        }),
    )
}

/// Binds the stream server, loads addons and serves `/stream/...` in the background.
async fn start() -> anyhow::Result<Arc<Core>> {
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", PORT)).await {
        Ok(l) => l,
        Err(_) => tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?,
    };
    let port = listener.local_addr()?.port();
    let core = Arc::new(Core::new(port, std::env::temp_dir().join("kinora-desktop")).await?);
    let router = server::router(core.clone());
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            tracing::error!("stream server stopped: {e:#}");
        }
    });
    tracing::info!("stream server on 127.0.0.1:{port}");
    Ok(core)
}

// ---- App shell ----

pub enum Route {
    Board,
    Discover(Option<CatalogRef>),
    Search(String),
    Detail { kind: String, id: String },
    Library,
    Addons,
}

/// What to play and how to record it in the library.
pub struct PlayReq {
    pub stream: Stream,
    pub title: String,
    pub lib: LibUpdate,
    pub video: Option<Video>,
    pub next: Option<Video>,
}

pub enum Action {
    Go(Route),
    Back,
    Play(Box<PlayReq>),
    Toast(String),
}

/// What the views get: the core, a way to run async work, and a place to queue navigation.
pub struct Env<'a> {
    pub core: &'a Arc<Core>,
    pub rt: Handle,
    pub ctx: egui::Context,
    pub library: Vec<LibItem>,
    pub actions: Vec<Action>,
}

impl Env<'_> {
    pub fn spawn<T, F>(&self, f: impl FnOnce(Arc<Core>) -> F) -> Task<T>
    where
        T: Send + 'static,
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        Task::spawn(&self.rt, &self.ctx, f(self.core.clone()))
    }

    pub fn go(&mut self, route: Route) {
        self.actions.push(Action::Go(route));
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.actions.push(Action::Toast(msg.into()));
    }

    pub fn lib_item(&self, id: &str) -> Option<&LibItem> {
        self.library.iter().find(|i| i.id == id)
    }

    /// Adds or removes a title from the library, with a toast saying which.
    pub fn toggle_saved(&mut self, u: LibUpdate) {
        let saved = u.saved == Some(true);
        let name = u.name.clone();
        match self.core.library.update(u) {
            Ok(()) => {
                self.library = self.core.library.items();
                self.toast(if saved { format!("Added “{name}” to your library") } else { format!("Removed “{name}”") });
            }
            Err(e) => self.toast(format!("Library update failed: {e:#}")),
        }
    }
}

enum Page {
    Board(Board),
    Discover(Discover),
    Search(Search),
    Detail(Box<Detail>),
    Library(LibraryPage),
    Addons(Box<AddonsPage>),
}

struct DesktopApp {
    rt: Runtime,
    core: Task<Arc<Core>>,
    page: Option<Page>,
    history: Vec<Page>,
    search: String,
    search_edited: Option<Instant>,
    player: Option<PlayerView>,
    toasts: Vec<(String, Instant)>,
    help: bool,
}

impl DesktopApp {
    fn new(rt: Runtime, ctx: &egui::Context) -> Self {
        let core = Task::spawn(rt.handle(), ctx, start());
        DesktopApp {
            rt,
            core,
            page: None,
            history: Vec::new(),
            search: String::new(),
            search_edited: None,
            player: None,
            toasts: Vec::new(),
            help: false,
        }
    }

    fn apply(&mut self, env: &Env, action: Action) {
        match action {
            Action::Go(route) => {
                if let Route::Search(q) = &route {
                    self.search = q.clone();
                } else {
                    self.search.clear();
                }
                let replace = matches!((&route, &self.page), (Route::Search(_), Some(Page::Search(_))));
                let page = open(env, route);
                if let Some(prev) = self.page.replace(page).filter(|_| !replace) {
                    self.history.push(prev);
                    if self.history.len() > 50 {
                        self.history.remove(0);
                    }
                }
            }
            Action::Back => {
                if let Some(prev) = self.history.pop() {
                    self.search = match &prev {
                        Page::Search(s) => s.query.clone(),
                        _ => String::new(),
                    };
                    self.page = Some(prev);
                }
            }
            Action::Play(req) => {
                let resume = env
                    .lib_item(&req.lib.id)
                    .filter(|i| i.last_video.is_some() && i.last_video == req.lib.video)
                    .map(|i| i.time_offset)
                    .filter(|&t| t > 15.0);
                if let Err(e) = env.core.library.update(req.lib.clone()) {
                    tracing::warn!("library update failed: {e:#}");
                }
                self.player = Some(PlayerView::start(env, *req, resume));
            }
            Action::Toast(msg) => self.toasts.push((msg, Instant::now())),
        }
    }
}

fn open(env: &Env, route: Route) -> Page {
    match route {
        Route::Board => Page::Board(Board::new(env)),
        Route::Discover(cat) => Page::Discover(Discover::new(env, cat)),
        Route::Search(q) => Page::Search(Search::new(env, q)),
        Route::Detail { kind, id } => Page::Detail(Box::new(Detail::new(env, kind, id))),
        Route::Library => Page::Library(LibraryPage::default()),
        Route::Addons => Page::Addons(Box::new(AddonsPage::new(env))),
    }
}

impl eframe::App for DesktopApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let core = match self.core.get() {
            Some(Ok(core)) => core.clone(),
            other => {
                let err = other.map(|r| r.as_ref().err().cloned().unwrap_or_default());
                egui::CentralPanel::no_frame().frame(Frame::new().fill(BG)).show(ui, |ui| splash(ui, err));
                return;
            }
        };
        let mut env = Env {
            library: core.library.items(),
            core: &core,
            rt: self.rt.handle().clone(),
            ctx: ui.ctx().clone(),
            actions: Vec::new(),
        };
        if self.page.is_none() {
            self.page = Some(Page::Board(Board::new(&env)));
        }

        if let Some(p) = &mut self.player {
            let mut exit = Exit::Stay;
            egui::CentralPanel::no_frame()
                .frame(Frame::new().fill(Color32::BLACK))
                .show(ui, |ui| exit = p.ui(ui, &env));
            if !matches!(exit, Exit::Stay) {
                p.save_progress(&core);
                self.player = None;
                // The next source replaces the torrent itself; stopping here could race with it.
                if !matches!(exit, Exit::Retry) {
                    let core = core.clone();
                    self.rt.spawn(async move { core.engine.stop().await });
                }
                match (exit, &mut self.page) {
                    (Exit::Next(video), Some(Page::Detail(d))) => d.open_episode(&env, &video),
                    (Exit::Retry, Some(Page::Detail(d))) => {
                        if !d.play_next_source(&mut env) {
                            env.toast("No other source left, pick one manually");
                        }
                    }
                    _ => {}
                }
                for a in std::mem::take(&mut env.actions) {
                    self.apply(&env, a);
                }
            }
            if self.player.is_none() {
                self.toasts_ui(ui);
            }
            return;
        }

        self.shortcuts(ui, &mut env);
        self.nav(ui, &mut env);
        self.top_bar(ui, &mut env);
        let detail = matches!(self.page, Some(Page::Detail(_)));
        let margin = if detail { Margin::ZERO } else { Margin { left: 28, right: 12, top: 4, bottom: 0 } };
        egui::CentralPanel::no_frame().frame(Frame::new().fill(BG).inner_margin(margin)).show(ui, |ui| {
            match self.page.as_mut().unwrap() {
                Page::Board(p) => p.ui(ui, &mut env),
                Page::Discover(p) => p.ui(ui, &mut env),
                Page::Search(p) => p.ui(ui, &mut env),
                Page::Detail(p) => p.ui(ui, &mut env),
                Page::Library(p) => p.ui(ui, &mut env),
                Page::Addons(p) => p.ui(ui, &mut env),
            }
        });
        self.help_ui(ui);

        let actions = std::mem::take(&mut env.actions);
        for a in actions {
            self.apply(&env, a);
        }
        self.toasts_ui(ui);
    }

    fn on_exit(&mut self) {
        if let Some(Ok(core)) = self.core.get() {
            if let Some(p) = &self.player {
                p.save_progress(core);
            }
            self.rt.block_on(core.engine.stop());
        }
    }
}

fn splash(ui: &mut Ui, err: Option<String>) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.38);
        logo(ui, 64.0);
        ui.add_space(14.0);
        ui.label(RichText::new("Kinora").font(bold(26.0)));
        ui.add_space(10.0);
        match err {
            Some(e) => {
                ui.label(RichText::new(format!("Failed to start: {e}")).color(ERR));
            }
            None => {
                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() / 2.0 - 70.0);
                    ui.spinner();
                    ui.label(muted("Loading addons…", 14.0));
                });
            }
        }
    });
}

/// The app mark: a play symbol on an accent tile.
fn logo(ui: &mut Ui, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, size * 0.28, ACCENT);
    paint_icon(p, Rect::from_center_size(rect.center() + vec2(size * 0.04, 0.0), vec2(size, size) * 0.5), Icon::Play, Color32::WHITE);
    resp
}

impl DesktopApp {
    fn shortcuts(&mut self, ui: &Ui, env: &mut Env) {
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        let (find, back) = ui.input_mut(|i| {
            let find = i.consume_key(Modifiers::COMMAND, Key::F) || (!typing && i.consume_key(Modifiers::NONE, Key::Slash));
            let back = i.consume_key(Modifiers::COMMAND, Key::OpenBracket)
                || i.consume_key(Modifiers::ALT, Key::ArrowLeft)
                || i.pointer.button_pressed(egui::PointerButton::Extra1)
                || (!typing && i.consume_key(Modifiers::NONE, Key::Backspace));
            (find, back)
        });
        if find {
            ui.ctx().memory_mut(|m| m.request_focus(Id::new(SEARCH_ID)));
        }
        if back {
            env.actions.push(Action::Back);
        }
        if !typing && ui.input(|i| i.key_pressed(Key::Questionmark)) {
            self.help = !self.help;
        }
    }

    fn nav(&mut self, ui: &mut Ui, env: &mut Env) {
        let current = match &self.page {
            Some(Page::Board(_)) => 0,
            Some(Page::Discover(_)) => 1,
            Some(Page::Library(_)) => 2,
            Some(Page::Addons(_)) => 3,
            _ => usize::MAX,
        };
        Panel::left("nav")
            .resizable(false)
            .show_separator_line(false)
            .exact_size(84.0)
            .frame(Frame::new().fill(PANEL).inner_margin(Margin::symmetric(10, 18)))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    if logo(ui, 42.0).on_hover_cursor(CursorIcon::PointingHand).clicked() && current != 0 {
                        env.go(Route::Board);
                    }
                    ui.add_space(22.0);
                    let items =
                        [(Icon::Home, "Home"), (Icon::Compass, "Discover"), (Icon::Bookmark, "Library"), (Icon::Addons, "Addons")];
                    for (i, (icon, name)) in items.into_iter().enumerate() {
                        if nav_item(ui, icon, name, i == current).clicked() && i != current {
                            env.go(match i {
                                0 => Route::Board,
                                1 => Route::Discover(None),
                                2 => Route::Library,
                                _ => Route::Addons,
                            });
                        }
                        ui.add_space(4.0);
                    }
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                        if icon_button(ui, Icon::Help, 36.0, "Keyboard shortcuts (?)").clicked() {
                            self.help = !self.help;
                        }
                    });
                });
            });
    }

    fn top_bar(&mut self, ui: &mut Ui, env: &mut Env) {
        Panel::top("top")
            .show_separator_line(false)
            .frame(Frame::new().fill(BG).inner_margin(Margin { left: 24, right: 24, top: 14, bottom: 10 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!self.history.is_empty(), |ui| {
                        if icon_button_with(ui, Icon::ChevronLeft, 38.0, "Back (⌘[)", PANEL2).clicked() {
                            env.actions.push(Action::Back);
                        }
                    });
                    ui.add_space(6.0);
                    let width = (ui.available_width() - 40.0).clamp(240.0, 560.0);
                    if let Some(q) = search_box(ui, &mut self.search, &mut self.search_edited, width) {
                        env.go(Route::Search(q));
                    }
                });
            });
    }

    fn help_ui(&mut self, ui: &Ui) {
        if !self.help {
            return;
        }
        let modal = egui::Modal::new(Id::new("help")).show(ui.ctx(), |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new("Keyboard shortcuts").font(bold(20.0)));
            ui.add_space(8.0);
            let rows = [
                ("⌘F  or  /", "Search"),
                ("⌘[  or  Option ←", "Back"),
                ("?", "Show this help"),
                ("Space", "Play / pause"),
                ("←  →", "Seek 10 seconds"),
                ("↑  ↓", "Volume"),
                ("F  ·  double-click", "Full screen"),
                ("M", "Mute"),
                ("Z  ·  X", "Subtitles earlier / later"),
                ("N", "Next episode"),
                ("Esc", "Leave full screen / close player"),
            ];
            egui::Grid::new("keys").num_columns(2).spacing(vec2(24.0, 10.0)).show(ui, |ui| {
                for (k, what) in rows {
                    ui.label(RichText::new(k).font(semibold(14.0)).color(ACCENT2));
                    ui.label(what);
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            button(ui, Kind::Secondary, None, "Close").clicked()
        });
        if modal.inner || modal.should_close() {
            self.help = false;
        }
    }

    fn toasts_ui(&mut self, ui: &Ui) {
        const SHOW: f32 = 3.2;
        self.toasts.retain(|(_, t)| t.elapsed().as_secs_f32() < SHOW);
        let screen = ui.ctx().content_rect();
        let mut y = screen.bottom() - 28.0;
        for (i, (msg, t)) in self.toasts.iter().enumerate().rev() {
            let age = t.elapsed().as_secs_f32();
            let alpha = (age / 0.15).min(1.0).min((SHOW - age) / 0.4).clamp(0.0, 1.0);
            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new(("toast", i))));
            let galley = painter.layout_no_wrap(msg.clone(), semibold(14.0), TEXT.gamma_multiply(alpha));
            let size = galley.size() + vec2(40.0, 22.0);
            let rect = Rect::from_min_size(pos2(screen.center().x - size.x / 2.0, y - size.y + (1.0 - alpha) * 10.0), size);
            painter.rect_filled(rect.translate(vec2(0.0, 4.0)), 12, Color32::from_black_alpha((90.0 * alpha) as u8));
            painter.rect_filled(rect, 12, RAISED.gamma_multiply(alpha));
            painter.circle_filled(pos2(rect.left() + 18.0, rect.center().y), 4.0, OK.gamma_multiply(alpha));
            painter.galley(pos2(rect.left() + 30.0, rect.center().y - galley.size().y / 2.0), galley, TEXT);
            y -= size.y + 8.0;
        }
        if !self.toasts.is_empty() {
            ui.ctx().request_repaint();
        }
    }
}

fn nav_item(ui: &mut Ui, icon: Icon, name: &str, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(64.0, 58.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || active, 0.15);
    let p = ui.painter();
    if active {
        p.rect_filled(rect, 12, ACCENT.gamma_multiply(0.22));
        p.rect_filled(Rect::from_center_size(pos2(rect.left() - 10.0, rect.center().y), vec2(4.0, 24.0)), 2, ACCENT_HI);
    } else if t > 0.0 {
        p.rect_filled(rect, 12, Color32::from_white_alpha((12.0 * t) as u8));
    }
    let color = if active { ACCENT2 } else { MUTED.lerp_to_gamma(TEXT, t) };
    paint_icon(p, Rect::from_center_size(rect.center() - vec2(0.0, 9.0), vec2(22.0, 22.0)), icon, color);
    let text_color = if active { TEXT } else { color };
    p.text(rect.center() + vec2(0.0, 15.0), Align2::CENTER_CENTER, name, semibold(11.0), text_color);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// The search field; returns a query once typing pauses or Enter is pressed.
fn search_box(ui: &mut Ui, text: &mut String, edited: &mut Option<Instant>, width: f32) -> Option<String> {
    let mut out = None;
    let id = Id::new(SEARCH_ID);
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    let stroke = if focused { egui::Stroke::new(1.5, ACCENT) } else { egui::Stroke::new(1.0, LINE) };
    Frame::new().fill(PANEL2).stroke(stroke).corner_radius(20).inner_margin(Margin { left: 12, right: 6, top: 2, bottom: 2 }).show(
        ui,
        |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(18.0, 34.0), Sense::hover());
                paint_icon(ui.painter(), Rect::from_center_size(r.center(), vec2(18.0, 18.0)), Icon::Search, MUTED);
                let clear_w = if text.is_empty() { 0.0 } else { 30.0 };
                let edit = ui.add(
                    TextEdit::singleline(text)
                        .id(id)
                        .frame(egui::Frame::NONE)
                        .hint_text(RichText::new("Search movies & series…   ⌘F").color(MUTED))
                        .font(egui::FontId::proportional(15.0))
                        .desired_width(ui.available_width() - clear_w)
                        .margin(Margin::symmetric(4, 6)),
                );
                if edit.changed() {
                    *edited = Some(Instant::now());
                }
                if !text.is_empty() && icon_button(ui, Icon::Close, 26.0, "Clear").clicked() {
                    text.clear();
                    *edited = None;
                    edit.request_focus();
                }
                if edit.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                    text.clear();
                    edit.surrender_focus();
                }
                let submit = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if submit && edited.is_none() && text.trim().chars().count() > 1 {
                    out = Some(text.trim().to_string());
                }
                if let Some(t) = *edited {
                    let wait = Duration::from_millis(450).saturating_sub(t.elapsed());
                    if submit || wait.is_zero() {
                        *edited = None;
                        let q = text.trim().to_string();
                        if q.chars().count() > 1 {
                            out = Some(q);
                        }
                    } else {
                        ui.ctx().request_repaint_after(wait);
                    }
                }
            });
        },
    );
    out
}
