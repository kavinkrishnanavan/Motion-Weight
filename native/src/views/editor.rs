//! Editor chrome: the top bar, the workspace split, the transport, and the
//! keyboard shortcuts.

use crate::app::{App, LibraryTab, Panel};
use crate::media::timecode;
use crate::model::ClipKind;
use crate::ui::widgets::{self, ButtonStyle, Icon};
use crate::ui::*;

pub fn draw(app: &mut App, ctx: &mut Ctx, full: Rect) {
    let (topbar, rest) = full.split_top(TOPBAR_H);
    draw_topbar(app, ctx, topbar);
    ctx.mark("topbar");

    // Each pane only claims space while it is docked, so popping one out lets
    // the others expand into the gap rather than leaving a hole. A PANEL_GAP
    // strip is carved out of the remainder on every split, so the gap moves
    // with whichever panes are still docked instead of leaving a hole itself.
    // A resize_bar sits centred on each gap, growing the hit area a few
    // pixels into both neighbours without touching what gets painted there.
    //
    // The player has no width of its own — it's just whatever the other two
    // don't claim — so when it's the one detached, nobody would naturally
    // expand into its space. Inspector is laid out before library so library
    // can see exactly how much inspector left it, and flexes to take all of
    // that when there's no player to share the row with; inspector does the
    // same in the rarer case that library is *also* gone. Either way the top
    // row always fills edge to edge, with no dead strip standing in for
    // whichever pane just left.
    let mut workspace = rest.inset(PANEL_GAP, PANEL_GAP);
    let player_docked = !app.is_detached(Panel::Player);

    let timeline = if app.is_detached(Panel::Timeline) {
        None
    } else {
        let max_h = (workspace.h * 0.7).max(TIMELINE_H_MIN);
        app.timeline_h = app.timeline_h.clamp(TIMELINE_H_MIN, max_h);
        let (t, rest) = workspace.split_bottom(app.timeline_h);
        let bar = Rect::new(rest.x, rest.bottom() - 3.0, rest.w, PANEL_GAP + 6.0);
        let dy = widgets::resize_bar(ctx, id_of("resize-timeline", 0), bar, true);
        app.timeline_h = (app.timeline_h - dy).clamp(TIMELINE_H_MIN, max_h);
        workspace = shrink_bottom(rest, PANEL_GAP);
        Some(t)
    };

    let inspector = if app.is_detached(Panel::Inspector) {
        None
    } else {
        let sole_survivor = !player_docked && app.is_detached(Panel::Library);
        let render_w = if sole_survivor {
            workspace.w.max(INSPECTOR_W_MIN)
        } else {
            let max_w = (workspace.w * 0.4).max(INSPECTOR_W_MIN);
            app.inspector_w = app.inspector_w.clamp(INSPECTOR_W_MIN, max_w);
            app.inspector_w
        };
        let (insp, rest) = workspace.split_right(render_w);
        if !sole_survivor {
            let max_w = (workspace.w * 0.4).max(INSPECTOR_W_MIN);
            let bar = Rect::new(rest.right() - 3.0, rest.y, PANEL_GAP + 6.0, rest.h);
            let dx = widgets::resize_bar(ctx, id_of("resize-inspector", 0), bar, false);
            app.inspector_w = (app.inspector_w - dx).clamp(INSPECTOR_W_MIN, max_w);
        }
        workspace = shrink_right(rest, PANEL_GAP);
        Some(insp)
    };

    let library = if app.is_detached(Panel::Library) {
        None
    } else {
        let (group, rest) = if player_docked {
            let max_w = (workspace.w * 0.5 - RAIL_W).max(LIBRARY_W_MIN);
            app.library_w = app.library_w.clamp(LIBRARY_W_MIN, max_w);
            let (group, rest) = workspace.split_left(RAIL_W + app.library_w);
            let bar = Rect::new(rest.x - 3.0, rest.y, PANEL_GAP + 6.0, rest.h);
            let dx = widgets::resize_bar(ctx, id_of("resize-library", 0), bar, false);
            app.library_w = (app.library_w + dx).clamp(LIBRARY_W_MIN, max_w);
            (group, rest)
        } else {
            // No player to bound it on the right: take every pixel inspector
            // (already laid out above) left in the workspace.
            (Rect::new(workspace.x, workspace.y, workspace.w, workspace.h), Rect::new(workspace.right(), workspace.y, 0.0, workspace.h))
        };
        let (rail, lib) = group.split_left(RAIL_W);
        workspace = shrink_left(rest, PANEL_GAP);
        Some((rail, lib))
    };

    if let Some((rail, lib)) = library {
        draw_library_group(app, ctx, rail, lib);
        ctx.mark("library");
    }
    if player_docked {
        draw_player(app, ctx, workspace);
        ctx.mark("player");
    }
    if let Some(insp) = inspector {
        super::inspector::draw(app, ctx, insp);
        ctx.mark("inspector");
    }
    if let Some(tl) = timeline {
        super::timeline::draw(app, ctx, tl);
        ctx.mark("timeline");
    }

    shortcuts(app, ctx);
}

/// Renders one pane on its own, filling a detached window.
pub fn draw_panel(app: &mut App, ctx: &mut Ctx, panel: Panel, full: Rect) {
    match panel {
        Panel::Library => {
            let (rail, body) = full.split_left(RAIL_W);
            draw_library_group(app, ctx, rail, body);
        }
        Panel::Player => draw_player(app, ctx, full),
        Panel::Inspector => super::inspector::draw(app, ctx, full),
        Panel::Timeline => super::timeline::draw(app, ctx, full),
    }
    shortcuts(app, ctx);
}

/// Removes a `PANEL_GAP` strip from the near edge of a workspace remainder,
/// so consecutive docked panes never touch each other.
fn shrink_left(r: Rect, gap: f32) -> Rect {
    Rect::new(r.x + gap, r.y, (r.w - gap).max(0.0), r.h)
}
fn shrink_right(r: Rect, gap: f32) -> Rect {
    Rect::new(r.x, r.y, (r.w - gap).max(0.0), r.h)
}
fn shrink_bottom(r: Rect, gap: f32) -> Rect {
    Rect::new(r.x, r.y, r.w, (r.h - gap).max(0.0))
}

/// The rail and the media library dock and detach together as one "Library"
/// pane, so they share a single rounded card rather than each getting their
/// own — the seam between them stays square, only the outer corners round.
fn draw_library_group(app: &mut App, ctx: &mut Ctx, rail: Rect, lib: Rect) {
    draw_rail(app, ctx, rail);
    ctx.mark("rail");
    super::library::draw(app, ctx, lib);
    // A detached pane's own window is already carved into this shape at the
    // OS level; see the matching comment in inspector.rs.
    if !app.is_detached(Panel::Library) {
        let group = Rect::new(rail.x, rail.y, rail.w + lib.w, rail.h);
        ctx.painter.round_corners(group, R_LG, BG_APP);
    }
}

/// Makes a docked pane's header a tear-off handle: press and pull it more
/// than a few pixels and the pane rips free into its own OS window, spawned
/// with the exact point that was grabbed still under the cursor and already
/// picked up by the window manager's own move — the same feel as pulling a
/// tab out of a browser's tab strip. A pane already in its own window is
/// moved with that window's native titlebar instead, so this is a no-op
/// there (`draw_panel` renders through the same functions for both).
pub fn drag_handle(app: &mut App, ctx: &mut Ctx, r: Rect, panel: Panel) {
    if app.is_detached(panel) || r.is_empty() {
        return;
    }
    let id = id_of("tear", panel as u64);
    let (hovered, _) = ctx.interact(id, r);
    if hovered || ctx.is_active(id) {
        ctx.cursor = Cursor::Move;
    }
    if ctx.is_active(id) {
        let dx = ctx.mouse.0 - ctx.drag_origin.0;
        let dy = ctx.mouse.1 - ctx.drag_origin.1;
        if dx.hypot(dy) > 6.0 {
            let grab = (ctx.drag_origin.0 - r.x, ctx.drag_origin.1 - r.y);
            app.request_detach(panel, grab);
            // The widget that triggered this vanishes from the docked
            // layout the instant it's detached, so nothing will ever call
            // interact() on this id again to clear it — release it by hand
            // or every future press-driven widget in this window is locked
            // out (interact only claims `active` when it is already 0).
            ctx.active = 0;
        }
    }
}

fn draw_topbar(app: &mut App, ctx: &mut Ctx, r: Rect) {
    ctx.painter.rect(r, BG_PANEL);
    ctx.painter.hline(r.x, r.right(), r.bottom() - 1.0, BORDER);

    // The mark doubles as the way back to the project picker.
    let mut x = r.x + 10.0;
    let home = Rect::new(x, r.cy() - 15.0, 30.0, 30.0);
    let (hovered, clicked) = ctx.interact(id_of("tb-home", 0), home);
    if hovered {
        ctx.cursor = Cursor::Hand;
        ctx.tooltip = Some((home, "Projects".into()));
        ctx.painter.round_rect(home, R_MD, BG_HOVER);
    }
    crate::brand::draw(ctx, home.inset(3.0, 3.0));
    if clicked {
        app.go_home();
        return;
    }
    x += 38.0;
    ctx.painter.label(
        Rect::new(x, r.y, 140.0, r.h),
        "MotionWeight",
        FS_BRAND,
        Weight::Bold,
        TEXT,
        Align::Left,
    );

    // Centre: project name and the save indicator.
    let name_w = 300.0;
    let centre = Rect::new(r.cx() - name_w / 2.0, r.y, name_w, r.h);
    ctx.painter
        .label(centre, &app.project_name, FS_BODY, Weight::Regular, TEXT_2, Align::Center);

    // Right: undo/redo, settings, exports.
    let mut rx = r.right() - 12.0;
    let export = Rect::new(rx - 86.0, r.cy() - 15.0, 86.0, 30.0);
    if widgets::button(ctx, id_of("tb-export", 0), export, "Export", ButtonStyle::Primary) {
        app.export_video();
    }
    rx -= 94.0;
    let frame = Rect::new(rx - 108.0, r.cy() - 15.0, 108.0, 30.0);
    if widgets::button(ctx, id_of("tb-frame", 0), frame, "Export frame", ButtonStyle::Normal) {
        app.export_frame();
    }
    rx -= 116.0;
    let can_redo = app.store.can_redo();
    if widgets::icon_button(
        ctx,
        id_of("tb-redo", 0),
        Rect::new(rx - 30.0, r.cy() - 15.0, 30.0, 30.0),
        Icon::Redo,
        "Redo (Ctrl+Y)",
        false,
    ) && can_redo
    {
        app.store.redo();
    }
    rx -= 34.0;
    let can_undo = app.store.can_undo();
    if widgets::icon_button(
        ctx,
        id_of("tb-undo", 0),
        Rect::new(rx - 30.0, r.cy() - 15.0, 30.0, 30.0),
        Icon::Undo,
        "Undo (Ctrl+Z)",
        false,
    ) && can_undo
    {
        app.store.undo();
    }
}

fn draw_rail(app: &mut App, ctx: &mut Ctx, r: Rect) {
    ctx.painter.rect(r, BG_PANEL);
    ctx.painter.vline(r.right() - 1.0, r.y, r.bottom(), BORDER);

    let tabs = [
        (LibraryTab::Media, Icon::Media, "Media"),
        (LibraryTab::Text, Icon::Text, "Text"),
        (LibraryTab::Stock, Icon::Stock, "Stock"),
    ];
    let mut y = r.y + 10.0;
    for (i, (tab, icon, label)) in tabs.into_iter().enumerate() {
        let item = Rect::new(r.x + 8.0, y, r.w - 16.0, 54.0);
        let active = app.library_tab == tab;
        let (hovered, clicked) = ctx.interact(id_of("rail", i as u64), item);
        if hovered {
            ctx.cursor = Cursor::Hand;
        }
        if active {
            ctx.painter.round_rect(item, R_MD, BG_ELEV);
        } else if hovered {
            ctx.painter.round_rect(item, R_MD, BG_PANEL_2);
        }
        let color = if active { TEXT } else { TEXT_3 };
        widgets::draw_icon(ctx, icon, Rect::new(item.cx() - 12.0, item.y + 8.0, 24.0, 24.0), color);
        ctx.painter.label(
            Rect::new(item.x, item.y + 32.0, item.w, 16.0),
            label,
            FS_SMALL,
            Weight::Regular,
            color,
            Align::Center,
        );
        if clicked {
            app.library_tab = tab;
            app.library_scroll = 0.0;
            let stock_empty = app.stock_photos.is_empty() && app.stock_videos.is_empty();
            let stock_loading = app.stock_photo_loading || app.stock_video_loading;
            if tab == LibraryTab::Stock && stock_empty && !stock_loading {
                app.run_stock_search();
            }
        }
        y += 58.0;
    }
}

fn draw_player(app: &mut App, ctx: &mut Ctx, r: Rect) {
    ctx.painter.rect(r, BG_PANEL);
    let body = widgets::panel(ctx, r);
    drag_handle(app, ctx, Rect::new(r.x, r.y, r.w, PANEL_HEAD_H), Panel::Player);
    let (info, body) = body.split_bottom(30.0);
    let (controls, stage_area) = body.split_bottom(46.0);

    ctx.mark("player.chrome");
    if app.preview.draw(ctx, &mut app.store, stage_area) {
        app.stop_playback();
    }
    ctx.mark("player.preview");

    draw_controls(app, ctx, controls);
    draw_info(app, ctx, info);
    // A detached pane's own window is already carved into this shape at the
    // OS level; see the matching comment in inspector.rs.
    if !app.is_detached(Panel::Player) {
        ctx.painter.round_corners(r, R_LG, BG_APP);
    }
}

fn draw_controls(app: &mut App, ctx: &mut Ctx, r: Rect) {
    ctx.painter.rect(r, BG_PANEL);
    ctx.painter.hline(r.x, r.right(), r.y, BORDER_SOFT);
    let fps = app.store.project.settings.fps;
    let total = app.store.total_duration();

    ctx.painter.label(
        Rect::new(r.x + 14.0, r.y, 190.0, r.h),
        &format!("{} / {}", timecode(app.store.playhead, fps), timecode(total, fps)),
        FS_BODY,
        Weight::Regular,
        TEXT_2,
        Align::Left,
    );

    let cy = r.cy();
    if widgets::icon_button(
        ctx,
        id_of("pl-start", 0),
        Rect::new(r.cx() - 66.0, cy - 14.0, 28.0, 28.0),
        Icon::SkipStart,
        "Go to start",
        false,
    ) {
        app.store.set_playhead(0.0);
    }
    let play = Rect::new(r.cx() - 16.0, cy - 16.0, 32.0, 32.0);
    let (hovered, clicked) = ctx.interact(id_of("pl-play", 0), play);
    if hovered {
        ctx.cursor = Cursor::Hand;
    }
    ctx.painter
        .round_rect(play, play.w / 2.0, if hovered { ACCENT_HI } else { ACCENT });
    widgets::draw_icon(
        ctx,
        if app.store.playing { Icon::Pause } else { Icon::Play },
        play,
        [255, 255, 255, 255],
    );
    if clicked {
        app.toggle_play();
    }
    if widgets::icon_button(
        ctx,
        id_of("pl-end", 0),
        Rect::new(r.cx() + 38.0, cy - 14.0, 28.0, 28.0),
        Icon::SkipEnd,
        "Go to end",
        false,
    ) {
        app.store.set_playhead(total);
    }

    // Right side: loop, speed, measured frame rate.
    let mut rx = r.right() - 12.0;
    ctx.painter.label(
        Rect::new(rx - 130.0, r.y, 130.0, r.h),
        &format!("{} fps · {:.0}ms lag", app.preview.measured_fps, ctx.input_latency_ms),
        FS_SMALL,
        Weight::Regular,
        TEXT_3,
        Align::Right,
    );
    rx -= 140.0;

    let rates = ["0.5x", "1x", "1.5x", "2x"];
    let values = [0.5, 1.0, 1.5, 2.0];
    let selected = values
        .iter()
        .position(|v| (*v - app.store.playback_rate).abs() < 1e-3)
        .unwrap_or(1);
    let rate_rect = Rect::new(rx - 74.0, r.cy() - 13.0, 74.0, 26.0);
    let items: Vec<String> = rates.iter().map(|s| s.to_string()).collect();
    if let Some(i) = widgets::dropdown(ctx, id_of("pl-rate", 0), rate_rect, &items, selected) {
        app.store.playback_rate = values[i];
    }
    rx -= 82.0;
    let loop_on = app.store.loop_playback;
    if widgets::icon_button(
        ctx,
        id_of("pl-loop", 0),
        Rect::new(rx - 28.0, r.cy() - 14.0, 28.0, 28.0),
        Icon::Loop,
        "Loop",
        loop_on,
    ) {
        app.store.loop_playback = !loop_on;
    }
}

fn draw_info(app: &mut App, ctx: &mut Ctx, r: Rect) {
    ctx.painter.rect(r, BG_PANEL_2);
    ctx.painter.hline(r.x, r.right(), r.y, BORDER_SOFT);
    let s = app.store.project.settings;
    let clip = app.store.selected_clip();

    let (name, xy, wh) = match clip {
        None => ("No clip selected".to_string(), "X — · Y —".to_string(), "W — · H —".to_string()),
        Some(c) if c.kind == ClipKind::Audio => (
            "Audio clip".to_string(),
            "X — · Y —".to_string(),
            "W — · H —".to_string(),
        ),
        Some(c) => {
            let name = if c.kind == ClipKind::Text {
                let head: String = c.text.chars().take(18).collect();
                format!("Text \u{201c}{head}\u{201d}")
            } else {
                app.store
                    .asset(c.asset_id)
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| c.kind.label().to_string())
            };
            let xy = format!(
                "X {} · Y {}",
                (c.x * s.width as f32).round() as i64,
                (c.y * s.height as f32).round() as i64
            );
            let wh = if c.kind == ClipKind::Text {
                format!("Size {} px", c.font_size.round() as i64)
            } else {
                format!(
                    "W {} · H {}",
                    (c.width * s.width as f32).round() as i64,
                    (c.height * s.height as f32).round() as i64
                )
            };
            (name, xy, wh)
        }
    };

    let mut x = r.x + 14.0;
    for (text, width) in [(name, 230.0), (xy, 150.0), (wh, 150.0)] {
        ctx.painter.label(
            Rect::new(x, r.y, width, r.h),
            &text,
            FS_SMALL,
            Weight::Regular,
            TEXT_2,
            Align::Left,
        );
        x += width + 8.0;
    }

    let grid_on = app.store.show_grid;
    let snap_on = app.store.snap_to_grid;
    if widgets::chip(
        ctx,
        id_of("info-snap", 0),
        Rect::new(r.right() - 68.0, r.cy() - 10.0, 56.0, 20.0),
        "Snap",
        snap_on,
    ) {
        app.store.snap_to_grid = !snap_on;
    }
    if widgets::chip(
        ctx,
        id_of("info-grid", 0),
        Rect::new(r.right() - 130.0, r.cy() - 10.0, 56.0, 20.0),
        "Grid",
        grid_on,
    ) {
        app.store.show_grid = !grid_on;
    }
}

fn shortcuts(app: &mut App, ctx: &mut Ctx) {
    // A focused text field owns the keyboard.
    if ctx.focus != 0 {
        return;
    }
    let keys = ctx.keys.clone();
    let mods = ctx.mods;
    for key in keys {
        match key {
            Key::Space => app.toggle_play(),
            Key::Delete | Key::Backspace => {
                if let Some(sel) = app.store.selection {
                    app.store.remove_clip(sel.clip_id);
                }
            }
            Key::Left => {
                let step = if mods.shift { 1.0 } else { 1.0 / app.store.project.settings.fps as f32 };
                let t = app.store.playhead - step;
                app.store.set_playhead(t);
            }
            Key::Right => {
                let step = if mods.shift { 1.0 } else { 1.0 / app.store.project.settings.fps as f32 };
                let t = app.store.playhead + step;
                app.store.set_playhead(t);
            }
            Key::Char('z') if mods.ctrl => {
                if mods.shift {
                    app.store.redo();
                } else {
                    app.store.undo();
                }
            }
            Key::Char('y') if mods.ctrl => app.store.redo(),
            Key::Char('s') if !mods.ctrl => {
                if let Some(sel) = app.store.selection {
                    let at = app.store.playhead;
                    app.store.split_clip(sel.clip_id, at);
                }
            }
            Key::Char('q') if !mods.ctrl => {
                if let Some(sel) = app.store.selection {
                    let at = app.store.playhead;
                    app.store.trim_to(sel.clip_id, at, true);
                }
            }
            Key::Char('w') if !mods.ctrl => {
                if let Some(sel) = app.store.selection {
                    let at = app.store.playhead;
                    app.store.trim_to(sel.clip_id, at, false);
                }
            }
            _ => {}
        }
    }
}
