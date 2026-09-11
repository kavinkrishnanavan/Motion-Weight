//! The timeline: lanes, clips, the ruler and the playhead, plus every drag that
//! moves, trims, splits or re-layers a clip.

use crate::app::App;
use crate::model::*;
use crate::ui::widgets::{self, Icon};
use crate::ui::*;

const RULER_H: f32 = 26.0;
const GUTTER_W: f32 = 92.0;
const LANE_H: f32 = 52.0;
const LANE_GAP: f32 = 6.0;
/// Empty strips above and below the lanes; dropping into one opens a new layer.
const EDGE_ZONE: f32 = 18.0;
const TRIM_W: f32 = 7.0;
/// Magnet distance for clip edges against the playhead and other clips.
const SNAP_PX: f32 = 8.0;
/// How close a drag must be to an existing lane to land in it rather than
/// opening a new one.
const LANE_SNAP_PX: f32 = 18.0;
/// Strip along the bottom of the timeline reserved for the horizontal scrollbar.
const HSCROLL_H: f32 = 12.0;
/// Zoom limits, shared by the wheel gesture and the zoom bar so both agree.
const ZOOM_MIN: f32 = 8.0;
const ZOOM_MAX: f32 = 600.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DragMode {
    Move,
    TrimStart,
    TrimEnd,
}

pub struct ClipDrag {
    clip_id: Id,
    mode: DragMode,
    /// Offset from the clip's start to where it was grabbed, in seconds.
    grab_dt: f32,
    base: Clip,
    /// Set when the drop would open a new lane at this index.
    pending_lane: Option<usize>,
}

#[derive(Default)]
pub struct TimelineState {
    drag: Option<ClipDrag>,
    seeking: bool,
    pub scroll_x: f32,
    pub scroll_y: f32,
    /// Snap guide to draw this frame, in seconds.
    snap_guide: Option<f32>,
}

pub fn draw(app: &mut App, ctx: &mut Ctx, r: Rect) {
    ctx.painter.rect(r, BG_PANEL);
    ctx.painter.hline(r.x, r.right(), r.y, BORDER);

    let (toolbar, rest) = r.split_top(32.0);
    draw_toolbar(app, ctx, toolbar);

    // The horizontal scrollbar owns a strip along the bottom, so a long project
    // can be reached by dragging rather than only by wheeling.
    let (hbar, rest) = rest.split_bottom(HSCROLL_H);
    let (ruler, lanes_area) = rest.split_top(RULER_H);
    let track_area = Rect::new(
        lanes_area.x + GUTTER_W,
        lanes_area.y,
        lanes_area.w - GUTTER_W,
        lanes_area.h,
    );

    app.timeline.snap_guide = None;
    handle_zoom_and_scroll(app, ctx, track_area);

    draw_ruler(app, ctx, Rect::new(track_area.x, ruler.y, track_area.w, RULER_H));
    draw_lanes(app, ctx, lanes_area, track_area);
    draw_playhead(app, ctx, Rect::new(track_area.x, ruler.y, track_area.w, rest.h));

    // Seeking is handled after the lanes so a clip drag wins the press.
    seek_interaction(app, ctx, Rect::new(track_area.x, ruler.y, track_area.w, RULER_H));

    draw_hscroll(app, ctx, Rect::new(track_area.x, hbar.y, track_area.w, hbar.h));
    // A detached pane's own window is already carved into this shape at the
    // OS level; see the matching comment in inspector.rs.
    if !app.is_detached(crate::app::Panel::Timeline) {
        ctx.painter.round_corners(r, R_LG, BG_APP);
    }
}

/// Total timeline width in pixels at the current zoom, with a little run-off
/// past the last clip so there is somewhere to drop things.
fn content_w(app: &App) -> f32 {
    app.store.total_duration() * app.store.px_per_sec + 200.0
}

fn draw_hscroll(app: &mut App, ctx: &mut Ctx, r: Rect) {
    ctx.painter.rect(r, BG_PANEL);
    let content = content_w(app);
    let overflow = (content - r.w).max(0.0);
    app.store.scroll_x = app.store.scroll_x.clamp(0.0, overflow);
    if overflow <= 0.5 {
        return;
    }
    let track = Rect::new(r.x, r.y + 3.0, r.w, r.h - 6.0);
    let thumb_w = (track.w * r.w / content).max(32.0);
    let t = app.store.scroll_x / overflow;
    let thumb = Rect::new(track.x + (track.w - thumb_w) * t, track.y, thumb_w, track.h);

    let id = id_of("tl-hscroll", 0);
    let (hovered, _) = ctx.interact(id, track);
    let active = ctx.is_active(id);
    if hovered || active {
        ctx.cursor = Cursor::Hand;
    }
    ctx.painter.round_rect(track, track.h / 2.0, with_alpha(BG_ELEV, 0.6));
    ctx.painter.round_rect(
        thumb,
        track.h / 2.0,
        if hovered || active { TEXT_3 } else { with_alpha(TEXT_3, 0.7) },
    );
    if active {
        let t = ((ctx.mouse.0 - track.x - thumb_w / 2.0) / (track.w - thumb_w).max(1.0)).clamp(0.0, 1.0);
        app.store.scroll_x = t * overflow;
    }
}

fn draw_toolbar(app: &mut App, ctx: &mut Ctx, r: Rect) {
    ctx.painter.rect(r, BG_PANEL);
    let mut x = r.x + 10.0;
    let has_selection = app.store.selection.is_some();

    if widgets::icon_button(
        ctx,
        id_of("tl-split", 0),
        Rect::new(x, r.cy() - 12.0, 26.0, 24.0),
        Icon::Scissors,
        "Split at playhead (S)",
        false,
    ) && has_selection
    {
        let at = app.store.playhead;
        if let Some(sel) = app.store.selection {
            app.store.split_clip(sel.clip_id, at);
        }
    }
    x += 32.0;
    if widgets::icon_button(
        ctx,
        id_of("tl-cutleft", 0),
        Rect::new(x, r.cy() - 12.0, 26.0, 24.0),
        Icon::CutLeft,
        "Cut left of playhead (Q)",
        false,
    ) && has_selection
    {
        let at = app.store.playhead;
        if let Some(sel) = app.store.selection {
            app.store.trim_to(sel.clip_id, at, true);
        }
    }
    x += 32.0;
    if widgets::icon_button(
        ctx,
        id_of("tl-cutright", 0),
        Rect::new(x, r.cy() - 12.0, 26.0, 24.0),
        Icon::CutRight,
        "Cut right of playhead (W)",
        false,
    ) && has_selection
    {
        let at = app.store.playhead;
        if let Some(sel) = app.store.selection {
            app.store.trim_to(sel.clip_id, at, false);
        }
    }
    // Delete sits last, separated from the three cutting tools it is easy to
    // hit by accident next to.
    x += 40.0;
    if widgets::icon_button(
        ctx,
        id_of("tl-del", 0),
        Rect::new(x, r.cy() - 12.0, 26.0, 24.0),
        Icon::Trash,
        "Delete clip (Del)",
        false,
    ) && has_selection
    {
        if let Some(sel) = app.store.selection {
            app.store.remove_clip(sel.clip_id);
        }
    }

    // The toolbar's dead middle stretch — past the cutting tools, short of the
    // zoom controls — doubles as the tear-off handle, since every other inch
    // of this row is already spoken for by a real button.
    let handle = Rect::new(x + 36.0, r.y, (r.right() - 234.0 - (x + 36.0)).max(0.0), r.h);
    crate::views::editor::drag_handle(app, ctx, handle, crate::app::Panel::Timeline);
    draw_zoom_bar(app, ctx, Rect::new(r.right() - 224.0, r.y, 178.0, r.h));
}

/// Zoom-out / slider / zoom-in, driving `px_per_sec` on a logarithmic scale so
/// the knob travels evenly across the whole 8..600 range.
fn draw_zoom_bar(app: &mut App, ctx: &mut Ctx, r: Rect) {
    let lo = ZOOM_MIN.ln();
    let hi = ZOOM_MAX.ln();
    let set = |app: &mut App, v: f32| {
        let old = app.store.px_per_sec;
        let new = v.clamp(ZOOM_MIN, ZOOM_MAX);
        if (new - old).abs() < 1e-4 {
            return;
        }
        // Hold the leftmost visible moment still, so zooming scales the view
        // in place instead of jumping somewhere else in the project.
        app.store.px_per_sec = new;
        app.store.scroll_x = (app.store.scroll_x * new / old).max(0.0);
    };

    if widgets::icon_button(
        ctx,
        id_of("tl-zoomout", 0),
        Rect::new(r.x, r.cy() - 11.0, 22.0, 22.0),
        Icon::ZoomOut,
        "Zoom out",
        false,
    ) {
        let v = app.store.px_per_sec / 1.4;
        set(app, v);
    }
    if widgets::icon_button(
        ctx,
        id_of("tl-zoomin", 0),
        Rect::new(r.right() - 22.0, r.cy() - 11.0, 22.0, 22.0),
        Icon::ZoomIn,
        "Zoom in",
        false,
    ) {
        let v = app.store.px_per_sec * 1.4;
        set(app, v);
    }

    let track = Rect::new(r.x + 28.0, r.y, r.w - 56.0, r.h);
    let t = (app.store.px_per_sec.ln() - lo) / (hi - lo);
    if let Some(nt) = widgets::slider(ctx, id_of("tl-zoom", 0), track, t, 0.0, 1.0) {
        set(app, (lo + nt * (hi - lo)).exp());
    }
}

fn handle_zoom_and_scroll(app: &mut App, ctx: &mut Ctx, area: Rect) {
    if ctx.wheel.abs() < 1e-4 || !ctx.hovered(area) || ctx.any_popup_open() {
        return;
    }
    if ctx.mods.ctrl {
        // Zoom about the pointer, so the frame under the cursor stays put.
        let t_at_cursor = time_at(app, area, ctx.mouse.0);
        let factor = if ctx.wheel > 0.0 { 1.18 } else { 1.0 / 1.18 };
        app.store.px_per_sec = (app.store.px_per_sec * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        let new_x = area.x + (t_at_cursor * app.store.px_per_sec) - app.store.scroll_x;
        app.store.scroll_x += new_x - ctx.mouse.0;
    } else {
        app.store.scroll_x -= ctx.wheel * 60.0;
    }
    let span = content_w(app);
    app.store.scroll_x = app.store.scroll_x.clamp(0.0, (span - area.w).max(0.0));
}

fn x_of(app: &App, area: Rect, t: f32) -> f32 {
    area.x + t * app.store.px_per_sec - app.store.scroll_x
}

fn time_at(app: &App, area: Rect, x: f32) -> f32 {
    ((x - area.x + app.store.scroll_x) / app.store.px_per_sec).max(0.0)
}

fn draw_ruler(app: &mut App, ctx: &mut Ctx, r: Rect) {
    ctx.painter.rect(r, BG_PANEL_2);
    ctx.painter.hline(r.x, r.right(), r.bottom() - 1.0, BORDER);
    let prev = ctx.painter.push_clip(r);

    // Pick a tick spacing that keeps labels roughly 70px apart.
    let target = 70.0 / app.store.px_per_sec;
    let step = [0.1f32, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0]
        .into_iter()
        .find(|s| *s >= target)
        .unwrap_or(600.0);

    let first = (time_at(app, r, r.x) / step).floor() * step;
    let last = time_at(app, r, r.right());
    let mut t = first;
    while t <= last + step {
        let x = x_of(app, r, t).round();
        if x >= r.x - 1.0 {
            ctx.painter.vline(x, r.bottom() - 7.0, r.bottom() - 1.0, BORDER);
            let label = if step >= 1.0 {
                format!("{}:{:02}", (t / 60.0) as i64, (t % 60.0) as i64)
            } else {
                format!("{:.1}s", t)
            };
            ctx.painter.label(
                Rect::new(x + 4.0, r.y, 60.0, r.h - 6.0),
                &label,
                FS_SMALL,
                Weight::Regular,
                TEXT_3,
                Align::Left,
            );
        }
        t += step;
    }
    ctx.painter.set_clip(prev);
}

fn seek_interaction(app: &mut App, ctx: &mut Ctx, ruler: Rect) {
    let hovered = ctx.hovered(ruler);
    if hovered {
        ctx.cursor = Cursor::ResizeH;
    }
    if hovered && ctx.mouse_pressed && !ctx.press_consumed {
        app.timeline.seeking = true;
        ctx.press_consumed = true;
        // Grabbing the playhead stops playback there rather than letting it run on.
        app.stop_playback();
    }
    if app.timeline.seeking {
        if !ctx.mouse_down {
            app.timeline.seeking = false;
        } else {
            let mut t = time_at(app, ruler, ctx.mouse.0);
            // The playhead snaps to clip starts and ends.
            if let Some(snapped) = snap_to_edges(app, t, None) {
                t = snapped;
                app.timeline.snap_guide = Some(t);
            }
            app.store.set_playhead(t);
        }
    }
}

/// Vertical placement of each lane, plus the drop zones around them.
fn lane_rects(app: &App, area: Rect) -> Vec<Rect> {
    let mut out = Vec::new();
    let mut y = area.y + EDGE_ZONE - app.timeline.scroll_y;
    for _ in 0..app.store.project.tracks.len() {
        out.push(Rect::new(area.x, y, area.w, LANE_H));
        y += LANE_H + LANE_GAP;
    }
    out
}

fn draw_lanes(app: &mut App, ctx: &mut Ctx, full: Rect, area: Rect) {
    let lanes = lane_rects(app, area);
    let content_h = lanes.len() as f32 * (LANE_H + LANE_GAP) + EDGE_ZONE * 2.0;
    let mut scroll = app.timeline.scroll_y;
    widgets::scroll(ctx, id_of("tl-vscroll", 0), full, content_h, &mut scroll);
    if scroll != app.timeline.scroll_y {
        app.timeline.scroll_y = scroll;
    }
    let lanes = lane_rects(app, area);

    let prev = ctx.painter.push_clip(full);

    if app.store.project.tracks.is_empty() {
        let msg = Rect::new(area.x + 24.0, area.cy() - 10.0, area.w - 48.0, 20.0);
        widgets::hint(
            ctx,
            msg,
            "Drag media here from the library. Drop near a lane to join it, or above and below to open a new layer.",
        );
    }

    // Lane backgrounds and gutter labels.
    for (i, lane) in lanes.iter().enumerate() {
        let Some(track) = app.store.project.tracks.get(i) else { continue };
        ctx.painter.round_rect(*lane, R_SM, BG_PANEL_2);
        let gutter = Rect::new(full.x, lane.y, GUTTER_W, lane.h);
        ctx.painter.round_rect(gutter.inset(6.0, 0.0), R_SM, BG_PANEL_2);
        let (icon, label) = match track.kind.map(track_family) {
            Some(TrackFamily::Audio) => (Icon::Audio, "Audio"),
            Some(TrackFamily::Text) => (Icon::Text, "Text"),
            _ => (Icon::Media, "Video"),
        };
        widgets::draw_icon(ctx, icon, Rect::new(gutter.x + 12.0, lane.cy() - 9.0, 18.0, 18.0), TEXT_3);
        ctx.painter.label(
            Rect::new(gutter.x + 34.0, lane.y, gutter.w - 40.0, lane.h),
            label,
            FS_SMALL,
            Weight::Regular,
            TEXT_3,
            Align::Left,
        );
    }

    // Clips, painted lane by lane, clipped to the track area so a scrolled
    // clip never spills over the lane labels in the gutter.
    let track_ids: Vec<Id> = app.store.project.tracks.iter().map(|t| t.id).collect();
    let clipped = ctx.painter.push_clip(area.intersect(&full));
    for (i, lane) in lanes.iter().enumerate() {
        if lane.bottom() < full.y || lane.y > full.bottom() {
            continue;
        }
        let clips: Vec<Clip> = app.store.project.tracks[i].clips.clone();
        for clip in &clips {
            draw_clip(app, ctx, area, *lane, track_ids[i], clip);
        }
    }
    ctx.painter.set_clip(clipped);

    // A drag in flight paints its target: either a highlighted lane or the
    // outline of the new lane that dropping would create.
    if let Some(drag) = &app.timeline.drag {
        if let Some(index) = drag.pending_lane {
            let y = if index >= lanes.len() {
                lanes.last().map(|l| l.bottom() + LANE_GAP).unwrap_or(area.y + EDGE_ZONE)
            } else {
                lanes[index].y - LANE_GAP - LANE_H
            };
            let marker = Rect::new(area.x, y, area.w, LANE_H);
            ctx.painter.stroke_round_rect(marker, R_SM, ACCENT, 1.5);
            ctx.painter.label(
                marker,
                "New layer",
                FS_SMALL,
                Weight::Regular,
                ACCENT_HI,
                Align::Center,
            );
        }
    }

    update_drag(app, ctx, area, &lanes);
    handle_library_drop(app, ctx, area, &lanes);

    if let Some(t) = app.timeline.snap_guide {
        let x = x_of(app, area, t).round();
        ctx.painter.vline(x, full.y, full.bottom(), WARN);
    }

    ctx.painter.set_clip(prev);
}

fn draw_clip(app: &mut App, ctx: &mut Ctx, area: Rect, lane: Rect, track_id: Id, clip: &Clip) {
    let x0 = x_of(app, area, clip.start);
    let w = clip.duration * app.store.px_per_sec;
    let r = Rect::new(x0, lane.y + 3.0, w.max(3.0), lane.h - 6.0);
    if r.right() < area.x || r.x > area.right() {
        return;
    }

    let selected = app.store.selection.map(|s| s.clip_id) == Some(clip.id);
    let id = id_of("tl-clip", clip.id);
    // `interact` marks the press as consumed once it takes this clip, so the
    // "was anything else already handling it" test has to be read first.
    let free = !ctx.press_consumed;
    let (hovered, _) = ctx.interact(id, r);

    let base = theme::clip_color(clip.kind);
    ctx.painter.round_rect(r, R_SM, if hovered { mix(base, TEXT, 0.08) } else { base });

    // Audio clips (and videos that carry audio) show their waveform.
    if let Some(asset) = app.store.asset(clip.asset_id) {
        if !asset.waveform.is_empty() && r.w > 8.0 {
            draw_waveform(ctx, r, area, &asset.waveform, clip, asset.duration);
        }
    }

    let label = match clip.kind {
        ClipKind::Text => clip.text.clone(),
        _ => app
            .store
            .asset(clip.asset_id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| clip.kind.label().to_string()),
    };
    if r.w > 24.0 {
        let prev = ctx.painter.push_clip(r);
        ctx.painter.label(
            Rect::new(r.x + 7.0, r.y, r.w - 14.0, 18.0),
            &label,
            FS_SMALL,
            Weight::Regular,
            TEXT,
            Align::Left,
        );
        ctx.painter.set_clip(prev);
    }

    ctx.painter
        .stroke_round_rect(r, R_SM, if selected { ACCENT_HI } else { rgba(0x000000, 90) }, if selected { 1.8 } else { 1.0 });

    // Trim grips, shown once the clip is wide enough to grab them.
    let trimmable = r.w > TRIM_W * 3.0;
    let start_grip = Rect::new(r.x, r.y, TRIM_W, r.h);
    let end_grip = Rect::new(r.right() - TRIM_W, r.y, TRIM_W, r.h);
    if trimmable && (hovered || selected) {
        for grip in [start_grip, end_grip] {
            ctx.painter.round_rect(grip.inset(1.5, 8.0), 2.0, rgba(0xffffff, 130));
        }
    }

    if hovered {
        ctx.cursor = if trimmable && (ctx.hovered(start_grip) || ctx.hovered(end_grip)) {
            Cursor::ResizeH
        } else {
            Cursor::Move
        };
    }

    if hovered && ctx.mouse_pressed && free && app.timeline.drag.is_none() {
        app.store.select(Some(Selection { track_id, clip_id: clip.id }));
        app.store.begin_history_group();
        app.store.suspend_track_cleanup = true;
        let mode = if trimmable && ctx.hovered(start_grip) {
            DragMode::TrimStart
        } else if trimmable && ctx.hovered(end_grip) {
            DragMode::TrimEnd
        } else {
            DragMode::Move
        };
        app.timeline.drag = Some(ClipDrag {
            clip_id: clip.id,
            mode,
            grab_dt: time_at(app, area, ctx.mouse.0) - clip.start,
            base: clip.clone(),
            pending_lane: None,
        });
    }
}

/// Bar pitch in on-screen pixels: 2px of bar plus 1px of gap, so the waveform
/// reads the same whether the clip is 80px or 8000px wide. Fixing the pitch to
/// the screen rather than to the peak table is what keeps the bars evenly and
/// closely spaced at every zoom level instead of spreading out as you zoom in.
const WAVE_PITCH: f32 = 3.0;
const WAVE_BAR_W: f32 = 2.0;

fn draw_waveform(ctx: &mut Ctx, r: Rect, view: Rect, peaks: &[f32], clip: &Clip, asset_duration: f32) {
    let bars = ((r.w / WAVE_PITCH) as usize).max(1);
    let span = asset_duration.max(0.01);
    let mid = r.cy() + 4.0;
    let half = (r.h - 22.0).max(4.0) / 2.0;
    let color = rgba(0xffffff, 110);
    let n = peaks.len() as f32;
    // Zoomed in, a clip can be tens of thousands of pixels wide; only the bars
    // that actually land inside the visible strip are worth computing.
    let first = (((view.x - r.x) / WAVE_PITCH).floor().max(0.0)) as usize;
    let last = (((view.right() - r.x) / WAVE_PITCH).ceil().max(0.0) as usize).min(bars);
    for i in first..last {
        let frac = i as f32 / bars as f32;
        let next = (i + 1) as f32 / bars as f32;
        // Each bar covers a slice of source time; take the loudest peak in that
        // slice so zooming out summarizes rather than randomly samples.
        let t0 = (clip.trim_in + frac * clip.duration) / span;
        let t1 = (clip.trim_in + next * clip.duration) / span;
        let a = ((t0 * n) as usize).min(peaks.len() - 1);
        let b = ((t1 * n).ceil() as usize).clamp(a + 1, peaks.len());
        let peak = peaks[a..b].iter().cloned().fold(0.0f32, f32::max);
        let h = (peak * half).max(0.5);
        let x = r.x + i as f32 * WAVE_PITCH;
        ctx.painter.rect(Rect::new(x, mid - h, WAVE_BAR_W, h * 2.0), color);
    }
}

fn update_drag(app: &mut App, ctx: &mut Ctx, area: Rect, lanes: &[Rect]) {
    let Some(drag) = &app.timeline.drag else { return };
    let (clip_id, mode, grab_dt) = (drag.clip_id, drag.mode, drag.grab_dt);
    let base = drag.base.clone();

    if !ctx.mouse_down {
        // Landing in a gutter opens the new lane the marker promised.
        let pending = drag.pending_lane;
        app.timeline.drag = None;
        app.store.suspend_track_cleanup = false;
        if let Some(index) = pending {
            let track = app.store.add_track(Some(base.kind), Some(index));
            let start = app.store.clip(clip_id).map(|c| c.start).unwrap_or(base.start);
            app.store.move_clip(clip_id, track, start);
        }
        app.store.cleanup_empty_tracks();
        app.store.end_history_group();
        return;
    }

    let cursor_t = time_at(app, area, ctx.mouse.0);

    match mode {
        DragMode::Move => {
            let mut start = (cursor_t - grab_dt).max(0.0);
            if let Some(snapped) = snap_clip_start(app, start, base.duration, clip_id) {
                app.timeline.snap_guide = Some(snapped);
                start = snapped;
            }

            // Vertical: land in the nearest lane, or open a new one.
            let (target, pending) = resolve_lane(app, lanes, ctx.mouse.1, base.kind, area);
            if let Some(track_id) = target {
                app.store.move_clip(clip_id, track_id, start);
            } else if let Some(clip) = app.store.clip_mut(clip_id) {
                clip.start = start;
            }
            if let Some(drag) = &mut app.timeline.drag {
                drag.pending_lane = pending;
            }
            app.store.touch();
        }
        DragMode::TrimStart => {
            let (prev_end, _) = app.store.neighbor_bounds(clip_id);
            let max_start = base.start + base.duration - 0.1;
            let mut start = cursor_t.clamp(prev_end, max_start);
            if let Some(snapped) = snap_to_edges(app, start, Some(clip_id)) {
                if snapped >= prev_end && snapped <= max_start {
                    start = snapped;
                    app.timeline.snap_guide = Some(start);
                }
            }
            // Trimming the head also advances into the source.
            let delta = start - base.start;
            if let Some(clip) = app.store.clip_mut(clip_id) {
                clip.start = start;
                clip.duration = base.duration - delta;
                if clip.kind != ClipKind::Text {
                    clip.trim_in = (base.trim_in + delta).max(0.0);
                }
            }
            app.store.touch();
        }
        DragMode::TrimEnd => {
            let (_, next_start) = app.store.neighbor_bounds(clip_id);
            let source_limit = source_end_limit(app, &base);
            let mut end = cursor_t.clamp(base.start + 0.1, next_start.min(source_limit));
            if let Some(snapped) = snap_to_edges(app, end, Some(clip_id)) {
                if snapped >= base.start + 0.1 && snapped <= next_start.min(source_limit) {
                    end = snapped;
                    app.timeline.snap_guide = Some(end);
                }
            }
            if let Some(clip) = app.store.clip_mut(clip_id) {
                clip.duration = end - clip.start;
            }
            app.store.touch();
        }
    }
}

/// The furthest a clip's end can go before it runs out of source material.
/// Images and text loop indefinitely, so they have no limit.
fn source_end_limit(app: &App, clip: &Clip) -> f32 {
    match clip.kind {
        ClipKind::Video | ClipKind::Audio => app
            .store
            .asset(clip.asset_id)
            .map(|a| clip.start - clip.trim_in + a.duration)
            .unwrap_or(f32::INFINITY),
        _ => f32::INFINITY,
    }
}

/// Nearest existing lane the clip may land in, or the index at which dropping
/// would open a new one.
fn resolve_lane(
    app: &App,
    lanes: &[Rect],
    y: f32,
    kind: ClipKind,
    area: Rect,
) -> (Option<Id>, Option<usize>) {
    let mut best: Option<(f32, usize)> = None;
    for (i, lane) in lanes.iter().enumerate() {
        let dist = if y < lane.y {
            lane.y - y
        } else if y > lane.bottom() {
            y - lane.bottom()
        } else {
            0.0
        };
        if best.map(|(d, _)| dist < d).unwrap_or(true) {
            best = Some((dist, i));
        }
    }

    if let Some((dist, i)) = best {
        let track = &app.store.project.tracks[i];
        if dist <= LANE_SNAP_PX && app.store.track_accepts(track, kind) {
            return (Some(track.id), None);
        }
        // Outside the magnet: which side of the nearest lane are we on?
        let lane = lanes[i];
        let index = if y < lane.cy() { i } else { i + 1 };
        return (None, Some(index));
    }
    let _ = area;
    (None, Some(0))
}

/// Snaps a clip's start so that either of its edges lands on a magnet point.
fn snap_clip_start(app: &App, start: f32, duration: f32, exclude: Id) -> Option<f32> {
    let tolerance = SNAP_PX / app.store.px_per_sec;
    let mut best: Option<(f32, f32)> = None;
    for target in magnets(app, Some(exclude)) {
        for (edge, offset) in [(start, 0.0), (start + duration, duration)] {
            let dist = (edge - target).abs();
            if dist <= tolerance && best.map(|(d, _)| dist < d).unwrap_or(true) {
                best = Some((dist, (target - offset).max(0.0)));
            }
        }
    }
    best.map(|(_, v)| v)
}

/// Snaps a single time value to a magnet point.
fn snap_to_edges(app: &App, t: f32, exclude: Option<Id>) -> Option<f32> {
    let tolerance = SNAP_PX / app.store.px_per_sec;
    magnets(app, exclude)
        .into_iter()
        .map(|m| (( t - m).abs(), m))
        .filter(|(d, _)| *d <= tolerance)
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, m)| m)
}

/// Everything a dragged edge sticks to: zero, the playhead, and every other
/// clip's start and end.
fn magnets(app: &App, exclude: Option<Id>) -> Vec<f32> {
    let mut out = vec![0.0, app.store.playhead];
    for track in &app.store.project.tracks {
        for clip in &track.clips {
            if Some(clip.id) == exclude {
                continue;
            }
            out.push(clip.start);
            out.push(clip.end());
        }
    }
    out
}

fn handle_library_drop(app: &mut App, ctx: &mut Ctx, area: Rect, lanes: &[Rect]) {
    let Some(payload) = ctx.drag_payload.clone() else { return };
    // The gutter counts as part of the drop target: aiming at a lane's label
    // and having nothing happen is the kind of inconsistency that makes the
    // whole gesture feel unreliable.
    let over = ctx.hovered(Rect::new(area.x - GUTTER_W, area.y, area.w + GUTTER_W, area.h));
    let (kind, duration) = match &payload {
        DragPayload::Text => (ClipKind::Text, 3.0),
        DragPayload::StockPhoto(_) => (ClipKind::Image, 3.0),
        DragPayload::StockVideo(i) => (ClipKind::Video, app.stock_videos.get(*i).map(|v| v.duration).unwrap_or(3.0)),
        DragPayload::StockAudio(i) => (ClipKind::Audio, app.stock_audio.get(*i).map(|a| a.duration).unwrap_or(3.0)),
        DragPayload::Asset(id) => match app.store.asset(*id) {
            Some(a) => (
                ClipKind::from(a.kind),
                if a.kind == AssetKind::Image { 3.0 } else { a.duration },
            ),
            None => (ClipKind::Video, 3.0),
        },
    };

    if !over {
        return;
    }
    let (target, pending) = resolve_lane(app, lanes, ctx.mouse.1, kind, area);
    let mut start = time_at(app, area, ctx.mouse.0);
    if let Some(snapped) = snap_to_edges(app, start, None) {
        start = snapped;
        app.timeline.snap_guide = Some(start);
    }

    // Show where the clip would land.
    let ghost_y = match (target, pending) {
        (Some(track_id), _) => app
            .store
            .project
            .tracks
            .iter()
            .position(|t| t.id == track_id)
            .and_then(|i| lanes.get(i))
            .map(|l| l.y),
        (None, Some(index)) => Some(if index >= lanes.len() {
            lanes.last().map(|l| l.bottom() + LANE_GAP).unwrap_or(area.y + EDGE_ZONE)
        } else {
            lanes[index].y - LANE_GAP - LANE_H
        }),
        _ => None,
    };
    if let Some(y) = ghost_y {
        // The ghost is the real width the clip will land at, so what you see
        // during the drag is what you get on release.
        let w = (duration * app.store.px_per_sec).clamp(6.0, area.w);
        let ghost = Rect::new(x_of(app, area, start), y + 3.0, w, LANE_H - 6.0);
        ctx.painter.round_rect(ghost, R_SM, with_alpha(theme::clip_color(kind), 0.6));
        ctx.painter.stroke_round_rect(ghost, R_SM, ACCENT, 1.5);
    }

    if !ctx.mouse_released {
        return;
    }

    let track_id = match target {
        Some(id) => id,
        None => app.store.add_track(Some(kind), pending),
    };

    match payload {
        DragPayload::Text => app.add_text_clip(Some(track_id), Some(start)),
        DragPayload::Asset(asset_id) => {
            let Some(asset) = app.store.asset(asset_id).cloned() else { return };
            let s = app.store.project.settings;
            let (w, h) = fit_clip_size(asset.width, asset.height, s.width, s.height);
            let duration = if asset.kind == AssetKind::Image { 3.0 } else { asset.duration };
            let clip = Clip::new_media(asset.kind.into(), asset_id, start, duration, w, h);
            app.store.add_clip(track_id, clip);
        }
        DragPayload::StockPhoto(index) => {
            if let Some(photo) = app.stock_photos.get(index).cloned() {
                app.import_stock_photo(&photo, Some((track_id, start)));
            }
        }
        DragPayload::StockVideo(index) => {
            if let Some(video) = app.stock_videos.get(index).cloned() {
                app.import_stock_video(&video, Some((track_id, start)));
            }
        }
        DragPayload::StockAudio(index) => {
            if let Some(audio) = app.stock_audio.get(index).cloned() {
                app.import_stock_audio(&audio, Some((track_id, start)));
            }
        }
    }
    ctx.drag_payload = None;
}

fn draw_playhead(app: &mut App, ctx: &mut Ctx, r: Rect) {
    let x = x_of(app, r, app.store.playhead).round();
    if x < r.x - 1.0 || x > r.right() {
        return;
    }
    let prev = ctx.painter.push_clip(r);
    ctx.painter.vline(x, r.y, r.bottom(), ACCENT_HI);
    let head = Rect::new(x - 5.0, r.y, 10.0, 10.0);
    ctx.painter.round_rect(head, 2.0, ACCENT_HI);
    ctx.painter.set_clip(prev);
}
