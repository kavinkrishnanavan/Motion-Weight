//! The inspector. Media clips get three tabs so crop, mask, transitions and
//! audio each have their own space instead of one endless column.

use crate::app::{tabs_for, App, SideTab, TransSlot};
use crate::color;
use crate::decode::Frame;
use crate::model::*;
use crate::transitions::TransitionState;
use crate::ui::paint::Blit;
use crate::ui::widgets::{self, ButtonStyle};
use crate::ui::*;

/// A simple top-down layout cursor.
struct Col {
    rect: Rect,
    y: f32,
}

impl Col {
    fn new(rect: Rect) -> Col {
        Col { y: rect.y, rect }
    }
    fn row(&mut self, h: f32) -> Rect {
        let r = Rect::new(self.rect.x, self.y, self.rect.w, h);
        self.y += h;
        r
    }
    fn gap(&mut self, h: f32) {
        self.y += h;
    }
    /// Splits the next row into `n` equal columns with a gap between.
    fn cols(&mut self, h: f32, n: usize) -> Vec<Rect> {
        let r = self.row(h);
        let gap = 8.0;
        let each = (r.w - gap * (n as f32 - 1.0)) / n as f32;
        (0..n)
            .map(|i| Rect::new(r.x + i as f32 * (each + gap), r.y, each, r.h))
            .collect()
    }
    fn used(&self) -> f32 {
        self.y - self.rect.y
    }
}

pub fn draw(app: &mut App, ctx: &mut Ctx, full: Rect) {
    let r = full;
    ctx.painter.rect(r, BG_PANEL);
    ctx.painter.vline(r.x, r.y, r.bottom(), BORDER);
    let header = Rect::new(r.x, r.y, r.w, PANEL_HEAD_H);
    crate::views::editor::drag_handle(app, ctx, header, crate::app::Panel::Inspector);
    let r = Rect::new(r.x, r.y + PANEL_HEAD_H, r.w, (r.h - PANEL_HEAD_H).max(0.0));

    let body = r.inset(14.0, 0.0);
    let view = Rect::new(body.x, body.y + 12.0, body.w, body.h - 20.0);
    // Leave a gutter on the right so nothing sits under the scrollbar.
    let content = Rect::new(view.x, view.y - app.side_scroll, view.w - 14.0, view.h);

    let prev = ctx.painter.push_clip(view);
    let used = match app.store.selection.and_then(|s| app.store.clip(s.clip_id)).cloned() {
        None => {
            app.store.mask_editing = false;
            project_settings(app, ctx, content)
        }
        Some(clip) => {
            let tabs = tabs_for(clip.kind);
            if !tabs.contains(&app.side_tab) {
                app.side_tab = SideTab::Basic;
            }
            // Opening the mask tab puts the mask's own handles on the preview.
            app.store.mask_editing =
                clip.kind != ClipKind::Text && app.side_tab == SideTab::Mask && clip.mask.is_active();
            if clip.kind == ClipKind::Text {
                text_props(app, ctx, content, &clip, &tabs)
            } else {
                media_props(app, ctx, content, &clip, &tabs)
            }
        }
    };
    ctx.painter.set_clip(prev);
    widgets::scroll(ctx, id_of("side-scroll", 0), view, used + 20.0, &mut app.side_scroll);
    // A detached pane's own window is already carved into this shape at the
    // OS level (see `round_window_corners` in main.rs); painting fake rounded
    // corners on top of that would just notch the seam under its titlebar.
    if !app.is_detached(crate::app::Panel::Inspector) {
        ctx.painter.round_corners(full, R_LG, BG_APP);
    }
}

fn title(ctx: &mut Ctx, col: &mut Col, text: &str) {
    let r = col.row(28.0);
    ctx.painter.label(r, text, FS_TITLE, Weight::Bold, TEXT, Align::Left);
}

fn project_settings(app: &mut App, ctx: &mut Ctx, area: Rect) -> f32 {
    let mut col = Col::new(area);
    title(ctx, &mut col, "Project");
    col.gap(6.0);

    let s = app.store.project.settings;
    widgets::field_label(ctx, col.row(18.0), "Resolution");
    let presets: Vec<String> = RESOLUTION_PRESETS.iter().map(|(l, _, _)| l.to_string()).collect();
    let selected = RESOLUTION_PRESETS
        .iter()
        .position(|(_, w, h)| *w == s.width && *h == s.height)
        .unwrap_or(0);
    if let Some(i) = widgets::dropdown(ctx, id_of("proj-res", 0), col.row(FIELD_H), &presets, selected) {
        app.store.snapshot_forced();
        app.store.project.settings.width = RESOLUTION_PRESETS[i].1;
        app.store.project.settings.height = RESOLUTION_PRESETS[i].2;
        app.store.touch();
    }
    col.gap(10.0);

    widgets::field_label(ctx, col.row(18.0), "Frame rate");
    let rates = [24u32, 25, 30, 60];
    let items: Vec<String> = rates.iter().map(|f| format!("{f} fps")).collect();
    let selected = rates.iter().position(|f| *f == s.fps).unwrap_or(2);
    if let Some(i) = widgets::dropdown(ctx, id_of("proj-fps", 0), col.row(FIELD_H), &items, selected) {
        app.store.project.settings.fps = rates[i];
        app.store.touch();
    }
    col.gap(14.0);

    if let Some(v) = widgets::checkbox(ctx, id_of("proj-grid", 0), col.row(22.0), "Show grid", app.store.show_grid) {
        app.store.show_grid = v;
    }
    col.gap(4.0);
    if let Some(v) = widgets::checkbox(ctx, id_of("proj-snap", 0), col.row(22.0), "Snap to grid", app.store.snap_to_grid) {
        app.store.snap_to_grid = v;
    }
    col.gap(16.0);

    let hint_rect = Rect::new(area.x, col.y, area.w, 60.0);
    let h = widgets::hint(
        ctx,
        hint_rect,
        "Select a clip on the timeline to edit it. Drag media from the library onto any lane.",
    );
    col.gap(h);
    col.used()
}

fn tab_bar(app: &mut App, ctx: &mut Ctx, col: &mut Col, tabs: &[SideTab]) {
    let r = col.row(28.0);
    ctx.painter.round_rect(r, R_SM, BG_PANEL_2);
    let each = r.w / tabs.len() as f32;
    for (i, t) in tabs.iter().enumerate() {
        let tr = Rect::new(r.x + i as f32 * each, r.y, each, r.h).inset(2.0, 2.0);
        if widgets::tab(ctx, id_of("side-tab", i as u64), tr, t.label(), *t == app.side_tab) {
            app.side_tab = *t;
            app.side_scroll = 0.0;
        }
    }
    col.gap(12.0);
}

/// Slider plus a numeric box, with a live readout in the label.
#[allow(clippy::too_many_arguments)]
fn slider_row(
    ctx: &mut Ctx,
    col: &mut Col,
    key: &str,
    label: &str,
    readout: &str,
    value: f32,
    min: f32,
    max: f32,
    num_value: f32,
    decimals: usize,
) -> (Option<f32>, Option<f32>) {
    let head = col.row(18.0);
    widgets::field_label(ctx, head, label);
    ctx.painter.label(
        head,
        readout,
        FS_SMALL,
        Weight::Regular,
        TEXT_3,
        Align::Right,
    );
    let row = col.row(FIELD_H);
    let (num, slide) = row.split_right(60.0);
    let slide = Rect::new(slide.x, slide.y, slide.w - 8.0, slide.h);
    let s = widgets::slider(ctx, id_of(key, 1), slide, value, min, max);
    let n = widgets::number_field(ctx, id_of(key, 2), num, num_value, decimals);
    col.gap(8.0);
    (s, n)
}

fn media_props(app: &mut App, ctx: &mut Ctx, area: Rect, clip: &Clip, tabs: &[SideTab]) -> f32 {
    let mut col = Col::new(area);
    let id = clip.id;
    let s = app.store.project.settings;
    let (pw, ph) = (s.width as f32, s.height as f32);
    tab_bar(app, ctx, &mut col, tabs);

    let has_audio = clip.kind == ClipKind::Audio
        || (clip.kind == ClipKind::Video && app.store.asset(clip.asset_id).map(|a| a.has_audio).unwrap_or(false));

    match app.side_tab {
        SideTab::Basic => basic_tab(app, ctx, &mut col, clip, pw, ph),
        SideTab::Mask => mask_tab(app, ctx, &mut col, clip),
        SideTab::Color => color_tab(app, ctx, &mut col, clip),
        SideTab::Transitions => {
            transitions_tab(app, ctx, &mut col, clip);
            if has_audio {
                audio_tab(app, ctx, &mut col, clip);
            }
        }
        SideTab::Audio => {
            if has_audio {
                audio_tab(app, ctx, &mut col, clip);
            } else {
                let r = col.row(40.0);
                widgets::hint(ctx, r, "This clip has no audio track.");
            }
        }
    }

    col.gap(16.0);
    let remove = col.row(FIELD_H + 4.0);
    if widgets::button(
        ctx,
        id_of("side-remove", 0),
        remove,
        &format!("Remove {}", clip.kind.label()),
        ButtonStyle::Danger,
    ) {
        app.store.remove_clip(id);
    }
    col.used()
}

fn basic_tab(app: &mut App, ctx: &mut Ctx, col: &mut Col, clip: &Clip, pw: f32, ph: f32) {
    let id = clip.id;
    if let Some(asset) = app.store.asset(clip.asset_id) {
        let name = asset.name.clone();
        widgets::field_label(ctx, col.row(16.0), "File");
        ctx.painter
            .label(col.row(20.0), &name, FS_BODY, Weight::Regular, TEXT, Align::Left);
        col.gap(10.0);
    }

    widgets::field_label(ctx, col.row(18.0), "Duration (sec)");
    if let Some(v) = widgets::number_field(ctx, id_of("side-dur", 0), col.row(FIELD_H), clip.duration, 2) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.duration = v.max(0.2);
        }
        app.store.touch();
    }
    col.gap(10.0);

    if matches!(clip.kind, ClipKind::Video | ClipKind::Audio) {
        widgets::field_label(ctx, col.row(18.0), "Trim in (sec)");
        if let Some(v) = widgets::number_field(ctx, id_of("side-trim", 0), col.row(FIELD_H), clip.trim_in, 2) {
            app.store.snapshot();
            if let Some(c) = app.store.clip_mut(id) {
                c.trim_in = v.max(0.0);
            }
            app.store.touch();
        }
        col.gap(10.0);
    }

    if !clip.is_visual() {
        return;
    }

    widgets::group_label(ctx, col.row(22.0), "TRANSFORM");
    col.gap(8.0);

    let xy = col.cols(FIELD_H + 18.0, 2);
    for (i, (label, value, is_x)) in [("X (px)", clip.x * pw, true), ("Y (px)", clip.y * ph, false)]
        .into_iter()
        .enumerate()
    {
        let (head, field) = xy[i].split_top(18.0);
        widgets::field_label(ctx, head, label);
        if let Some(v) = widgets::number_field(ctx, id_of("side-xy", i as u64), field, value, 0) {
            app.store.snapshot();
            if let Some(c) = app.store.clip_mut(id) {
                if is_x {
                    c.x = v / pw;
                } else {
                    c.y = v / ph;
                }
            }
            app.store.touch();
        }
    }
    col.gap(10.0);

    let (sw, nw) = slider_row(
        ctx,
        col,
        "side-w",
        "Width",
        &format!("{} px", (clip.width * pw).round() as i64),
        clip.width,
        0.03,
        2.0,
        (clip.width * pw).round(),
        0,
    );
    if let Some(v) = sw.map(|v| v).or(nw.map(|v| v / pw)) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.width = v.clamp(0.01, 4.0);
        }
        app.store.touch();
    }

    let (sh, nh) = slider_row(
        ctx,
        col,
        "side-h",
        "Height",
        &format!("{} px", (clip.height * ph).round() as i64),
        clip.height,
        0.03,
        2.0,
        (clip.height * ph).round(),
        0,
    );
    if let Some(v) = sh.map(|v| v).or(nh.map(|v| v / ph)) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.height = v.clamp(0.01, 4.0);
        }
        app.store.touch();
    }

    let buttons = col.cols(FIELD_H, 2);
    if widgets::button(ctx, id_of("side-fit", 0), buttons[0], "Fit frame", ButtonStyle::Normal) {
        let size = app
            .store
            .asset(clip.asset_id)
            .map(|a| fit_clip_size(a.width, a.height, pw as u32, ph as u32))
            .unwrap_or((1.0, 1.0));
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.width = size.0;
            c.height = size.1;
            c.x = 0.5;
            c.y = 0.5;
        }
        app.store.touch();
    }
    if widgets::button(ctx, id_of("side-centre", 0), buttons[1], "Centre", ButtonStyle::Normal) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.x = 0.5;
            c.y = 0.5;
        }
        app.store.touch();
    }
    col.gap(12.0);

    widgets::field_label(ctx, col.row(18.0), "Opacity");
    if let Some(v) = widgets::slider(ctx, id_of("side-op", 0), col.row(FIELD_H), clip.opacity, 0.0, 1.0) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.opacity = v;
        }
        app.store.touch();
    }
    col.gap(12.0);

    let h = widgets::hint(
        ctx,
        Rect::new(col.rect.x, col.y, col.rect.w, 60.0),
        "Drag on the preview to move. Eight handles resize; hold Shift on a corner to keep the aspect ratio.",
    );
    col.gap(h);
}

fn mask_tab(app: &mut App, ctx: &mut Ctx, col: &mut Col, clip: &Clip) {
    let id = clip.id;
    widgets::group_label(ctx, col.row(22.0), "CROP");
    col.gap(8.0);

    let crop = clip.crop;
    let pairs: [(&str, f32); 4] = [
        ("Left %", crop.left),
        ("Right %", crop.right),
        ("Top %", crop.top),
        ("Bottom %", crop.bottom),
    ];
    for chunk in 0..2 {
        let cols = col.cols(FIELD_H + 18.0, 2);
        for i in 0..2 {
            let index = chunk * 2 + i;
            let (label, value) = pairs[index];
            let (head, field) = cols[i].split_top(18.0);
            widgets::field_label(ctx, head, label);
            if let Some(v) = widgets::number_field(
                ctx,
                id_of("side-crop", index as u64),
                field,
                (value * 100.0).round(),
                0,
            ) {
                let f = (v / 100.0).clamp(0.0, 0.95);
                app.store.snapshot();
                if let Some(c) = app.store.clip_mut(id) {
                    match index {
                        0 => c.crop.left = f,
                        1 => c.crop.right = f,
                        2 => c.crop.top = f,
                        _ => c.crop.bottom = f,
                    }
                }
                app.store.touch();
            }
        }
        col.gap(6.0);
    }
    if widgets::button(
        ctx,
        id_of("side-cropreset", 0),
        Rect::new(col.rect.x, col.row(FIELD_H).y, 110.0, FIELD_H),
        "Reset crop",
        ButtonStyle::Normal,
    ) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.crop = Crop::default();
        }
        app.store.touch();
    }
    col.gap(14.0);

    widgets::group_label(ctx, col.row(22.0), "MASK");
    col.gap(8.0);
    widgets::field_label(ctx, col.row(18.0), "Shape");
    let items: Vec<String> = MASK_SHAPES.iter().map(|(_, l)| l.to_string()).collect();
    let selected = MASK_SHAPES.iter().position(|(s, _)| *s == clip.mask.shape).unwrap_or(0);
    if let Some(i) = widgets::dropdown(ctx, id_of("side-maskshape", 0), col.row(FIELD_H), &items, selected) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.mask.shape = MASK_SHAPES[i].0;
        }
        app.store.touch();
    }
    col.gap(10.0);

    if !clip.mask.is_active() {
        return;
    }

    let h = widgets::hint(
        ctx,
        Rect::new(col.rect.x, col.y, col.rect.w, 44.0),
        "Drag the mask on the preview to move it, its handles to resize, and the top knob to rotate.",
    );
    col.gap(h + 8.0);

    widgets::field_label(ctx, col.row(18.0), "Rotation");
    ctx.painter.label(
        Rect::new(col.rect.x, col.y - 18.0, col.rect.w, 18.0),
        &format!("{}\u{00b0}", clip.mask.rotation.round() as i64),
        FS_SMALL,
        Weight::Regular,
        TEXT_3,
        Align::Right,
    );
    if let Some(v) = widgets::slider(
        ctx,
        id_of("side-maskrot", 0),
        col.row(FIELD_H),
        clip.mask.rotation,
        -180.0,
        180.0,
    ) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.mask.rotation = v;
        }
        app.store.touch();
    }
    col.gap(10.0);

    widgets::field_label(ctx, col.row(18.0), "Feather");
    ctx.painter.label(
        Rect::new(col.rect.x, col.y - 18.0, col.rect.w, 18.0),
        &format!("{}%", (clip.mask.feather * 100.0).round() as i64),
        FS_SMALL,
        Weight::Regular,
        TEXT_3,
        Align::Right,
    );
    if let Some(v) = widgets::slider(
        ctx,
        id_of("side-feather", 0),
        col.row(FIELD_H),
        clip.mask.feather,
        0.0,
        0.5,
    ) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.mask.feather = v;
        }
        app.store.touch();
    }
    col.gap(10.0);

    if let Some(v) = widgets::checkbox(ctx, id_of("side-invert", 0), col.row(22.0), "Invert mask", clip.mask.invert) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.mask.invert = v;
        }
        app.store.touch();
    }
    col.gap(10.0);
    if widgets::button(
        ctx,
        id_of("side-maskreset", 0),
        Rect::new(col.rect.x, col.row(FIELD_H).y, 110.0, FIELD_H),
        "Reset mask",
        ButtonStyle::Normal,
    ) {
        app.store.snapshot_forced();
        let shape = clip.mask.shape;
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.mask = Mask { shape, ..Mask::default() };
        }
        app.store.touch();
    }
}

/// A -1..1 slider with a signed percent readout, the shape every basic
/// color-correction control shares.
fn percent_slider(ctx: &mut Ctx, col: &mut Col, key: &str, label: &str, value: f32) -> Option<f32> {
    let pct = (value * 100.0).round();
    let (sv, nv) = slider_row(ctx, col, key, label, &format!("{:+.0}", pct), value, -1.0, 1.0, pct, 0);
    sv.or(nv.map(|v| (v / 100.0).clamp(-1.0, 1.0)))
}

const CURVE_MAX_POINTS: usize = 8;

/// A draggable tone-curve editor. The two endpoints move vertically only
/// (their x stays pinned to the curve's domain); interior points move both
/// ways, clamped so they can never cross a neighbour. Clicking empty space
/// inside the graph adds a new point there, up to `CURVE_MAX_POINTS`.
/// Returns a replacement point list, kept sorted by x, whenever anything
/// changed this frame.
fn curve_editor(ctx: &mut Ctx, col: &mut Col, points: &[(f32, f32)]) -> Option<Vec<(f32, f32)>> {
    let row = col.row(160.0);
    let side = row.w.min(row.h);
    let graph = Rect::new(row.x, row.y, side, side);
    col.gap(10.0);

    ctx.painter.round_rect(graph, R_SM, BG_PANEL_2);
    for i in 1..4 {
        let gx = graph.x + graph.w * i as f32 / 4.0;
        ctx.painter.vline(gx, graph.y, graph.bottom(), BORDER_SOFT);
        let gy = graph.y + graph.h * i as f32 / 4.0;
        ctx.painter.hline(graph.x, graph.right(), gy, BORDER_SOFT);
    }
    ctx.painter.line(graph.x, graph.bottom(), graph.right(), graph.y, BORDER_SOFT, 1.0);
    ctx.painter.stroke_round_rect(graph, R_SM, BORDER, 1.0);

    let mut pts = if points.len() >= 2 { points.to_vec() } else { vec![(0.0, 0.0), (1.0, 1.0)] };
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));

    let to_screen = |p: (f32, f32)| (graph.x + p.0 * graph.w, graph.bottom() - p.1 * graph.h);
    let to_graph = |x: f32, y: f32| {
        (((x - graph.x) / graph.w).clamp(0.0, 1.0), ((graph.bottom() - y) / graph.h).clamp(0.0, 1.0))
    };

    // The curve line first, so the point handles draw on top of it. Traced as
    // one stroked path rather than 48 separate `line` calls: each `line` call
    // builds its own path and pays for its own anti-aliased fill, so this
    // used to mean 48 path allocations and fills every single redraw the
    // Color tab was open, not just while dragging a point.
    let steps = 48;
    let curve_pts: Vec<(f32, f32)> = (0..=steps)
        .map(|i| to_screen((i as f32 / steps as f32, color::eval_curve(&pts, i as f32 / steps as f32))))
        .collect();
    ctx.painter.polyline(&curve_pts, ACCENT, 1.6);

    let mut changed: Option<Vec<(f32, f32)>> = None;
    let radius = 5.0;
    let last = pts.len() - 1;
    for i in 0..pts.len() {
        let (sx, sy) = to_screen(pts[i]);
        let hit = Rect::new(sx - 8.0, sy - 8.0, 16.0, 16.0);
        let id = id_of("curve-pt", i as u64);
        let (hovered, _) = ctx.interact(id, hit);
        if hovered {
            ctx.cursor = Cursor::Hand;
        }
        if ctx.is_active(id) {
            let (mut gx, gy) = to_graph(ctx.mouse.0, ctx.mouse.1);
            if i == 0 {
                gx = 0.0;
            } else if i == last {
                gx = 1.0;
            } else {
                let lo = pts[i - 1].0 + 0.02;
                let hi = pts[i + 1].0 - 0.02;
                gx = gx.clamp(lo.min(hi), hi.max(lo));
            }
            pts[i] = (gx, gy);
            changed = Some(pts.clone());
        }
        let dot_color = if ctx.is_active(id) || hovered { ACCENT_HI } else { TEXT };
        ctx.painter
            .round_rect(Rect::new(sx - radius, sy - radius, radius * 2.0, radius * 2.0), radius, dot_color);
    }

    if pts.len() < CURVE_MAX_POINTS {
        let bg = id_of("curve-bg", 0);
        let (hovered, clicked) = ctx.interact(bg, graph);
        if hovered && ctx.active == 0 {
            ctx.cursor = Cursor::Hand;
        }
        if clicked {
            let (gx, gy) = to_graph(ctx.mouse.0, ctx.mouse.1);
            let too_close = pts.iter().any(|p| (p.0 - gx).abs() < 0.03);
            if !too_close {
                pts.push((gx, gy));
                pts.sort_by(|a, b| a.0.total_cmp(&b.0));
                changed = Some(pts.clone());
            }
        }
    }

    changed
}

fn color_tab(app: &mut App, ctx: &mut Ctx, col: &mut Col, clip: &Clip) {
    let id = clip.id;
    let grade = clip.color_grade.clone();

    widgets::group_label(ctx, col.row(22.0), "COLOR CORRECTION");
    col.gap(8.0);

    macro_rules! grade_slider {
        ($key:expr, $label:expr, $field:ident) => {
            if let Some(v) = percent_slider(ctx, col, $key, $label, grade.$field) {
                app.store.snapshot();
                if let Some(c) = app.store.clip_mut(id) {
                    c.color_grade.$field = v;
                    c.color_grade.preset = ColorPreset::Custom;
                }
                app.store.touch();
            }
        };
    }
    grade_slider!("side-temp", "Temperature", temperature);
    grade_slider!("side-tint", "Tint", tint);

    let ev = grade.exposure * 2.0;
    let (sv, nv) = slider_row(
        ctx,
        col,
        "side-exposure",
        "Exposure",
        &format!("{:+.1} EV", ev),
        grade.exposure,
        -1.0,
        1.0,
        ev,
        1,
    );
    if let Some(v) = sv.or(nv.map(|v| (v / 2.0).clamp(-1.0, 1.0))) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.color_grade.exposure = v;
            c.color_grade.preset = ColorPreset::Custom;
        }
        app.store.touch();
    }

    grade_slider!("side-highlights", "Highlights", highlights);
    grade_slider!("side-shadows", "Shadows", shadows);
    col.gap(8.0);

    widgets::group_label(ctx, col.row(22.0), "CURVES");
    col.gap(8.0);
    if let Some(pts) = curve_editor(ctx, col, &grade.curve) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.color_grade.curve = pts;
            c.color_grade.preset = ColorPreset::Custom;
        }
        app.store.touch();
    }
    if widgets::button(
        ctx,
        id_of("side-curvereset", 0),
        Rect::new(col.rect.x, col.row(FIELD_H).y, 110.0, FIELD_H),
        "Reset curve",
        ButtonStyle::Normal,
    ) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.color_grade.curve = vec![(0.0, 0.0), (1.0, 1.0)];
        }
        app.store.touch();
    }
    col.gap(16.0);

    widgets::group_label(ctx, col.row(22.0), "LUT");
    col.gap(8.0);
    let lut_label = if grade.lut_path.is_empty() {
        "Load LUT\u{2026}".to_string()
    } else {
        std::path::Path::new(&grade.lut_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| grade.lut_path.clone())
    };
    let has_lut = !grade.lut_path.is_empty();
    let lut_row = col.cols(FIELD_H, if has_lut { 2 } else { 1 });
    if widgets::button(ctx, id_of("side-lutload", 0), lut_row[0], &lut_label, ButtonStyle::Normal) {
        app.pick_lut(id);
    }
    if has_lut && widgets::button(ctx, id_of("side-lutclear", 0), lut_row[1], "Clear", ButtonStyle::Normal) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.color_grade.lut_path.clear();
        }
        app.store.touch();
    }
    col.gap(10.0);
    if has_lut {
        let pct = (grade.lut_strength * 100.0).round();
        let (sv, nv) = slider_row(
            ctx,
            col,
            "side-lutstrength",
            "LUT strength",
            &format!("{}%", pct as i64),
            grade.lut_strength,
            0.0,
            1.0,
            pct,
            0,
        );
        if let Some(v) = sv.or(nv.map(|v| (v / 100.0).clamp(0.0, 1.0))) {
            app.store.snapshot();
            if let Some(c) = app.store.clip_mut(id) {
                c.color_grade.lut_strength = v;
            }
            app.store.touch();
        }
        col.gap(6.0);
    }
    col.gap(10.0);

    widgets::group_label(ctx, col.row(22.0), "PRESETS");
    col.gap(8.0);
    let per_row = 3;
    let rows = (COLOR_PRESETS.len() as f32 / per_row as f32).ceil() as usize;
    for row in 0..rows {
        let cols = col.cols(FIELD_H, per_row);
        for (i, slot) in cols.iter().enumerate() {
            let idx = row * per_row + i;
            let Some((preset, label)) = COLOR_PRESETS.get(idx) else { continue };
            let active = grade.preset == *preset;
            let style = if active { ButtonStyle::Primary } else { ButtonStyle::Normal };
            if widgets::button(ctx, id_of("side-preset", idx as u64), *slot, label, style) {
                app.store.snapshot_forced();
                app.store.snapshot();
                if let Some(c) = app.store.clip_mut(id) {
                    c.color_grade.apply_preset(*preset);
                }
                app.store.touch();
            }
        }
        col.gap(6.0);
    }
    col.gap(8.0);

    if widgets::button(
        ctx,
        id_of("side-colorreset", 0),
        Rect::new(col.rect.x, col.row(FIELD_H).y, 110.0, FIELD_H),
        "Reset color",
        ButtonStyle::Normal,
    ) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            let (lut_path, lut_strength) = (c.color_grade.lut_path.clone(), c.color_grade.lut_strength);
            c.color_grade = ColorGrade { lut_path, lut_strength, ..ColorGrade::default() };
        }
        app.store.touch();
    }
}

/// Two small "clips" the gallery transitions between — there is no bundled
/// demo footage, so the app's own mark on a plain panel stands in for "the
/// next clip" and "the one leaving", sized and placed differently between
/// the two so a transition has something visible to reveal. Built once and
/// cached: recompositing the logo for up to 30 tiles every frame would be
/// wasteful when nothing about them ever changes.
fn demo_frames() -> &'static (Frame, Frame) {
    static CELL: std::sync::OnceLock<(Frame, Frame)> = std::sync::OnceLock::new();
    // Deliberately high-contrast (dark charcoal, small mark) vs (accent blue,
    // large mark) so every transition reads clearly against the panel's own
    // near-black background — two near-identical dark panels made fades and
    // wipes almost invisible at a glance.
    CELL.get_or_init(|| (demo_frame(0x2a2e38, 0.32), demo_frame(0x3d7eff, 0.6)))
}

fn demo_frame(bg: u32, logo_h: f32) -> Frame {
    let (w, h) = (160u32, 90u32);
    let sk = tiny_skia::Color::from_rgba8(((bg >> 16) & 0xff) as u8, ((bg >> 8) & 0xff) as u8, (bg & 0xff) as u8, 255);
    let mut pixmap = tiny_skia::Pixmap::new(w, h).expect("demo pixmap");
    pixmap.fill(sk);
    if let Some(logo) = crate::brand::pixmap() {
        let side = h as f32 * logo_h;
        let scale = side / logo.width().max(1) as f32;
        let tx = (w as f32 - logo.width() as f32 * scale) / 2.0;
        let ty = (h as f32 - logo.height() as f32 * scale) / 2.0;
        let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty);
        pixmap.draw_pixmap(0, 0, logo.as_ref(), &tiny_skia::PixmapPaint::default(), transform, None);
    }
    Frame { width: w, height: h, rgba: pixmap.data().to_vec() }
}

/// Renders one gallery tile: the "incoming" demo clip sits still underneath,
/// the "outgoing" one plays `kind`'s out-transition over it at progress `p` —
/// exactly the situation a real clip's `transition_out` renders into the
/// clip behind it, just staged with placeholder content instead of footage.
fn draw_transition_demo(ctx: &mut Ctx, r: Rect, kind: TransitionType, p: f32) {
    let (a, b) = demo_frames();
    let clipped = ctx.painter.push_clip(r);
    ctx.painter.blit(&b.rgba, b.width, b.height, r, &crate::ui::paint::Blit::default());
    if kind != TransitionType::None {
        let mut st = TransitionState::default();
        crate::transitions::apply(&mut st, kind, p, true);
        // `render_transitioned` takes its destination rect as given — it has
        // no clip geometry of its own to fold `offset_x/offset_y/scale` into,
        // unlike the real preview's `clip_rect`. The gallery's "clip" is
        // simply the whole tile, so the same fold-in happens here instead.
        let w = r.w * st.scale * st.scale_x;
        let h = r.h * st.scale;
        let dst = Rect::new(r.cx() + st.offset_x * r.w - w / 2.0, r.cy() + st.offset_y * r.h - h / 2.0, w, h);
        crate::preview::render_transitioned(ctx, &a.rgba, a.width, a.height, dst, (0.0, 0.0, 1.0, 1.0), st.alpha, &st, kind as Id, None);
    }
    ctx.painter.set_clip(clipped);
    // `blit` only clips to the (rectangular) clip rect above, so the content's
    // own corners come out square; paint them back over with the panel's own
    // background to fake rounding on top of it, same trick `widgets::thumb` uses.
    ctx.painter.round_corners(r, R_SM, BG_PANEL);
    ctx.painter.stroke_round_rect(r, R_SM, BORDER, 1.0);
}

/// 0 -> 1 -> 0 triangle wave from the shared gallery clock, used to loop the
/// hovered/selected tile's preview without tracking a per-tile start time.
fn gallery_phase(app: &App) -> f32 {
    let t = (app.gallery_clock.elapsed().as_secs_f32() / 1.6) % 1.0;
    1.0 - (t * 2.0 - 1.0).abs()
}

fn transitions_tab(app: &mut App, ctx: &mut Ctx, col: &mut Col, clip: &Clip) {
    let id = clip.id;
    widgets::group_label(ctx, col.row(22.0), "TRANSITIONS");
    col.gap(8.0);

    let slots = col.cols(FIELD_H, 2);
    if widgets::tab(ctx, id_of("trs-in", 0), slots[0], "In", app.trans_slot == TransSlot::In) {
        app.trans_slot = TransSlot::In;
    }
    if widgets::tab(ctx, id_of("trs-out", 0), slots[1], "Out", app.trans_slot == TransSlot::Out) {
        app.trans_slot = TransSlot::Out;
    }
    col.gap(8.0);

    let is_in = app.trans_slot == TransSlot::In;
    let current = if is_in { clip.transition_in } else { clip.transition_out };
    if current.kind != TransitionType::None {
        widgets::field_label(ctx, col.row(18.0), "Duration (sec)");
        if let Some(v) = widgets::number_field(ctx, id_of("trs-dur", 0), col.row(FIELD_H), current.duration, 1) {
            app.store.snapshot();
            if let Some(c) = app.store.clip_mut(id) {
                let t = if is_in { &mut c.transition_in } else { &mut c.transition_out };
                t.duration = v.clamp(0.1, 10.0);
            }
            app.store.touch();
        }
        col.gap(10.0);
    }

    let cols_n = 2usize;
    let gap = 8.0;
    let tile_w = (col.rect.w - gap * (cols_n as f32 - 1.0)) / cols_n as f32;
    let thumb_h = (tile_w * 9.0 / 16.0).round();
    let label_h = 16.0;
    let tile_h = thumb_h + label_h + 4.0;
    let phase = gallery_phase(app);
    let catalog: &[(TransitionType, &str)] =
        if clip.kind == ClipKind::Text { &TEXT_TRANSITIONS } else { &TRANSITIONS };

    for (row, chunk) in catalog.chunks(cols_n).enumerate() {
        let row_r = col.row(tile_h + gap);
        for (ci, (kind, label)) in chunk.iter().enumerate() {
            let thumb = Rect::new(row_r.x + ci as f32 * (tile_w + gap), row_r.y, tile_w, thumb_h);
            let selected = *kind == current.kind;
            let (hovered, clicked) = ctx.interact(id_of("trs-tile", (row * cols_n + ci) as u64), thumb);
            let p = if hovered || selected { phase } else { 0.5 };

            draw_transition_demo(ctx, thumb, *kind, p);
            if selected || hovered {
                ctx.painter.stroke_round_rect(thumb, R_SM, ACCENT, 2.0);
            }
            let name_r = Rect::new(thumb.x, thumb.bottom() + 3.0, thumb.w, label_h);
            ctx.painter.label(
                name_r,
                label,
                FS_SMALL,
                Weight::Regular,
                if selected { ACCENT_HI } else { TEXT_2 },
                Align::Center,
            );

            if clicked {
                app.store.snapshot_forced();
                let mirror = is_in && *kind != TransitionType::None && clip.transition_out.kind == TransitionType::None;
                let dur = current.duration;
                app.store.snapshot();
                if let Some(c) = app.store.clip_mut(id) {
                    if is_in {
                        c.transition_in.kind = *kind;
                        // Choosing an in-transition pre-fills the mirrored out.
                        if mirror {
                            c.transition_out = Transition { kind: kind.opposite(), duration: dur };
                        }
                    } else {
                        c.transition_out.kind = *kind;
                    }
                }
                app.store.touch();
            }
        }
    }
    col.gap(6.0);
}

fn audio_tab(app: &mut App, ctx: &mut Ctx, col: &mut Col, clip: &Clip) {
    let id = clip.id;
    widgets::group_label(ctx, col.row(22.0), "AUDIO");
    col.gap(8.0);

    let pct = (clip.volume * 100.0).round();
    let (sv, nv) = slider_row(
        ctx,
        col,
        "side-vol",
        "Volume",
        &if pct == 0.0 { "Muted".to_string() } else { format!("{}%", pct as i64) },
        clip.volume,
        0.0,
        1.0,
        pct,
        0,
    );
    if let Some(v) = sv.or(nv.map(|v| v / 100.0)) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.volume = v.clamp(0.0, 1.0);
        }
        app.store.touch();
    }

    let fades = col.cols(FIELD_H + 18.0, 2);
    for (i, (label, value)) in [("Fade in (sec)", clip.fade_in), ("Fade out (sec)", clip.fade_out)]
        .into_iter()
        .enumerate()
    {
        let (head, field) = fades[i].split_top(18.0);
        widgets::field_label(ctx, head, label);
        if let Some(v) = widgets::number_field(ctx, id_of("side-fade", i as u64), field, value, 1) {
            app.store.snapshot();
            if let Some(c) = app.store.clip_mut(id) {
                if i == 0 {
                    c.fade_in = v.max(0.0);
                } else {
                    c.fade_out = v.max(0.0);
                }
            }
            app.store.touch();
        }
    }
    col.gap(10.0);
    let h = widgets::hint(
        ctx,
        Rect::new(col.rect.x, col.y, col.rect.w, 40.0),
        "Sliding the volume all the way down mutes this clip completely.",
    );
    col.gap(h);
}

const SWATCHES: [&str; 10] = [
    "#ffffff", "#000000", "#ec5b53", "#ffb020", "#ffe14d", "#5ad17e", "#3d7eff", "#9b6cff", "#ff6ec7", "#9c9ca6",
];

/// A hex field with a row of quick swatches beside it.
fn color_field(ctx: &mut Ctx, col: &mut Col, key: &str, label: &str, value: &str) -> Option<String> {
    widgets::field_label(ctx, col.row(18.0), label);
    let row = col.row(FIELD_H);
    let (swatch, field) = row.split_left(FIELD_H);
    let [r, g, b] = parse_hex(value);
    ctx.painter.round_rect(swatch.inset(0.0, 2.0), R_SM, [r, g, b, 255]);
    ctx.painter.stroke_round_rect(swatch.inset(0.0, 2.0), R_SM, BORDER, 1.0);
    let field = Rect::new(field.x + 8.0, field.y, field.w - 8.0, field.h);
    let mut result = widgets::text_field(ctx, id_of(key, 0), field, value, "#rrggbb");

    let picks = col.row(20.0);
    let each = (picks.w / SWATCHES.len() as f32).min(22.0);
    for (i, hex) in SWATCHES.iter().enumerate() {
        let r = Rect::new(picks.x + i as f32 * each, picks.y + 2.0, each - 4.0, 14.0);
        let (hovered, clicked) = ctx.interact(id_of(key, 10 + i as u64), r);
        if hovered {
            ctx.cursor = Cursor::Hand;
        }
        let [cr, cg, cb] = parse_hex(hex);
        ctx.painter.round_rect(r, 3.0, [cr, cg, cb, 255]);
        if *hex == value {
            ctx.painter.stroke_round_rect(r, 3.0, ACCENT_HI, 1.5);
        }
        if clicked {
            result = Some(hex.to_string());
        }
    }
    col.gap(8.0);

    if let Some(v) = color_picker(ctx, key, col.row(104.0), result.as_deref().unwrap_or(value)) {
        result = Some(v);
    }
    col.gap(8.0);
    result
}

/// A saturation/value square (tinted by the current hue) plus a hue strip
/// beneath it — real point-and-click picking for any color, everywhere
/// `color_field` is used, rather than only hex-typing or the fixed swatch
/// row above. Rendered into a small buffer and blitted, since the painter
/// has no per-pixel gradient fill of its own; the buffer is tiny enough
/// (a few thousand pixels) that redrawing it every frame costs nothing.
fn color_picker(ctx: &mut Ctx, key: &str, r: Rect, value: &str) -> Option<String> {
    let sv_id = id_of(key, 40);
    let hue_id = id_of(key, 41);
    let (sv_rect, rest) = r.split_top((r.h - 22.0).max(40.0));
    let (_, hue_rect) = rest.split_top(6.0);

    let [r0, g0, b0] = parse_hex(value);
    let (h, s, v) = rgb_to_hsv(r0, g0, b0);

    let (sv_hover, _) = ctx.interact(sv_id, sv_rect);
    let (hue_hover, _) = ctx.interact(hue_id, hue_rect);
    if sv_hover || hue_hover || ctx.is_active(sv_id) || ctx.is_active(hue_id) {
        ctx.cursor = Cursor::Hand;
    }

    const N: u32 = 40;
    let mut buf = vec![0u8; (N * N * 4) as usize];
    for y in 0..N {
        for x in 0..N {
            let s2 = x as f32 / (N - 1) as f32;
            let v2 = 1.0 - y as f32 / (N - 1) as f32;
            let [pr, pg, pb] = hsv_to_rgb(h, s2, v2);
            let i = ((y * N + x) * 4) as usize;
            buf[i] = pr;
            buf[i + 1] = pg;
            buf[i + 2] = pb;
            buf[i + 3] = 255;
        }
    }
    ctx.painter.blit(&buf, N, N, sv_rect, &Blit::default());
    ctx.painter.stroke_round_rect(sv_rect, R_SM, BORDER, 1.0);
    let cursor = Rect::new(sv_rect.x + s * sv_rect.w - 4.0, sv_rect.y + (1.0 - v) * sv_rect.h - 4.0, 8.0, 8.0);
    ctx.painter.stroke_round_rect(cursor, 4.0, TEXT, 1.5);

    const HN: u32 = 64;
    let mut hbuf = vec![0u8; (HN * 4) as usize];
    for x in 0..HN {
        let hue = x as f32 / (HN - 1) as f32 * 360.0;
        let [pr, pg, pb] = hsv_to_rgb(hue, 1.0, 1.0);
        let i = (x * 4) as usize;
        hbuf[i] = pr;
        hbuf[i + 1] = pg;
        hbuf[i + 2] = pb;
        hbuf[i + 3] = 255;
    }
    ctx.painter.blit(&hbuf, HN, 1, hue_rect, &Blit::default());
    ctx.painter.stroke_round_rect(hue_rect, 3.0, BORDER, 1.0);
    let marker_x = hue_rect.x + (h / 360.0) * hue_rect.w;
    ctx.painter
        .stroke_round_rect(Rect::new(marker_x - 2.0, hue_rect.y - 2.0, 4.0, hue_rect.h + 4.0), 2.0, TEXT, 1.5);

    let new_hsv = if ctx.is_active(sv_id) {
        let ns = ((ctx.mouse.0 - sv_rect.x) / sv_rect.w).clamp(0.0, 1.0);
        let nv = (1.0 - (ctx.mouse.1 - sv_rect.y) / sv_rect.h).clamp(0.0, 1.0);
        Some((h, ns, nv))
    } else if ctx.is_active(hue_id) {
        let nh = ((ctx.mouse.0 - hue_rect.x) / hue_rect.w).clamp(0.0, 1.0) * 360.0;
        Some((nh, s, v))
    } else {
        None
    };
    new_hsv.map(|(h, s, v)| {
        let [nr, ng, nb] = hsv_to_rgb(h, s, v);
        format!("#{nr:02x}{ng:02x}{nb:02x}")
    })
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let h = if delta < 1e-6 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let s = if max < 1e-6 { 0.0 } else { delta / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = if hp < 1.0 {
        (c, x, 0.0)
    } else if hp < 2.0 {
        (x, c, 0.0)
    } else if hp < 3.0 {
        (0.0, c, x)
    } else if hp < 4.0 {
        (0.0, x, c)
    } else if hp < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let m = v - c;
    [
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

fn text_props(app: &mut App, ctx: &mut Ctx, area: Rect, clip: &Clip, tabs: &[SideTab]) -> f32 {
    let mut col = Col::new(area);
    let id = clip.id;
    let s = app.store.project.settings;
    let (pw, ph) = (s.width as f32, s.height as f32);
    tab_bar(app, ctx, &mut col, tabs);

    if app.side_tab == SideTab::Transitions {
        transitions_tab(app, ctx, &mut col, clip);
        col.gap(16.0);
        if widgets::button(
            ctx,
            id_of("txt-remove", 0),
            col.row(FIELD_H + 4.0),
            "Remove Text",
            ButtonStyle::Danger,
        ) {
            app.store.remove_clip(id);
        }
        return col.used();
    }

    widgets::field_label(ctx, col.row(18.0), "Content");
    if let Some(v) = widgets::text_field(ctx, id_of("txt-content", 0), col.row(FIELD_H), &clip.text, "Text") {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.text = v;
        }
        app.store.touch();
    }
    col.gap(12.0);

    widgets::field_label(ctx, col.row(18.0), "Font");
    let fonts: Vec<String> = FONT_FAMILIES.iter().map(|f| f.to_string()).collect();
    let selected = FONT_FAMILIES.iter().position(|f| *f == clip.font_family).unwrap_or(0);
    if let Some(i) = widgets::dropdown(ctx, id_of("txt-font", 0), col.row(FIELD_H), &fonts, selected) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.font_family = FONT_FAMILIES[i].to_string();
            c.text_preset = TextPreset::Custom;
        }
        app.store.touch();
    }
    col.gap(10.0);

    let size_spacing = col.cols(FIELD_H + 18.0, 2);
    let (size_head, size_field) = size_spacing[0].split_top(18.0);
    widgets::field_label(ctx, size_head, "Size");
    if let Some(v) = widgets::number_field(ctx, id_of("txt-size", 0), size_field, clip.font_size, 0) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.font_size = v.clamp(8.0, 400.0);
            c.text_preset = TextPreset::Custom;
        }
        app.store.touch();
    }
    let (spacing_head, spacing_field) = size_spacing[1].split_top(18.0);
    widgets::field_label(ctx, spacing_head, "Letter spacing");
    if let Some(v) = widgets::number_field(ctx, id_of("txt-spacing", 0), spacing_field, clip.letter_spacing, 0) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.letter_spacing = v.clamp(-20.0, 100.0);
            c.text_preset = TextPreset::Custom;
        }
        app.store.touch();
    }
    col.gap(10.0);

    if let Some(v) = color_field(ctx, &mut col, "txt-color", "Colour", &clip.color) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.color = v;
            c.text_preset = TextPreset::Custom;
        }
        app.store.touch();
    }

    let toggles = col.cols(FIELD_H, 4);
    if widgets::tab(ctx, id_of("txt-bold", 0), toggles[0], "B", clip.bold) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.bold = !c.bold;
            c.text_preset = TextPreset::Custom;
        }
        app.store.touch();
    }
    if widgets::tab(ctx, id_of("txt-italic", 0), toggles[1], "I", clip.italic) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.italic = !c.italic;
            c.text_preset = TextPreset::Custom;
        }
        app.store.touch();
    }
    if widgets::tab(ctx, id_of("txt-upper", 0), toggles[2], "AA", clip.uppercase) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.uppercase = !c.uppercase;
            c.text_preset = TextPreset::Custom;
        }
        app.store.touch();
    }
    let aligns = ["Left", "Center", "Right"];
    let items: Vec<String> = aligns.iter().map(|a| a.to_string()).collect();
    let sel = match clip.align {
        TextAlign::Left => 0,
        TextAlign::Center => 1,
        TextAlign::Right => 2,
    };
    if let Some(i) = widgets::dropdown(ctx, id_of("txt-align", 0), toggles[3], &items, sel) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.align = [TextAlign::Left, TextAlign::Center, TextAlign::Right][i];
        }
        app.store.touch();
    }
    col.gap(12.0);

    if let Some(v) = widgets::checkbox(ctx, id_of("txt-bg", 0), col.row(22.0), "Background", clip.bg_enabled) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.bg_enabled = v;
        }
        app.store.touch();
    }
    col.gap(6.0);
    if clip.bg_enabled {
        if let Some(v) = color_field(ctx, &mut col, "txt-bgcolor", "Background colour", &clip.bg_color) {
            app.store.snapshot();
            if let Some(c) = app.store.clip_mut(id) {
                c.bg_color = v;
            }
            app.store.touch();
        }
    }
    col.gap(6.0);

    widgets::group_label(ctx, col.row(22.0), "EFFECTS");
    col.gap(8.0);
    if let Some(v) = widgets::checkbox(ctx, id_of("txt-outline", 0), col.row(22.0), "Outline", clip.outline_enabled) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.outline_enabled = v;
            c.text_preset = TextPreset::Custom;
        }
        app.store.touch();
    }
    col.gap(6.0);
    if clip.outline_enabled {
        if let Some(v) = color_field(ctx, &mut col, "txt-outlinecolor", "Outline colour", &clip.outline_color) {
            app.store.snapshot();
            if let Some(c) = app.store.clip_mut(id) {
                c.outline_color = v;
                c.text_preset = TextPreset::Custom;
            }
            app.store.touch();
        }
        widgets::field_label(ctx, col.row(18.0), "Outline width");
        if let Some(v) = widgets::number_field(ctx, id_of("txt-outlinewidth", 0), col.row(FIELD_H), clip.outline_width, 1) {
            app.store.snapshot();
            if let Some(c) = app.store.clip_mut(id) {
                c.outline_width = v.clamp(0.0, 20.0);
                c.text_preset = TextPreset::Custom;
            }
            app.store.touch();
        }
        col.gap(10.0);
    }

    if let Some(v) = widgets::checkbox(ctx, id_of("txt-glow", 0), col.row(22.0), "Glow", clip.glow_enabled) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.glow_enabled = v;
            c.text_preset = TextPreset::Custom;
        }
        app.store.touch();
    }
    col.gap(6.0);
    if clip.glow_enabled {
        if let Some(v) = color_field(ctx, &mut col, "txt-glowcolor", "Glow colour", &clip.glow_color) {
            app.store.snapshot();
            if let Some(c) = app.store.clip_mut(id) {
                c.glow_color = v;
                c.text_preset = TextPreset::Custom;
            }
            app.store.touch();
        }
    }

    if let Some(v) = widgets::checkbox(ctx, id_of("txt-shadow", 0), col.row(22.0), "Shadow", clip.shadow_enabled) {
        app.store.snapshot_forced();
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.shadow_enabled = v;
            c.text_preset = TextPreset::Custom;
        }
        app.store.touch();
    }
    col.gap(6.0);
    if clip.shadow_enabled {
        if let Some(v) = color_field(ctx, &mut col, "txt-shadowcolor", "Shadow colour", &clip.shadow_color) {
            app.store.snapshot();
            if let Some(c) = app.store.clip_mut(id) {
                c.shadow_color = v;
                c.text_preset = TextPreset::Custom;
            }
            app.store.touch();
        }
    }
    col.gap(6.0);

    widgets::field_label(ctx, col.row(18.0), "Opacity");
    if let Some(v) = widgets::slider(ctx, id_of("txt-op", 0), col.row(FIELD_H), clip.opacity, 0.0, 1.0) {
        app.store.snapshot();
        if let Some(c) = app.store.clip_mut(id) {
            c.opacity = v;
        }
        app.store.touch();
    }
    col.gap(12.0);

    widgets::group_label(ctx, col.row(22.0), "POSITION");
    col.gap(8.0);
    let xy = col.cols(FIELD_H + 18.0, 2);
    for (i, (label, value, is_x)) in [("X (px)", clip.x * pw, true), ("Y (px)", clip.y * ph, false)]
        .into_iter()
        .enumerate()
    {
        let (head, field) = xy[i].split_top(18.0);
        widgets::field_label(ctx, head, label);
        if let Some(v) = widgets::number_field(ctx, id_of("txt-xy", i as u64), field, value, 0) {
            app.store.snapshot();
            if let Some(c) = app.store.clip_mut(id) {
                if is_x {
                    c.x = v / pw;
                } else {
                    c.y = v / ph;
                }
            }
            app.store.touch();
        }
    }
    col.gap(10.0);

    let timing = col.cols(FIELD_H + 18.0, 2);
    for (i, (label, value)) in [("Start (sec)", clip.start), ("Duration (sec)", clip.duration)]
        .into_iter()
        .enumerate()
    {
        let (head, field) = timing[i].split_top(18.0);
        widgets::field_label(ctx, head, label);
        if let Some(v) = widgets::number_field(ctx, id_of("txt-time", i as u64), field, value, 2) {
            app.store.snapshot();
            if let Some(c) = app.store.clip_mut(id) {
                if i == 0 {
                    c.start = v.max(0.0);
                } else {
                    c.duration = v.max(0.1);
                }
            }
            app.store.touch();
        }
    }
    col.gap(16.0);

    if widgets::button(
        ctx,
        id_of("txt-remove", 0),
        col.row(FIELD_H + 4.0),
        "Remove Text",
        ButtonStyle::Danger,
    ) {
        app.store.remove_clip(id);
    }
    col.used()
}
