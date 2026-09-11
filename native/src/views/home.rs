//! The project picker.

use crate::app::{App, Modal};
use crate::projects;
use crate::ui::widgets::{self, Icon};
use crate::ui::*;

const TILE_W: f32 = 208.0;
const TILE_H: f32 = 158.0;
const GAP: f32 = 18.0;
const CREATE_BAR_H: f32 = 76.0;
const CREATE_GRADIENT: [Color; 5] =
    [rgb(0xc1edcc), rgb(0xb0c0bc), rgb(0xa7a7a9), rgb(0x797270), rgb(0x453f3c)];

pub fn draw(app: &mut App, ctx: &mut Ctx, full: Rect) {
    ctx.painter.rect(full, BG_APP);

    let content = Rect::new(
        full.x + (full.w - full.w.min(1120.0)) / 2.0,
        full.y,
        full.w.min(1120.0),
        full.h,
    )
    .inset(28.0, 0.0);

    let (header, body) = content.split_top(120.0);
    let mark = Rect::new(header.x, header.y + 34.0, 52.0, 52.0);
    crate::brand::draw(ctx, mark);
    let text_x = mark.right() + 14.0;
    ctx.painter.label(
        Rect::new(text_x, header.y + 40.0, header.w - 66.0, 30.0),
        "MotionWeight",
        24.0,
        Weight::Bold,
        TEXT,
        Align::Left,
    );
    ctx.painter.label(
        Rect::new(text_x, header.y + 72.0, header.w - 66.0, 20.0),
        "Pro-style video editing. Just 20 MB of RAM.",
        FS_BODY,
        Weight::Regular,
        TEXT_3,
        Align::Left,
    );

    let list = projects::list();

    // A full-width bar of its own, distinct from the project grid below it —
    // the primary action shouldn't read as just another tile in the list.
    let (create, body) = body.split_top(CREATE_BAR_H);
    let (hovered, clicked) = ctx.interact(id_of("home-create", 0), create);
    if hovered {
        ctx.cursor = Cursor::Hand;
    }
    ctx.painter.round_rect_gradient(create, R_LG, &CREATE_GRADIENT);
    if hovered {
        // A translucent lift rather than swapping to a flat hover color, so
        // the gradient stays the point.
        ctx.painter.round_rect(create, R_LG, rgba(0xffffff, 22));
    }
    ctx.painter
        .stroke_round_rect(create, R_LG, if hovered { rgba(0xffffff, 130) } else { rgba(0x000000, 90) }, 1.0);

    let icon_w = 24.0;
    let gap = 10.0;
    let label = "New project";
    let label_w = ctx.painter.text_width(label, FS_BODY, Weight::Bold);
    let start_x = create.cx() - (icon_w + gap + label_w) / 2.0;
    let icon_box = Rect::new(start_x, create.cy() - 12.0, icon_w, icon_w);
    widgets::draw_icon(ctx, Icon::Plus, icon_box, TEXT);
    ctx.painter.label(
        Rect::new(icon_box.right() + gap, create.y, label_w + 4.0, create.h),
        label,
        FS_BODY,
        Weight::Bold,
        TEXT,
        Align::Left,
    );
    if clicked {
        app.modal = Modal::NewProject { name: format!("Project {}", list.len() + 1) };
        ctx.focus = 0;
    }
    let body = Rect::new(body.x, body.y + GAP, body.w, body.h - GAP);

    let per_row = ((body.w + GAP) / (TILE_W + GAP)).floor().max(1.0) as usize;
    let rows = (list.len() as f32 / per_row as f32).ceil();
    let content_h = rows * (TILE_H + GAP) + 20.0;

    widgets::scroll(ctx, id_of("home-scroll", 0), body, content_h, &mut app.home_scroll);
    let prev = ctx.painter.push_clip(body);

    let scroll = app.home_scroll;
    let tile_rect = |index: usize| {
        let col = index % per_row;
        let row = index / per_row;
        Rect::new(
            body.x + col as f32 * (TILE_W + GAP),
            body.y + row as f32 * (TILE_H + GAP) - scroll,
            TILE_W,
            TILE_H,
        )
    };

    for (i, meta) in list.iter().enumerate() {
        let r = tile_rect(i);
        if r.bottom() < body.y - 20.0 || r.y > body.bottom() + 20.0 {
            continue;
        }
        let id = id_of("home-tile", i as u64);
        let (hovered, clicked) = ctx.interact(id, r);
        if hovered {
            ctx.cursor = Cursor::Hand;
        }
        let (thumb, caption) = r.split_top(TILE_H - 42.0);
        app.ensure_project_thumb(&meta.id);
        let preview = app.project_thumbs.get(&meta.id).filter(|f| f.width > 1);
        widgets::thumb(ctx, thumb, preview.map(|f| (&f.rgba[..], f.width, f.height)));
        if hovered {
            ctx.painter.stroke_round_rect(thumb, R_LG, ACCENT, 1.5);
        }

        ctx.painter.label(
            Rect::new(caption.x + 2.0, caption.y + 2.0, caption.w - 8.0, 18.0),
            &meta.name,
            FS_BODY,
            Weight::Regular,
            TEXT,
            Align::Left,
        );
        ctx.painter.label(
            Rect::new(caption.x + 2.0, caption.y + 20.0, caption.w - 8.0, 16.0),
            &date_label(meta.updated_at),
            FS_SMALL,
            Weight::Regular,
            TEXT_3,
            Align::Left,
        );

        // Delete affordance appears on hover, in the tile's corner.
        let del = Rect::new(thumb.right() - 30.0, thumb.y + 8.0, 22.0, 22.0);
        let mut deleted = false;
        if hovered || ctx.hovered(del) {
            if widgets::icon_button(ctx, id_of("home-del", i as u64), del, Icon::Close, "Delete", false) {
                app.modal = Modal::DeleteProject { id: meta.id.clone(), name: meta.name.clone() };
                deleted = true;
            }
        }
        if clicked && !deleted && !ctx.hovered(del) {
            app.open_project(&meta.id);
        }
    }

    ctx.painter.set_clip(prev);
}

fn date_label(ms: u64) -> String {
    // Days since the epoch is enough resolution for a tile caption, and it
    // avoids pulling in a date library for one line of text.
    let days = ms / 86_400_000;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Howard Hinnant's days-from-civil, inverted. Public-domain algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
