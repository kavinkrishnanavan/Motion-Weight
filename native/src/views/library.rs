//! The media library: imported assets, the text preset, and stock search.
//! Items here are drag sources for the timeline.

use crate::app::{App, LibraryTab};
use crate::media::format_duration;
use crate::model::{AssetKind, Id};
use crate::ui::widgets::{self, ButtonStyle, Icon};
use crate::ui::*;

const TILE_W: f32 = 104.0;
const TILE_H: f32 = 92.0;
const GAP: f32 = 10.0;

pub fn draw(app: &mut App, ctx: &mut Ctx, r: Rect) {
    ctx.painter.rect(r, BG_PANEL);
    ctx.painter.vline(r.right() - 1.0, r.y, r.bottom(), BORDER);

    // A slim header doubles as the tear-off handle.
    let (head, body) = r.split_top(PANEL_HEAD_H);
    crate::views::editor::drag_handle(app, ctx, head, crate::app::Panel::Library);
    let inner = body.inset(12.0, 0.0);

    match app.library_tab {
        LibraryTab::Media => draw_assets(app, ctx, inner),
        LibraryTab::Text => draw_text_tab(app, ctx, inner),
        LibraryTab::Stock => draw_stock(app, ctx, inner),
    }

    draw_drag_ghost(app, ctx);
}

fn draw_assets(app: &mut App, ctx: &mut Ctx, r: Rect) {
    let (head, body) = r.split_top(52.0);

    let import = Rect::new(head.x, head.y + 12.0, head.w, 30.0);
    if widgets::button(ctx, id_of("lib-import", 0), import, "Import", ButtonStyle::Primary) {
        app.import_files();
    }

    // One bin for everything. Audio sorts to the end so it does not break up
    // the visual run of thumbnails, but it is imported and dragged identically.
    let mut ids: Vec<(bool, Id)> = app
        .store
        .project
        .assets
        .iter()
        .map(|a| (a.kind == AssetKind::Audio, a.id))
        .collect();
    ids.sort_by_key(|(is_audio, _)| *is_audio);
    let ids: Vec<Id> = ids.into_iter().map(|(_, id)| id).collect();

    let (title, grid) = body.split_top(28.0);
    ctx.painter.label(
        title,
        &format!("Media  {}", ids.len()),
        FS_SMALL,
        Weight::Bold,
        TEXT_3,
        Align::Left,
    );

    if ids.is_empty() {
        let empty = Rect::new(grid.x, grid.y + 24.0, grid.w, 60.0);
        widgets::hint(
            ctx,
            empty,
            "Import photos, videos or audio to begin. Drag items onto the timeline.",
        );
        return;
    }

    draw_grid(app, ctx, grid, &ids);
}

fn draw_grid(app: &mut App, ctx: &mut Ctx, grid: Rect, ids: &[Id]) {
    let per_row = ((grid.w + GAP) / (TILE_W + GAP)).floor().max(1.0) as usize;
    let rows = (ids.len() as f32 / per_row as f32).ceil();
    let content_h = rows * (TILE_H + GAP);
    widgets::scroll(ctx, id_of("lib-scroll", 0), grid, content_h, &mut app.library_scroll);
    let prev = ctx.painter.push_clip(grid);

    for (i, asset_id) in ids.iter().enumerate() {
        let col = i % per_row;
        let row = i / per_row;
        let tile = Rect::new(
            grid.x + col as f32 * (TILE_W + GAP),
            grid.y + row as f32 * (TILE_H + GAP) - app.library_scroll,
            TILE_W,
            TILE_H,
        );
        if tile.bottom() < grid.y || tile.y > grid.bottom() {
            continue;
        }
        let Some(asset) = app.store.asset(*asset_id).cloned() else { continue };
        let id = id_of("lib-item", *asset_id);
        let (hovered, _) = ctx.interact(id, tile);
        if hovered {
            ctx.cursor = Cursor::Hand;
            ctx.tooltip = Some((tile, "Drag to the timeline · double-click to append".into()));
        }

        let (thumb_rect, name_rect) = tile.split_top(TILE_H - 20.0);
        let thumb = app.thumbs.get(asset_id).filter(|f| f.width > 1);
        widgets::thumb(ctx, thumb_rect, thumb.map(|f| (&f.rgba[..], f.width, f.height)));
        if asset.kind == AssetKind::Audio {
            widgets::draw_icon(
                ctx,
                Icon::Audio,
                Rect::new(thumb_rect.cx() - 14.0, thumb_rect.cy() - 14.0, 28.0, 28.0),
                TEXT_3,
            );
        }
        if asset.kind != AssetKind::Image {
            let badge_text = format_duration(asset.duration);
            let bw = ctx.painter.text_width(&badge_text, FS_SMALL, Weight::Regular) + 10.0;
            let badge = Rect::new(thumb_rect.right() - bw - 4.0, thumb_rect.bottom() - 18.0, bw, 14.0);
            ctx.painter.round_rect(badge, 3.0, rgba(0x000000, 190));
            ctx.painter
                .label(badge, &badge_text, FS_SMALL, Weight::Regular, TEXT_2, Align::Center);
        }
        ctx.painter
            .label(name_rect, &asset.name, FS_SMALL, Weight::Regular, TEXT_2, Align::Left);

        if ctx.is_active(id) && dragging_far(ctx) {
            ctx.drag_payload = Some(DragPayload::Asset(*asset_id));
        }
        if hovered && ctx.double_click {
            app.append_asset(*asset_id);
        }
    }
    ctx.painter.set_clip(prev);
}

fn draw_text_tab(app: &mut App, ctx: &mut Ctx, r: Rect) {
    let (title, body) = r.split_top(40.0);
    ctx.painter.label(
        Rect::new(title.x, title.y + 14.0, title.w, 22.0),
        "Text",
        FS_SMALL,
        Weight::Bold,
        TEXT_3,
        Align::Left,
    );
    let tile = Rect::new(body.x, body.y + 4.0, TILE_W, TILE_H);
    let id = id_of("lib-text", 0);
    let (hovered, _) = ctx.interact(id, tile);
    if hovered {
        ctx.cursor = Cursor::Hand;
        ctx.tooltip = Some((tile, "Drag to the timeline · double-click to add".into()));
    }
    let (preview, label) = tile.split_top(TILE_H - 20.0);
    ctx.painter.round_rect(preview, R_SM, BG_ELEV);
    ctx.painter
        .stroke_round_rect(preview, R_SM, if hovered { ACCENT } else { BORDER }, 1.0);
    ctx.painter
        .label(preview, "Text", 20.0, Weight::Bold, TEXT, Align::Center);
    ctx.painter
        .label(label, "Default text", FS_SMALL, Weight::Regular, TEXT_2, Align::Left);

    if ctx.is_active(id) && dragging_far(ctx) {
        ctx.drag_payload = Some(DragPayload::Text);
    }
    if hovered && ctx.double_click {
        app.add_text_clip(None, None);
    }
}

fn draw_stock(app: &mut App, ctx: &mut Ctx, r: Rect) {
    let (head, body) = r.split_top(52.0);
    ctx.painter.label(
        Rect::new(head.x, head.y + 4.0, head.w, 18.0),
        "Stock",
        FS_SMALL,
        Weight::Bold,
        TEXT_3,
        Align::Left,
    );

    let search = Rect::new(head.x, head.y + 22.0, head.w, FIELD_H);
    let query = app.stock_query.clone();
    if let Some(v) = widgets::text_field(ctx, id_of("stock-q", 0), search, &query, "Search photos, video, audio") {
        app.stock_query = v;
    }
    if ctx.focus == id_of("stock-q", 0) && ctx.keys.contains(&Key::Enter) {
        app.run_stock_search();
    }

    let (_, body) = body.split_top(8.0);
    let per_row = ((body.w + GAP) / (TILE_W + GAP)).floor().max(1.0) as usize;

    let section_h = |n: usize, more: bool| -> f32 {
        let rows = if n == 0 { 0.0 } else { (n as f32 / per_row as f32).ceil() };
        let status_h = if n == 0 { 20.0 } else { 0.0 };
        let more_h = if more { 34.0 } else { 0.0 };
        24.0 + rows * (TILE_H + GAP) + status_h + more_h + 14.0
    };
    let content_h = section_h(app.stock_photos.len(), app.stock_photo_more)
        + section_h(app.stock_videos.len(), app.stock_video_more)
        + section_h(app.stock_audio.len(), app.stock_audio_more);

    widgets::scroll(ctx, id_of("stock-scroll", 0), body, content_h, &mut app.stock_scroll);
    let prev = ctx.painter.push_clip(body);
    let mut y = body.y - app.stock_scroll;
    y = draw_stock_photo_section(app, ctx, body, y, per_row);
    y = draw_stock_video_section(app, ctx, body, y, per_row);
    draw_stock_audio_section(app, ctx, body, y, per_row);
    ctx.painter.set_clip(prev);
}

/// A section's empty/loading placeholder, or `None` when there is real
/// content to draw instead.
fn section_status(loading: bool, n: usize, key_missing: bool) -> Option<&'static str> {
    // Once there is anything to show, keep showing it — including while a
    // "Load more" fetch for the next page is quietly running in the background.
    if n > 0 {
        return None;
    }
    if key_missing {
        Some("Unavailable — no API key configured for this build.")
    } else if loading {
        Some("Searching...")
    } else {
        Some("No results yet — try a search above.")
    }
}

fn draw_stock_photo_section(app: &mut App, ctx: &mut Ctx, body: Rect, y: f32, per_row: usize) -> f32 {
    ctx.painter.label(
        Rect::new(body.x, y, body.w, 20.0),
        &format!("Photos  {}", app.stock_photos.len()),
        FS_SMALL,
        Weight::Bold,
        TEXT_3,
        Align::Left,
    );
    let mut y = y + 24.0;

    if let Some(msg) = section_status(app.stock_photo_loading, app.stock_photos.len(), crate::stock::api_key().is_empty()) {
        widgets::hint(ctx, Rect::new(body.x, y, body.w, 20.0), msg);
        return y + 20.0 + 14.0;
    }

    let count = app.stock_photos.len();
    let rows = (count as f32 / per_row as f32).ceil();
    let grid = Rect::new(body.x, y, body.w, rows * (TILE_H + GAP));
    let mut import: Option<usize> = None;
    for i in 0..count {
        let col = i % per_row;
        let row = i / per_row;
        let tile = Rect::new(grid.x + col as f32 * (TILE_W + GAP), grid.y + row as f32 * (TILE_H + GAP), TILE_W, TILE_H);
        if tile.bottom() < body.y || tile.y > body.bottom() {
            continue;
        }
        let id = id_of("stock-photo", i as u64);
        let (hovered, _) = ctx.interact(id, tile);
        if hovered {
            ctx.cursor = Cursor::Hand;
        }
        let (thumb_rect, name_rect) = tile.split_top(TILE_H - 20.0);
        let photo_id = app.stock_photos[i].id;
        let preview = app.stock_thumbs.get(&photo_id).filter(|f| f.width > 1);
        widgets::thumb(ctx, thumb_rect, preview.map(|f| (&f.rgba[..], f.width, f.height)));
        let name = app.stock_photos[i].photographer.clone();
        ctx.painter.label(name_rect, &name, FS_SMALL, Weight::Regular, TEXT_2, Align::Left);
        if ctx.is_active(id) && dragging_far(ctx) {
            ctx.drag_payload = Some(DragPayload::StockPhoto(i));
        }
        if hovered && ctx.double_click {
            import = Some(i);
        }
    }
    if let Some(i) = import {
        let photo = app.stock_photos[i].clone();
        app.import_stock_photo(&photo, None);
    }
    y = grid.bottom() + GAP;
    if app.stock_photo_more {
        let btn = Rect::new(body.x, y, 100.0, 26.0);
        if widgets::button(ctx, id_of("stock-photo-more", 0), btn, "Load more", ButtonStyle::Normal) {
            app.load_more_stock_photos();
        }
        y += 34.0;
    }
    y + 14.0
}

fn draw_stock_video_section(app: &mut App, ctx: &mut Ctx, body: Rect, y: f32, per_row: usize) -> f32 {
    ctx.painter.label(
        Rect::new(body.x, y, body.w, 20.0),
        &format!("Video  {}", app.stock_videos.len()),
        FS_SMALL,
        Weight::Bold,
        TEXT_3,
        Align::Left,
    );
    let mut y = y + 24.0;

    if let Some(msg) = section_status(app.stock_video_loading, app.stock_videos.len(), crate::stock::api_key().is_empty()) {
        widgets::hint(ctx, Rect::new(body.x, y, body.w, 20.0), msg);
        return y + 20.0 + 14.0;
    }

    let count = app.stock_videos.len();
    let rows = (count as f32 / per_row as f32).ceil();
    let grid = Rect::new(body.x, y, body.w, rows * (TILE_H + GAP));
    let mut import: Option<usize> = None;
    for i in 0..count {
        let col = i % per_row;
        let row = i / per_row;
        let tile = Rect::new(grid.x + col as f32 * (TILE_W + GAP), grid.y + row as f32 * (TILE_H + GAP), TILE_W, TILE_H);
        if tile.bottom() < body.y || tile.y > body.bottom() {
            continue;
        }
        let id = id_of("stock-video", i as u64);
        let (hovered, _) = ctx.interact(id, tile);
        if hovered {
            ctx.cursor = Cursor::Hand;
        }
        let (thumb_rect, name_rect) = tile.split_top(TILE_H - 20.0);
        let video_id = app.stock_videos[i].id;
        let preview = app.stock_video_thumbs.get(&video_id).filter(|f| f.width > 1);
        widgets::thumb(ctx, thumb_rect, preview.map(|f| (&f.rgba[..], f.width, f.height)));
        let badge_text = format_duration(app.stock_videos[i].duration);
        let bw = ctx.painter.text_width(&badge_text, FS_SMALL, Weight::Regular) + 10.0;
        let badge = Rect::new(thumb_rect.right() - bw - 4.0, thumb_rect.bottom() - 18.0, bw, 14.0);
        ctx.painter.round_rect(badge, 3.0, rgba(0x000000, 190));
        ctx.painter.label(badge, &badge_text, FS_SMALL, Weight::Regular, TEXT_2, Align::Center);
        let name = app.stock_videos[i].photographer.clone();
        ctx.painter.label(name_rect, &name, FS_SMALL, Weight::Regular, TEXT_2, Align::Left);
        if ctx.is_active(id) && dragging_far(ctx) {
            ctx.drag_payload = Some(DragPayload::StockVideo(i));
        }
        if hovered && ctx.double_click {
            import = Some(i);
        }
    }
    if let Some(i) = import {
        let video = app.stock_videos[i].clone();
        app.import_stock_video(&video, None);
    }
    y = grid.bottom() + GAP;
    if app.stock_video_more {
        let btn = Rect::new(body.x, y, 100.0, 26.0);
        if widgets::button(ctx, id_of("stock-video-more", 0), btn, "Load more", ButtonStyle::Normal) {
            app.load_more_stock_videos();
        }
        y += 34.0;
    }
    y + 14.0
}

fn draw_stock_audio_section(app: &mut App, ctx: &mut Ctx, body: Rect, y: f32, per_row: usize) -> f32 {
    ctx.painter.label(
        Rect::new(body.x, y, body.w, 20.0),
        &format!("Audio  {}", app.stock_audio.len()),
        FS_SMALL,
        Weight::Bold,
        TEXT_3,
        Align::Left,
    );
    let mut y = y + 24.0;

    if let Some(msg) = section_status(app.stock_audio_loading, app.stock_audio.len(), crate::freesound::api_key().is_empty()) {
        widgets::hint(ctx, Rect::new(body.x, y, body.w, 20.0), msg);
        return y + 20.0 + 14.0;
    }

    let count = app.stock_audio.len();
    let rows = (count as f32 / per_row as f32).ceil();
    let grid = Rect::new(body.x, y, body.w, rows * (TILE_H + GAP));
    let mut import: Option<usize> = None;
    for i in 0..count {
        let col = i % per_row;
        let row = i / per_row;
        let tile = Rect::new(grid.x + col as f32 * (TILE_W + GAP), grid.y + row as f32 * (TILE_H + GAP), TILE_W, TILE_H);
        if tile.bottom() < body.y || tile.y > body.bottom() {
            continue;
        }
        let id = id_of("stock-audio", i as u64);
        let (hovered, _) = ctx.interact(id, tile);
        if hovered {
            ctx.cursor = Cursor::Hand;
        }
        let (thumb_rect, name_rect) = tile.split_top(TILE_H - 20.0);
        widgets::thumb(ctx, thumb_rect, None);
        widgets::draw_icon(
            ctx,
            Icon::Audio,
            Rect::new(thumb_rect.cx() - 14.0, thumb_rect.cy() - 14.0, 28.0, 28.0),
            TEXT_3,
        );
        let badge_text = format_duration(app.stock_audio[i].duration);
        let bw = ctx.painter.text_width(&badge_text, FS_SMALL, Weight::Regular) + 10.0;
        let badge = Rect::new(thumb_rect.right() - bw - 4.0, thumb_rect.bottom() - 18.0, bw, 14.0);
        ctx.painter.round_rect(badge, 3.0, rgba(0x000000, 190));
        ctx.painter.label(badge, &badge_text, FS_SMALL, Weight::Regular, TEXT_2, Align::Center);
        let name = app.stock_audio[i].name.clone();
        ctx.painter.label(name_rect, &name, FS_SMALL, Weight::Regular, TEXT_2, Align::Left);
        if ctx.is_active(id) && dragging_far(ctx) {
            ctx.drag_payload = Some(DragPayload::StockAudio(i));
        }
        if hovered && ctx.double_click {
            import = Some(i);
        }
    }
    if let Some(i) = import {
        let audio = app.stock_audio[i].clone();
        app.import_stock_audio(&audio, None);
    }
    y = grid.bottom() + GAP;
    if app.stock_audio_more {
        let btn = Rect::new(body.x, y, 100.0, 26.0);
        if widgets::button(ctx, id_of("stock-audio-more", 0), btn, "Load more", ButtonStyle::Normal) {
            app.load_more_stock_audio();
        }
        y += 34.0;
    }
    y + 14.0
}

/// A press only becomes a drag once the pointer has actually travelled.
fn dragging_far(ctx: &Ctx) -> bool {
    let dx = ctx.mouse.0 - ctx.drag_origin.0;
    let dy = ctx.mouse.1 - ctx.drag_origin.1;
    dx.hypot(dy) > 5.0
}

fn draw_drag_ghost(app: &App, ctx: &mut Ctx) {
    let Some(payload) = ctx.drag_payload.clone() else { return };
    let label = match &payload {
        DragPayload::Asset(id) => app
            .store
            .asset(*id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Clip".into()),
        DragPayload::Text => "Text".into(),
        DragPayload::StockPhoto(i) => app
            .stock_photos
            .get(*i)
            .map(|p| p.photographer.clone())
            .unwrap_or_else(|| "Photo".into()),
        DragPayload::StockVideo(i) => app
            .stock_videos
            .get(*i)
            .map(|v| v.photographer.clone())
            .unwrap_or_else(|| "Video".into()),
        DragPayload::StockAudio(i) => app
            .stock_audio
            .get(*i)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Audio".into()),
    };
    let w = ctx.painter.text_width(&label, FS_SMALL, Weight::Regular) + 22.0;
    let r = Rect::new(ctx.mouse.0 + 12.0, ctx.mouse.1 + 10.0, w.min(180.0), 22.0);
    let prev = ctx.painter.set_clip(ctx.painter.full_rect());
    ctx.painter.round_rect(r, R_SM, rgba(0x000000, 210));
    ctx.painter.stroke_round_rect(r, R_SM, ACCENT, 1.0);
    ctx.painter
        .label(r.inset(8.0, 0.0), &label, FS_SMALL, Weight::Regular, TEXT, Align::Left);
    ctx.painter.set_clip(prev);
}
