//! Application state and the per-frame orchestration: which view is up, what
//! background work has landed, and the shared caches the views draw from.

use crate::audio::AudioEngine;
use crate::decode::Frame;
use crate::media::{self, Imported};
use crate::model::*;
use crate::preview::Preview;
use crate::projects;
use crate::stock::{StockPhoto, StockVideo};
use crate::store::Store;
use crate::ui::widgets::{self, ButtonStyle, Icon};
use crate::ui::*;
use crate::views;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};

/// Where an export should write. `MW_SAVE_PATH` is an automation seam: when it
/// is set the save dialog is skipped, which is what lets the export pipeline be
/// exercised without a human at the keyboard.
fn save_target(default_name: &str, filter: &str, ext: &str) -> Option<String> {
    if let Ok(dir) = std::env::var("MW_SAVE_PATH") {
        let dir = dir.trim_end_matches(['/', '\\']).to_string();
        return Some(format!("{}/{}", dir, default_name));
    }
    rfd::FileDialog::new()
        .add_filter(filter, &[ext])
        .set_file_name(default_name)
        .save_file()
        .map(|p| p.to_string_lossy().to_string())
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum View {
    Home,
    Editor,
}

#[derive(Clone, Copy, PartialEq, Eq)]
/// Audio lives in the same bin as video and images: splitting it out meant
/// hunting through two places for one project's material.
pub enum LibraryTab {
    Media,
    Text,
    Stock,
}

/// The Stock tab's own sub-tabs, mirroring the Basic/Mask/Color/Transitions
/// split in the clip inspector rather than stacking every kind in one
/// endless scroll.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StockTab {
    Photo,
    Video,
}

/// The four workspace panes. Any of them can be pulled out of the main window
/// into a window of its own; the rest close the gap it leaves behind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    Library,
    Player,
    Inspector,
    Timeline,
}

impl Panel {
    pub fn title(self) -> &'static str {
        match self {
            Panel::Library => "Media",
            Panel::Player => "Player",
            Panel::Inspector => "Clip settings",
            Panel::Timeline => "Timeline",
        }
    }

    /// Sensible size for the pane's own window.
    pub fn window_size(self) -> (f64, f64) {
        match self {
            Panel::Library => (360.0, 620.0),
            Panel::Player => (900.0, 620.0),
            Panel::Inspector => (330.0, 720.0),
            Panel::Timeline => (1100.0, 340.0),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SideTab {
    Basic,
    Mask,
    Color,
    Transitions,
    Audio,
}

impl SideTab {
    pub fn label(self) -> &'static str {
        match self {
            SideTab::Basic => "Basic",
            SideTab::Mask => "Mask",
            SideTab::Color => "Color",
            SideTab::Transitions => "Transitions",
            SideTab::Audio => "Audio",
        }
    }
}

/// Which tabs a clip offers — video and images share the same four; only
/// visual clips get color grading, since it has nothing to act on otherwise.
pub fn tabs_for(kind: ClipKind) -> Vec<SideTab> {
    match kind {
        ClipKind::Audio => vec![SideTab::Basic, SideTab::Audio],
        ClipKind::Text => vec![SideTab::Basic, SideTab::Transitions],
        _ => vec![SideTab::Basic, SideTab::Mask, SideTab::Color, SideTab::Transitions],
    }
}

/// Which end of a clip the transitions gallery is currently editing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TransSlot {
    In,
    Out,
}

/// A window-chrome action requested from the custom titlebar this frame. The
/// host drains it after render, since only it holds the winit `Window`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowAction {
    Minimize,
    ToggleMaximize,
    Close,
}

/// A chrome action requested from a detached pane's own titlebar this frame.
/// The host drains it after render, since only it holds the pane's `Window`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaneAction {
    None,
    Minimize,
    ToggleMaximize,
    Close,
    Drag,
}

/// Height of the custom titlebar that replaces the OS one.
pub const TITLEBAR_H: f32 = 34.0;

pub enum Modal {
    None,
    NewProject { name: String },
    DeleteProject { id: String, name: String },
    Exporting { progress: f32 },
}

/// Width of a dialog action button. Both buttons in a pair share it: unequal
/// widths were what made the Cancel/Create row look ragged.
const MODAL_BTN_W: f32 = 96.0;
const MODAL_BTN_GAP: f32 = 10.0;

/// Splits an action row into a right-aligned pair, returning
/// `(primary, secondary)` — primary flush with the dialog's right padding.
fn action_pair(row: Rect) -> (Rect, Rect) {
    let (primary, rest) = row.split_right(MODAL_BTN_W);
    let (_, rest) = rest.split_right(MODAL_BTN_GAP);
    let (secondary, _) = rest.split_right(MODAL_BTN_W);
    (primary, secondary)
}

/// Finds the earliest video or image clip in a project and decodes a
/// thumbnail from the asset it references — a stand-in for a real rendered
/// cover frame, cheap enough to compute for every tile on the home screen.
fn first_visual_frame(project: &Project) -> Option<Frame> {
    let clip = project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| matches!(c.kind, ClipKind::Video | ClipKind::Image))
        .min_by(|a, b| a.start.total_cmp(&b.start))?;
    let asset = project.assets.iter().find(|a| a.id == clip.asset_id)?;
    let is_image = asset.kind == AssetKind::Image;
    let aspect = asset.width.max(1) as f32 / asset.height.max(1) as f32;
    crate::decode::thumbnail(&asset.path, asset.duration, is_image, media::THUMB_W, aspect)
}

/// Results of background work, delivered to the UI thread.
pub enum Job {
    Asset(Result<Imported, String>),
    /// A stock item (photo, video or audio) that finished downloading, plus
    /// where to drop it. All three end up as an `Imported` the same way, so
    /// one variant serves all of them.
    StockAsset(Result<Imported, String>, Option<(Id, f32)>),
    /// `bool` is whether this appends to existing results (Load more) or
    /// replaces them (a fresh search).
    StockPhotoResults(Result<Vec<StockPhoto>, String>, bool),
    StockVideoResults(Result<Vec<StockVideo>, String>, bool),
    /// A thumbnail rebuilt for an asset that was already in the project file.
    Thumbnail(Id, Frame),
    StockThumbnail(u64, Frame),
    StockVideoThumbnail(u64, Frame),
    ProjectThumbnail(String, Frame),
    ExportProgress(f32),
    ExportDone(Result<String, String>),
}

pub struct App {
    pub store: Store,
    pub view: View,
    pub preview: Preview,
    /// Opened on the first play: initializing WASAPI costs real memory, and an
    /// editing session that never hits play should not pay it.
    pub audio: Option<AudioEngine>,
    /// Small RGBA thumbnails for the media bin, keyed by asset id.
    pub thumbs: HashMap<Id, Frame>,
    pub stock_thumbs: HashMap<u64, Frame>,
    pub stock_video_thumbs: HashMap<u64, Frame>,
    /// A preview frame for each project shown on the home screen, keyed by
    /// project id. Populated lazily as tiles scroll into view.
    pub project_thumbs: HashMap<String, Frame>,
    project_thumb_pending: std::collections::HashSet<String>,

    pub library_tab: LibraryTab,
    pub library_scroll: f32,
    pub text_scroll: f32,
    pub stock_tab: StockTab,
    pub stock_scroll: f32,
    pub stock_query: String,
    pub stock_photos: Vec<StockPhoto>,
    pub stock_videos: Vec<StockVideo>,
    pub stock_photo_page: u32,
    pub stock_video_page: u32,
    pub stock_photo_loading: bool,
    pub stock_video_loading: bool,
    /// Whether the most recent page for each kind came back full — a proxy
    /// for "there might be more", since Pexels doesn't return a total count
    /// worth trusting across both media kinds' differing response shapes.
    pub stock_photo_more: bool,
    pub stock_video_more: bool,

    pub side_tab: SideTab,
    pub side_scroll: f32,
    /// Which end of the selected clip the transitions gallery edits.
    pub trans_slot: TransSlot,
    /// Clock the transitions gallery reads to animate the hovered/selected
    /// tile — a shared, ever-running phase rather than a per-tile timestamp,
    /// so no state needs resetting when the hover moves to a new tile.
    pub gallery_clock: std::time::Instant,

    pub home_scroll: f32,
    pub modal: Modal,
    pub project_name: String,

    /// Panes currently living in their own window.
    pub detached: Vec<Panel>,
    /// Pop-out requests raised by this frame's UI; the host drains them,
    /// because only it can create windows. The point carried alongside each
    /// panel is where inside its header the user grabbed it, so the host can
    /// spawn the new window with that exact point under the cursor — a tab
    /// torn free stays pinned to the pointer instead of popping up elsewhere.
    pub detach_requests: Vec<(Panel, (f32, f32))>,

    /// Live sizes for the three resizable splits (library width, inspector
    /// width, timeline height); the player always takes what's left.
    pub library_w: f32,
    pub inspector_w: f32,
    pub timeline_h: f32,

    pub timeline: views::timeline::TimelineState,
    pub toast: Option<(String, bool, std::time::Instant)>,
    pub ffmpeg_ok: bool,
    pub export_progress: Option<f32>,

    /// Expected playhead for the running audio stream, used to notice a seek.
    audio_anchor: Option<(f32, std::time::Instant)>,

    /// A window-chrome action clicked in the custom titlebar this frame;
    /// taken and applied by the host right after `frame()` returns.
    pub window_action: Option<WindowAction>,
    /// Set for one frame when the user pressed down on the empty part of the
    /// titlebar, so the host can start an OS-native window move.
    pub want_drag_window: bool,
    /// Fed in by the host each frame (only it can ask winit), so the
    /// titlebar can show a maximize or restore icon correctly.
    pub window_maximized: bool,

    tx: Sender<Job>,
    rx: Receiver<Job>,
    pending_jobs: usize,
}

impl App {
    pub fn new() -> App {
        let (tx, rx) = channel();
        App {
            store: Store::new(),
            view: View::Home,
            preview: Preview::new(),
            audio: None,
            thumbs: HashMap::new(),
            stock_thumbs: HashMap::new(),
            stock_video_thumbs: HashMap::new(),
            project_thumbs: HashMap::new(),
            project_thumb_pending: std::collections::HashSet::new(),
            library_tab: LibraryTab::Media,
            library_scroll: 0.0,
            text_scroll: 0.0,
            stock_tab: StockTab::Photo,
            stock_scroll: 0.0,
            stock_query: "nature".into(),
            stock_photos: Vec::new(),
            stock_videos: Vec::new(),
            stock_photo_page: 0,
            stock_video_page: 0,
            stock_photo_loading: false,
            stock_video_loading: false,
            stock_photo_more: false,
            stock_video_more: false,
            side_tab: SideTab::Basic,
            trans_slot: TransSlot::In,
            gallery_clock: std::time::Instant::now(),
            side_scroll: 0.0,
            home_scroll: 0.0,
            modal: Modal::None,
            project_name: String::new(),
            detached: Vec::new(),
            detach_requests: Vec::new(),
            library_w: LIBRARY_W,
            inspector_w: INSPECTOR_W,
            timeline_h: TIMELINE_H,
            timeline: views::timeline::TimelineState::default(),
            toast: None,
            ffmpeg_ok: crate::ffmpeg::available(),
            export_progress: None,
            audio_anchor: None,
            window_action: None,
            want_drag_window: false,
            window_maximized: false,
            tx,
            rx,
            pending_jobs: 0,
        }
    }

    /// One-line state summary for the scripted test harness.
    #[cfg(feature = "harness")]
    pub fn debug_state(&self) -> String {
        let sel = self.store.selection.map(|s| s.clip_id.to_string()).unwrap_or_else(|| "-".into());
        let lanes: Vec<String> = self
            .store
            .project
            .tracks
            .iter()
            .map(|t| {
                let clips: Vec<String> = t
                    .clips
                    .iter()
                    .map(|c| format!("{}@{:.2}+{:.2}", c.id, c.start, c.duration))
                    .collect();
                format!("{:?}[{}]", t.kind, clips.join(","))
            })
            .collect();
        let detail = self
            .store
            .selected_clip()
            .map(|c| {
                format!(
                    " start={:.2} dur={:.2} trim={:.2} vol={:.2} x={:.3} w={:.3} text=\"{}\"",
                    c.start, c.duration, c.trim_in, c.volume, c.x, c.width, c.text
                )
            })
            .unwrap_or_default();
        format!(
            "view={} playhead={:.3} playing={} sel={} tab={:?} mask_edit={} lanes={}{}",
            if self.view == View::Home { "home" } else { "editor" },
            self.store.playhead,
            self.store.playing,
            sel,
            self.side_tab as u8,
            self.store.mask_editing,
            lanes.join(" | "),
            detail
        )
    }

    // ------------------------------------------------------------- detaching

    pub fn is_detached(&self, panel: Panel) -> bool {
        self.detached.contains(&panel)
    }

    /// Asks for a pane to be pulled out, `grab` being where inside its own
    /// header rect the drag started. The pane is marked detached straight
    /// away so this frame already reflows; the host opens the window next,
    /// positioned so `grab` lands under the cursor and picks the drag up.
    pub fn request_detach(&mut self, panel: Panel, grab: (f32, f32)) {
        if self.is_detached(panel) {
            return;
        }
        self.detached.push(panel);
        self.detach_requests.push((panel, grab));
    }

    pub fn take_detach_requests(&mut self) -> Vec<(Panel, (f32, f32))> {
        std::mem::take(&mut self.detach_requests)
    }

    /// Closing a detached window puts the pane back where it came from.
    pub fn redock(&mut self, panel: Panel) {
        self.detached.retain(|p| *p != panel);
    }

    pub fn frame_panel(&mut self, ctx: &mut Ctx, panel: Panel, maximized: bool) -> PaneAction {
        let full = ctx.painter.full_rect();
        ctx.painter.clear(BG_APP);
        let (titlebar, body) = full.split_top(TITLEBAR_H);
        let action = self.draw_pane_titlebar(ctx, titlebar, panel, maximized);
        views::editor::draw_panel(self, ctx, panel, body);
        self.draw_toast(ctx, body);
        action
    }

    /// The titlebar drawn atop a detached pane's own window — same visual
    /// style and button set as the main window's (`draw_titlebar`): minimize,
    /// maximize/restore, close. Only the close button's meaning differs — it
    /// redocks the pane rather than destroying it, so there is nothing to
    /// "restore" from that.
    fn draw_pane_titlebar(&mut self, ctx: &mut Ctx, r: Rect, panel: Panel, maximized: bool) -> PaneAction {
        const BTN_W: f32 = 46.0;
        ctx.painter.rect(r, BG_PANEL);
        ctx.painter.hline(r.x, r.right(), r.bottom() - 1.0, BORDER);

        let close_r = Rect::new(r.right() - BTN_W, r.y, BTN_W, r.h);
        let max_r = Rect::new(close_r.x - BTN_W, r.y, BTN_W, r.h);
        let min_r = Rect::new(max_r.x - BTN_W, r.y, BTN_W, r.h);

        let (min_hover, min_clicked) = ctx.interact(id_of("pane-min", panel as u64), min_r);
        let (max_hover, max_clicked) = ctx.interact(id_of("pane-max", panel as u64), max_r);
        let (close_hover, close_clicked) = ctx.interact(id_of("pane-close", panel as u64), close_r);
        if min_hover || max_hover || close_hover {
            ctx.cursor = Cursor::Hand;
        }
        if min_hover {
            ctx.painter.rect(min_r, BG_ELEV);
        }
        if max_hover {
            ctx.painter.rect(max_r, BG_ELEV);
        }
        if close_hover {
            ctx.painter.rect(close_r, rgb(0xe81123));
        }

        let icon = |r: Rect| Rect::new(r.cx() - 8.0, r.cy() - 8.0, 16.0, 16.0);
        widgets::draw_icon(ctx, Icon::Minimize, icon(min_r), TEXT_2);
        let max_icon = if maximized { Icon::Restore } else { Icon::Maximize };
        widgets::draw_icon(ctx, max_icon, icon(max_r), TEXT_2);
        widgets::draw_icon(ctx, Icon::Close, icon(close_r), if close_hover { TEXT } else { TEXT_2 });

        let drag_area = Rect::new(r.x, r.y, min_r.x - r.x, r.h);
        if close_clicked {
            PaneAction::Close
        } else if max_clicked {
            PaneAction::ToggleMaximize
        } else if min_clicked {
            PaneAction::Minimize
        } else if ctx.mouse_pressed && ctx.hovered(drag_area) {
            PaneAction::Drag
        } else if ctx.double_click && ctx.hovered(drag_area) {
            PaneAction::ToggleMaximize
        } else {
            PaneAction::None
        }
    }

    /// The app's own titlebar, replacing the OS one: an app label on the
    /// left, minimize/maximize/close on the right, and — everywhere else in
    /// the strip — a region that starts an OS-native window move on press,
    /// so the window still drags and double-click-to-maximize like a normal
    /// one despite `with_decorations(false)`.
    fn draw_titlebar(&mut self, ctx: &mut Ctx, r: Rect) {
        const BTN_W: f32 = 46.0;
        ctx.painter.rect(r, BG_PANEL);
        ctx.painter.hline(r.x, r.right(), r.bottom() - 1.0, BORDER);

        let close_r = Rect::new(r.right() - BTN_W, r.y, BTN_W, r.h);
        let max_r = Rect::new(close_r.x - BTN_W, r.y, BTN_W, r.h);
        let min_r = Rect::new(max_r.x - BTN_W, r.y, BTN_W, r.h);

        let (min_hover, min_clicked) = ctx.interact(id_of("win-min", 0), min_r);
        let (max_hover, max_clicked) = ctx.interact(id_of("win-max", 0), max_r);
        let (close_hover, close_clicked) = ctx.interact(id_of("win-close", 0), close_r);
        if min_hover || max_hover || close_hover {
            ctx.cursor = Cursor::Hand;
        }
        if min_hover {
            ctx.painter.rect(min_r, BG_ELEV);
        }
        if max_hover {
            ctx.painter.rect(max_r, BG_ELEV);
        }
        if close_hover {
            ctx.painter.rect(close_r, rgb(0xe81123));
        }

        let icon = |r: Rect| Rect::new(r.cx() - 8.0, r.cy() - 8.0, 16.0, 16.0);
        widgets::draw_icon(ctx, Icon::Minimize, icon(min_r), TEXT_2);
        let max_icon = if self.window_maximized { Icon::Restore } else { Icon::Maximize };
        widgets::draw_icon(ctx, max_icon, icon(max_r), TEXT_2);
        widgets::draw_icon(ctx, Icon::Close, icon(close_r), if close_hover { TEXT } else { TEXT_2 });

        if min_clicked {
            self.window_action = Some(WindowAction::Minimize);
        }
        if max_clicked {
            self.window_action = Some(WindowAction::ToggleMaximize);
        }
        if close_clicked {
            self.window_action = Some(WindowAction::Close);
        }

        let drag_area = Rect::new(r.x, r.y, min_r.x - r.x, r.h);
        if ctx.mouse_pressed && ctx.hovered(drag_area) {
            self.want_drag_window = true;
        }
        if ctx.double_click && ctx.hovered(drag_area) {
            self.window_action = Some(WindowAction::ToggleMaximize);
        }
    }

    pub fn shutdown(&mut self) {
        if let Some(a) = &mut self.audio {
            a.stop();
        }
        self.preview.release();
        self.store.save_now();
        crate::exporter::cleanup_temp();
    }

    /// True while something on screen is moving, so the host polls at 60 Hz
    /// instead of idling.
    pub fn wants_animation(&self) -> bool {
        self.store.playing
            || self.pending_jobs > 0
            || self.export_progress.is_some()
            || self.toast.is_some()
            || self.preview.is_dragging()
            // The transitions gallery keeps its hovered/selected tile looping;
            // gated on a selection existing, so an idle "no clip picked" state
            // (project settings shown instead) still costs nothing.
            || (self.side_tab == SideTab::Transitions && self.store.selection.is_some())
    }

    pub fn toast(&mut self, message: impl Into<String>, error: bool) {
        self.toast = Some((message.into(), error, std::time::Instant::now()));
    }

    pub fn spawn<F>(&mut self, work: F)
    where
        F: FnOnce(Sender<Job>) + Send + 'static,
    {
        let tx = self.tx.clone();
        self.pending_jobs += 1;
        std::thread::Builder::new()
            .name("mw-job".into())
            .spawn(move || work(tx))
            .ok();
    }

    // ------------------------------------------------------------------ frame

    pub fn frame(&mut self, ctx: &mut Ctx) {
        self.drain_jobs();
        self.want_drag_window = false;
        let full = ctx.painter.full_rect();
        ctx.painter.clear(BG_APP);
        ctx.mark("clear");

        let (titlebar, full) = full.split_top(TITLEBAR_H);
        self.draw_titlebar(ctx, titlebar);

        match self.view {
            View::Home => views::home::draw(self, ctx, full),
            View::Editor => views::editor::draw(self, ctx, full),
        }

        self.draw_modal(ctx, full);
        self.draw_toast(ctx, full);
        self.sync_audio();
        self.store.maybe_save();
    }

    fn drain_jobs(&mut self) {
        while let Ok(job) = self.rx.try_recv() {
            match job {
                Job::Asset(result) => {
                    self.pending_jobs = self.pending_jobs.saturating_sub(1);
                    match result {
                        Ok(imported) => self.accept_asset(imported, None),
                        Err(e) => self.toast(format!("Import failed: {e}"), true),
                    }
                }
                Job::StockAsset(result, drop_at) => {
                    self.pending_jobs = self.pending_jobs.saturating_sub(1);
                    match result {
                        Ok(imported) => self.accept_asset(imported, drop_at),
                        Err(e) => self.toast(format!("Stock import failed: {e}"), true),
                    }
                }
                Job::StockPhotoResults(result, append) => {
                    self.pending_jobs = self.pending_jobs.saturating_sub(1);
                    self.stock_photo_loading = false;
                    match result {
                        Ok(list) => {
                            self.stock_photo_more = list.len() >= 24;
                            self.fetch_stock_thumbnails(&list);
                            if append {
                                self.stock_photos.extend(list);
                            } else {
                                self.stock_photos = list;
                            }
                        }
                        Err(e) => {
                            if !append {
                                self.stock_photos.clear();
                            }
                            self.toast(format!("Photo search failed: {e}"), true);
                        }
                    }
                }
                Job::StockVideoResults(result, append) => {
                    self.pending_jobs = self.pending_jobs.saturating_sub(1);
                    self.stock_video_loading = false;
                    match result {
                        Ok(list) => {
                            self.stock_video_more = list.len() >= 24;
                            self.fetch_stock_video_thumbnails(&list);
                            if append {
                                self.stock_videos.extend(list);
                            } else {
                                self.stock_videos = list;
                            }
                        }
                        Err(e) => {
                            if !append {
                                self.stock_videos.clear();
                            }
                            self.toast(format!("Video search failed: {e}"), true);
                        }
                    }
                }
                Job::Thumbnail(id, frame) => {
                    self.pending_jobs = self.pending_jobs.saturating_sub(1);
                    self.thumbs.insert(id, frame);
                }
                Job::StockThumbnail(id, frame) => {
                    self.pending_jobs = self.pending_jobs.saturating_sub(1);
                    self.stock_thumbs.insert(id, frame);
                }
                Job::StockVideoThumbnail(id, frame) => {
                    self.pending_jobs = self.pending_jobs.saturating_sub(1);
                    self.stock_video_thumbs.insert(id, frame);
                }
                Job::ProjectThumbnail(id, frame) => {
                    self.pending_jobs = self.pending_jobs.saturating_sub(1);
                    self.project_thumb_pending.remove(&id);
                    self.project_thumbs.insert(id, frame);
                }
                Job::ExportProgress(p) => {
                    self.export_progress = Some(p);
                    if let Modal::Exporting { progress } = &mut self.modal {
                        *progress = p;
                    }
                }
                Job::ExportDone(result) => {
                    self.pending_jobs = self.pending_jobs.saturating_sub(1);
                    self.export_progress = None;
                    self.modal = Modal::None;
                    crate::exporter::cleanup_temp();
                    match result {
                        Ok(path) => self.toast(format!("Exported to {path}"), false),
                        Err(e) => self.toast(e, true),
                    }
                }
            }
        }
    }

    fn accept_asset(&mut self, imported: Imported, drop_at: Option<(Id, f32)>) {
        let Imported { asset, thumbnail } = imported;
        let id = asset.id;
        let (aw, ah, kind, duration) = (asset.width, asset.height, asset.kind, asset.duration);
        if let Some(t) = thumbnail {
            self.thumbs.insert(id, t);
        }
        self.store.add_asset(asset);
        if let Some((track_id, start)) = drop_at {
            let s = self.store.project.settings;
            let (w, h) = fit_clip_size(aw, ah, s.width, s.height);
            let dur = if kind == AssetKind::Image { 3.0 } else { duration };
            let clip = Clip::new_media(kind.into(), id, start, dur, w, h);
            self.store.add_clip(track_id, clip);
        }
    }

    /// Adds a clip to the end of the track that suits its kind.
    pub fn append_asset(&mut self, asset_id: Id) {
        let Some(asset) = self.store.asset(asset_id).cloned() else { return };
        let kind: ClipKind = asset.kind.into();
        let track_id = self.store.track_for(kind, None);
        let start = self
            .store
            .track(track_id)
            .map(|t| t.clips.iter().map(|c| c.end()).fold(0.0_f32, f32::max))
            .unwrap_or(0.0);
        let s = self.store.project.settings;
        let (w, h) = fit_clip_size(asset.width, asset.height, s.width, s.height);
        let dur = if asset.kind == AssetKind::Image { 3.0 } else { asset.duration };
        let clip = Clip::new_media(kind, asset_id, start, dur, w, h);
        self.store.add_clip(track_id, clip);
    }

    pub fn add_text_clip(&mut self, track_id: Option<Id>, start: Option<f32>, preset: TextPreset) {
        let track_id = self.store.track_for(ClipKind::Text, track_id);
        let clip = Clip::new_text(start.unwrap_or(self.store.playhead), preset);
        self.store.add_clip(track_id, clip);
    }

    /// Opens a file picker for a `.cube` 3D LUT and points the given clip's
    /// color grade at it. Synchronous like `import_files`'s dialog — loading
    /// the LUT itself happens lazily, cached by path, the first time a frame
    /// actually needs it (see `Preview::lut_for`).
    pub fn pick_lut(&mut self, clip_id: Id) {
        let Some(path) = rfd::FileDialog::new().add_filter("LUT", &["cube"]).pick_file() else { return };
        let path = path.to_string_lossy().to_string();
        self.store.snapshot_forced();
        if let Some(c) = self.store.clip_mut(clip_id) {
            c.color_grade.lut_path = path;
        }
        self.store.touch();
    }

    pub fn import_files(&mut self) {
        let files = rfd::FileDialog::new()
            .add_filter("Media", &media::all_extensions())
            .pick_files();
        let Some(files) = files else { return };
        for path in files {
            let path = path.to_string_lossy().to_string();
            self.spawn(move |tx| {
                let _ = tx.send(Job::Asset(media::build_asset(&path)));
            });
        }
    }

    /// Fresh search: all three sections reset to page 1 and their old results
    /// are replaced, so hitting Enter always shows a coherent set for the new
    /// query rather than mixing pages from two different searches.
    pub fn run_stock_search(&mut self) {
        self.stock_photo_page = 1;
        self.stock_video_page = 1;
        self.search_stock_photos(false);
        self.search_stock_videos(false);
    }

    /// Runs just the photo search — the Photo and Video stock tabs are now
    /// split like the inspector's own tabs, each with its own search action,
    /// rather than one shared search firing both at once.
    pub fn run_stock_photo_search(&mut self) {
        self.stock_photo_page = 1;
        self.search_stock_photos(false);
    }

    pub fn run_stock_video_search(&mut self) {
        self.stock_video_page = 1;
        self.search_stock_videos(false);
    }

    pub fn load_more_stock_photos(&mut self) {
        self.stock_photo_page += 1;
        self.search_stock_photos(true);
    }

    pub fn load_more_stock_videos(&mut self) {
        self.stock_video_page += 1;
        self.search_stock_videos(true);
    }

    fn search_stock_photos(&mut self, append: bool) {
        let key = crate::stock::api_key();
        if key.is_empty() {
            return;
        }
        let (query, page) = (self.stock_query.clone(), self.stock_photo_page.max(1));
        self.stock_photo_loading = true;
        self.spawn(move |tx| {
            let _ = tx.send(Job::StockPhotoResults(crate::stock::search(&key, &query, page), append));
        });
    }

    fn search_stock_videos(&mut self, append: bool) {
        let key = crate::stock::api_key();
        if key.is_empty() {
            return;
        }
        let (query, page) = (self.stock_query.clone(), self.stock_video_page.max(1));
        self.stock_video_loading = true;
        self.spawn(move |tx| {
            let _ = tx.send(Job::StockVideoResults(crate::stock::search_videos(&key, &query, page), append));
        });
    }

    /// Pulls each result's preview image into the cache and decodes it. Results
    /// that are already cached cost one file read.
    fn fetch_stock_thumbnails(&mut self, list: &[StockPhoto]) {
        for photo in list {
            if self.stock_thumbs.contains_key(&photo.id) {
                continue;
            }
            let (url, id) = (photo.thumbnail.clone(), photo.id);
            let aspect = photo.width.max(1) as f32 / photo.height.max(1) as f32;
            self.spawn(move |tx| {
                if let Ok(path) = crate::stock::download_thumb(&url, id) {
                    if let Some(frame) = crate::decode::thumbnail(&path, 0.0, true, media::THUMB_W, aspect) {
                        let _ = tx.send(Job::StockThumbnail(id, frame));
                        return;
                    }
                }
                let _ = tx.send(Job::StockThumbnail(id, crate::decode::Frame::blank(1, 1)));
            });
        }
    }

    /// Same idea as `fetch_stock_thumbnails`, for the video section's own
    /// still-frame previews (Pexels gives one JPG still per video, no
    /// separate small/medium sizes the way photos have).
    fn fetch_stock_video_thumbnails(&mut self, list: &[StockVideo]) {
        for video in list {
            if self.stock_video_thumbs.contains_key(&video.id) {
                continue;
            }
            let (url, id) = (video.thumbnail.clone(), video.id);
            let aspect = video.width.max(1) as f32 / video.height.max(1) as f32;
            self.spawn(move |tx| {
                if let Ok(path) = crate::stock::download_thumb(&url, id) {
                    if let Some(frame) = crate::decode::thumbnail(&path, 0.0, true, media::THUMB_W, aspect) {
                        let _ = tx.send(Job::StockVideoThumbnail(id, frame));
                        return;
                    }
                }
                let _ = tx.send(Job::StockVideoThumbnail(id, crate::decode::Frame::blank(1, 1)));
            });
        }
    }

    pub fn import_stock_photo(&mut self, photo: &StockPhoto, drop_at: Option<(Id, f32)>) {
        let (url, id) = (photo.url.clone(), photo.id);
        self.spawn(move |tx| {
            let result = crate::stock::download(&url, id).and_then(|path| media::build_asset(&path));
            let _ = tx.send(Job::StockAsset(result, drop_at));
        });
    }

    pub fn import_stock_video(&mut self, video: &StockVideo, drop_at: Option<(Id, f32)>) {
        let (url, id) = (video.url.clone(), video.id);
        self.spawn(move |tx| {
            let result = crate::stock::download_video(&url, id).and_then(|path| media::build_asset(&path));
            let _ = tx.send(Job::StockAsset(result, drop_at));
        });
    }

    /// Kicks off a background decode of a home-screen project tile's preview,
    /// unless one is already cached or already in flight. The project file
    /// itself is only a few KB, so loading it just to find the first visual
    /// clip's asset path is cheap next to spawning ffmpeg.
    pub fn ensure_project_thumb(&mut self, id: &str) {
        if self.project_thumbs.contains_key(id) || self.project_thumb_pending.contains(id) {
            return;
        }
        self.project_thumb_pending.insert(id.to_string());
        let id = id.to_string();
        self.spawn(move |tx| {
            let project = projects::load(&id);
            let frame = first_visual_frame(&project).unwrap_or_else(|| Frame::blank(1, 1));
            let _ = tx.send(Job::ProjectThumbnail(id, frame));
        });
    }

    // ----------------------------------------------------------------- export

    pub fn export_video(&mut self) {
        if self.store.total_duration() <= 0.0 {
            self.toast("Add at least one clip to the timeline first.", true);
            return;
        }
        let Some(out) = save_target("export.mp4", "Video", "mp4") else { return };
        let tracks = crate::exporter::build_tracks(&self.store);
        let s = self.store.project.settings;
        self.modal = Modal::Exporting { progress: 0.0 };
        self.export_progress = Some(0.0);
        self.spawn(move |tx| {
            let progress_tx = tx.clone();
            let result = crate::ffmpeg::export_video(&tracks, s.width, s.height, s.fps, &out, move |p| {
                let _ = progress_tx.send(Job::ExportProgress(p));
            });
            let _ = tx.send(Job::ExportDone(result.map(|_| out)));
        });
    }

    pub fn export_frame(&mut self) {
        let Some(out) = save_target("frame.png", "Image", "png") else { return };
        let tracks = crate::exporter::build_frame_tracks(&self.store, self.store.playhead);
        let s = self.store.project.settings;
        self.spawn(move |tx| {
            let result = crate::ffmpeg::export_frame(&tracks, s.width, s.height, &out);
            let _ = tx.send(Job::ExportDone(result.map(|_| out)));
        });
    }

    // --------------------------------------------------------------- playback

    pub fn toggle_play(&mut self) {
        if self.store.playing {
            self.preview.pause(&mut self.store);
            self.stop_audio();
        } else {
            self.preview.play(&mut self.store);
            self.start_audio();
        }
    }

    pub fn stop_playback(&mut self) {
        if self.store.playing {
            self.preview.pause(&mut self.store);
            self.stop_audio();
        }
    }

    fn stop_audio(&mut self) {
        if let Some(a) = &mut self.audio {
            a.stop();
        }
        self.audio_anchor = None;
    }

    fn start_audio(&mut self) {
        if !self.ffmpeg_ok {
            return;
        }
        let tracks = crate::exporter::build_audio_tracks(&self.store);
        let has_audio = tracks.iter().flatten().any(|c| c.has_audio && c.volume > 1e-4);
        if !has_audio {
            self.audio_anchor = None;
            return;
        }
        let engine = self.audio.get_or_insert_with(AudioEngine::new);
        if !engine.is_available() {
            return;
        }
        engine.start(&tracks, self.store.playhead);
        self.audio_anchor = Some((self.store.playhead, std::time::Instant::now()));
    }

    /// Restarts the audio stream when the playhead was moved out from under it.
    fn sync_audio(&mut self) {
        if !self.store.playing {
            if self.audio_anchor.is_some() {
                self.stop_audio();
            }
            return;
        }
        let Some((start, at)) = self.audio_anchor else { return };
        let expected = start + at.elapsed().as_secs_f32() * self.store.playback_rate;
        if (self.store.playhead - expected).abs() > 0.30 {
            self.start_audio();
        }
    }

    // ----------------------------------------------------------------- chrome

    pub fn open_project(&mut self, id: &str) {
        let project = projects::load(id);
        self.store.load(id, project);
        self.project_name = projects::name_of(id);
        self.thumbs.clear();
        self.preview.release();
        self.view = View::Editor;
        self.rebuild_thumbnails();
    }

    /// Thumbnails are not persisted, so they are rebuilt in the background when
    /// a project opens. Keeping them out of the project file is what stops
    /// project files from ballooning.
    fn rebuild_thumbnails(&mut self) {
        let wanted: Vec<(Id, String, f32, bool, u32, u32)> = self
            .store
            .project
            .assets
            .iter()
            .filter(|a| a.kind != AssetKind::Audio)
            .map(|a| (a.id, a.path.clone(), a.duration, a.kind == AssetKind::Image, a.width, a.height))
            .collect();
        for (id, path, duration, is_image, w, h) in wanted {
            let aspect = w.max(1) as f32 / h.max(1) as f32;
            let tx = self.tx.clone();
            self.pending_jobs += 1;
            std::thread::Builder::new()
                .name("mw-thumb".into())
                .spawn(move || {
                    if let Some(frame) = crate::decode::thumbnail(&path, duration, is_image, media::THUMB_W, aspect) {
                        let _ = tx.send(Job::Thumbnail(id, frame));
                    } else {
                        let _ = tx.send(Job::Thumbnail(id, crate::decode::Frame::blank(1, 1)));
                    }
                })
                .ok();
        }
    }

    pub fn go_home(&mut self) {
        self.stop_playback();
        self.store.save_now();
        self.preview.release();
        self.view = View::Home;
    }

    fn draw_toast(&mut self, ctx: &mut Ctx, full: Rect) {
        let Some((message, error, at)) = self.toast.clone() else { return };
        if at.elapsed().as_secs_f32() > 4.5 {
            self.toast = None;
            return;
        }
        let w = (ctx.painter.text_width(&message, FS_BODY, Weight::Regular) + 32.0).min(full.w - 40.0);
        let r = Rect::new(full.cx() - w / 2.0, full.bottom() - 64.0, w, 36.0);
        ctx.painter.round_rect(r, R_MD, if error { rgb(0x3a1f1e) } else { BG_ELEV });
        ctx.painter
            .stroke_round_rect(r, R_MD, if error { DANGER } else { BORDER }, 1.0);
        ctx.painter.label(
            r,
            &message,
            FS_BODY,
            Weight::Regular,
            if error { DANGER } else { TEXT },
            Align::Center,
        );
    }

    fn draw_modal(&mut self, ctx: &mut Ctx, full: Rect) {
        if matches!(self.modal, Modal::None) {
            return;
        }
        ctx.painter.rect(full, rgba(0x000000, 150));
        // The scrim swallows clicks so the view behind cannot be operated.
        if ctx.mouse_pressed {
            ctx.press_consumed = true;
        }

        let box_w = 380.0;
        let box_h = 176.0;
        let r = Rect::new(
            (full.cx() - box_w / 2.0).round(),
            (full.cy() - box_h / 2.0).round(),
            box_w,
            box_h,
        );
        ctx.painter.round_rect(r, R_LG, BG_PANEL);
        ctx.painter.stroke_round_rect(r, R_LG, BORDER, 1.0);
        let inner = r.inset(20.0, 18.0);

        let mut close = false;
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::None => {}
            Modal::NewProject { mut name } => {
                let (title, rest) = inner.split_top(26.0);
                ctx.painter
                    .label(title, "New project", FS_TITLE, Weight::Bold, TEXT, Align::Left);
                let (_, rest) = rest.split_top(14.0);
                let (label, rest) = rest.split_top(18.0);
                widgets::field_label(ctx, label, "Name");
                let (field, rest) = rest.split_top(FIELD_H);
                let id = id_of("modal-name", 0);
                if ctx.focus != id && !ctx.any_popup_open() {
                    ctx.edit_text = name.clone();
                    ctx.set_focus(id, name.len());
                }
                if let Some(v) = widgets::text_field(ctx, id, field, &name, "Project name") {
                    name = v;
                }
                let (actions, _) = rest.split_bottom(FIELD_H);
                let (create, cancel) = action_pair(actions);
                let submit = ctx.keys.contains(&Key::Enter)
                    || widgets::button(ctx, id_of("modal-create", 0), create, "Create", ButtonStyle::Primary);
                let cancelled = ctx.keys.contains(&Key::Escape)
                    || widgets::button(ctx, id_of("modal-cancel", 0), cancel, "Cancel", ButtonStyle::Normal);
                if submit {
                    ctx.focus = 0;
                    let meta = projects::create(&name);
                    self.open_project(&meta.id);
                    close = true;
                } else if cancelled {
                    ctx.focus = 0;
                    close = true;
                } else {
                    self.modal = Modal::NewProject { name };
                }
            }
            Modal::DeleteProject { id, name } => {
                let (title, rest) = inner.split_top(26.0);
                ctx.painter.label(
                    title,
                    &format!("Delete \"{name}\"?"),
                    FS_TITLE,
                    Weight::Bold,
                    TEXT,
                    Align::Left,
                );
                let (_, rest) = rest.split_top(10.0);
                let (hint, rest) = rest.split_top(20.0);
                widgets::hint(ctx, hint, "This cannot be undone.");
                let (actions, _) = rest.split_bottom(FIELD_H);
                let (delete, cancel) = action_pair(actions);
                if widgets::button(ctx, id_of("modal-del", 0), delete, "Delete", ButtonStyle::Danger) {
                    projects::delete(&id);
                    close = true;
                } else if widgets::button(ctx, id_of("modal-cancel", 0), cancel, "Cancel", ButtonStyle::Normal)
                    || ctx.keys.contains(&Key::Escape)
                {
                    close = true;
                } else {
                    self.modal = Modal::DeleteProject { id, name };
                }
            }
            Modal::Exporting { progress } => {
                let (title, rest) = inner.split_top(26.0);
                ctx.painter
                    .label(title, "Exporting", FS_TITLE, Weight::Bold, TEXT, Align::Left);
                let (_, rest) = rest.split_top(24.0);
                let (bar, rest) = rest.split_top(8.0);
                ctx.painter.round_rect(bar, 4.0, BG_ELEV);
                ctx.painter
                    .round_rect(Rect::new(bar.x, bar.y, bar.w * progress.clamp(0.0, 1.0), bar.h), 4.0, ACCENT);
                let (_, pct) = rest.split_top(12.0);
                ctx.painter.label(
                    Rect::new(pct.x, pct.y, pct.w, 20.0),
                    &format!("{}%", (progress * 100.0).round() as i32),
                    FS_BODY,
                    Weight::Regular,
                    TEXT_2,
                    Align::Center,
                );
                self.modal = Modal::Exporting { progress };
            }
        }
        if close {
            self.modal = Modal::None;
        }
    }
}
