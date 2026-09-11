//! The editor's control set. Every widget is a function that draws itself and
//! reports what the user did this frame.

use super::paint::{round_rect_path, Align, Color, Rect};
use super::theme::*;
use super::{Cursor, Ctx, Key, Weight, WidgetId};
use tiny_skia::PathBuilder;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    Normal,
    Primary,
    Danger,
}

pub fn button(ctx: &mut Ctx, id: WidgetId, r: Rect, label: &str, style: ButtonStyle) -> bool {
    let (hovered, clicked) = ctx.interact(id, r);
    let held = ctx.is_active(id) && hovered;
    if hovered {
        ctx.cursor = Cursor::Hand;
    }

    let (bg, fg, border) = match style {
        ButtonStyle::Primary => {
            let base = if held { ACCENT } else if hovered { ACCENT_HI } else { ACCENT };
            (base, [255, 255, 255, 255], base)
        }
        ButtonStyle::Danger => (
            if hovered { super::mix(BG_ELEV, DANGER, 0.25) } else { BG_ELEV },
            DANGER,
            BORDER,
        ),
        ButtonStyle::Normal => (if hovered { BG_HOVER } else { BG_ELEV }, TEXT, BORDER),
    };

    ctx.painter.round_rect(r, R_MD, bg);
    if border[3] > 0 {
        ctx.painter.stroke_round_rect(r, R_MD, border, 1.0);
    }
    ctx.painter
        .label(r.inset(8.0, 0.0), label, FS_BODY, Weight::Regular, fg, Align::Center);
    clicked
}

/// A small square button carrying an icon rather than a label.
pub fn icon_button(ctx: &mut Ctx, id: WidgetId, r: Rect, icon: Icon, tooltip: &str, on: bool) -> bool {
    let (hovered, clicked) = ctx.interact(id, r);
    if hovered {
        ctx.cursor = Cursor::Hand;
        if !tooltip.is_empty() {
            ctx.tooltip = Some((r, tooltip.to_string()));
        }
    }
    if on {
        ctx.painter.round_rect(r, R_MD, super::with_alpha(ACCENT, 0.18));
    } else if hovered {
        ctx.painter.round_rect(r, R_MD, BG_HOVER);
    }
    let color = if on { ACCENT_HI } else if hovered { TEXT } else { TEXT_2 };
    draw_icon(ctx, icon, r, color);
    clicked
}

/// A pill toggle, used for the grid and snap switches in the player info bar.
pub fn chip(ctx: &mut Ctx, id: WidgetId, r: Rect, label: &str, on: bool) -> bool {
    let (hovered, clicked) = ctx.interact(id, r);
    if hovered {
        ctx.cursor = Cursor::Hand;
    }
    let bg = if on {
        super::with_alpha(ACCENT, 0.20)
    } else if hovered {
        BG_HOVER
    } else {
        BG_ELEV
    };
    ctx.painter.round_rect(r, r.h / 2.0, bg);
    ctx.painter.stroke_round_rect(r, r.h / 2.0, if on { ACCENT } else { BORDER }, 1.0);
    let fg = if on { ACCENT_HI } else { TEXT_2 };
    ctx.painter
        .label(r.inset(9.0, 0.0), label, FS_SMALL, Weight::Regular, fg, Align::Center);
    clicked
}

/// A tab in a segmented control. Returns true when it was picked.
pub fn tab(ctx: &mut Ctx, id: WidgetId, r: Rect, label: &str, active: bool) -> bool {
    let (hovered, clicked) = ctx.interact(id, r);
    if hovered {
        ctx.cursor = Cursor::Hand;
    }
    if active {
        ctx.painter.round_rect(r, R_SM, BG_ELEV);
    } else if hovered {
        ctx.painter.round_rect(r, R_SM, BG_PANEL_2);
    }
    let fg = if active { TEXT } else { TEXT_3 };
    let weight = if active { Weight::Bold } else { Weight::Regular };
    ctx.painter.label(r, label, FS_SMALL, weight, fg, Align::Center);
    clicked
}

pub fn checkbox(ctx: &mut Ctx, id: WidgetId, r: Rect, label: &str, value: bool) -> Option<bool> {
    let (hovered, clicked) = ctx.interact(id, r);
    if hovered {
        ctx.cursor = Cursor::Hand;
    }
    let box_size = 14.0;
    let bx = Rect::new(r.x, r.cy() - box_size / 2.0, box_size, box_size);
    ctx.painter.round_rect(bx, 3.0, if value { ACCENT } else { BG_ELEV });
    ctx.painter
        .stroke_round_rect(bx, 3.0, if value { ACCENT } else { BORDER }, 1.0);
    if value {
        let mut pb = PathBuilder::new();
        pb.move_to(bx.x + 3.5, bx.cy());
        pb.line_to(bx.x + 6.0, bx.bottom() - 4.0);
        pb.line_to(bx.right() - 3.0, bx.y + 4.0);
        if let Some(p) = pb.finish() {
            ctx.painter.stroke_path(&p, [255, 255, 255, 255], 1.8);
        }
    }
    let text_area = Rect::new(bx.right() + 8.0, r.y, r.w - box_size - 8.0, r.h);
    ctx.painter.label(
        text_area,
        label,
        FS_BODY,
        Weight::Regular,
        if hovered { TEXT } else { TEXT_2 },
        Align::Left,
    );
    clicked.then_some(!value)
}

/// Horizontal slider. Returns the new value on any frame the user moves it.
pub fn slider(ctx: &mut Ctx, id: WidgetId, r: Rect, value: f32, min: f32, max: f32) -> Option<f32> {
    let (hovered, _) = ctx.interact(id, r);
    if hovered || ctx.is_active(id) {
        ctx.cursor = Cursor::Hand;
    }
    let track = Rect::new(r.x, r.cy() - 2.0, r.w, 4.0);
    let span = (max - min).max(1e-6);
    let t = ((value - min) / span).clamp(0.0, 1.0);
    let knob_r = 6.0;
    let usable = (r.w - knob_r * 2.0).max(1.0);
    let knob_x = r.x + knob_r + usable * t;

    ctx.painter.round_rect(track, 2.0, BG_ELEV);
    ctx.painter
        .round_rect(Rect::new(track.x, track.y, knob_x - track.x, track.h), 2.0, ACCENT);
    let knob = Rect::new(knob_x - knob_r, r.cy() - knob_r, knob_r * 2.0, knob_r * 2.0);
    ctx.painter.round_rect(knob, knob_r, if hovered || ctx.is_active(id) { ACCENT_HI } else { TEXT });

    if ctx.is_active(id) {
        let nt = ((ctx.mouse.0 - r.x - knob_r) / usable).clamp(0.0, 1.0);
        let nv = min + nt * span;
        if (nv - value).abs() > 1e-6 {
            return Some(nv);
        }
    }
    None
}

/// Editable text. The buffer lives in the caller; while focused, edits go
/// through `ctx.edit_text` so a partially typed value never corrupts the model.
pub fn text_field(ctx: &mut Ctx, id: WidgetId, r: Rect, value: &str, placeholder: &str) -> Option<String> {
    let (hovered, _) = ctx.interact(id, r);
    if hovered {
        ctx.cursor = Cursor::Text;
    }
    let focused = ctx.focus == id;

    if hovered && ctx.mouse_pressed {
        if !focused {
            ctx.edit_text = value.to_string();
            let caret = caret_at(ctx, r, value.to_string());
            ctx.set_focus(id, caret);
        } else if ctx.double_click {
            // Double-click takes the whole value, the same as Ctrl+A.
            ctx.sel_anchor = 0;
            ctx.caret = ctx.edit_text.len();
        } else {
            let buffer = ctx.edit_text.clone();
            ctx.caret = caret_at(ctx, r, buffer);
            ctx.sel_anchor = ctx.caret;
            ctx.reset_caret_blink();
        }
    } else if focused && ctx.is_active(id) && ctx.mouse_down && !ctx.double_click {
        // Dragging from inside the field sweeps out a selection.
        let buffer = ctx.edit_text.clone();
        ctx.caret = caret_at(ctx, r, buffer);
    } else if !hovered && ctx.mouse_pressed && focused {
        // Clicking anywhere else commits and releases focus.
        let out = ctx.edit_text.clone();
        ctx.focus = 0;
        draw_field(ctx, r, value, placeholder, false, hovered);
        return (out != value).then_some(out);
    }

    let mut result = None;
    if focused {
        let changed = edit_string(ctx);
        if ctx.keys.contains(&Key::Enter) || ctx.keys.contains(&Key::Tab) {
            let out = ctx.edit_text.clone();
            ctx.focus = 0;
            result = Some(out);
        } else if ctx.keys.contains(&Key::Escape) {
            ctx.focus = 0;
        } else if changed {
            result = Some(ctx.edit_text.clone());
        }
    }

    let shown = if ctx.focus == id { ctx.edit_text.clone() } else { value.to_string() };
    draw_field(ctx, r, &shown, placeholder, ctx.focus == id, hovered);
    result
}

/// Byte index in `text` under the pointer, in field coordinates.
fn caret_at(ctx: &mut Ctx, r: Rect, text: String) -> usize {
    let x = ctx.mouse.0 - (r.x + 8.0);
    ctx.painter.index_at_x(&text, FS_BODY, Weight::Regular, x.max(0.0))
}

fn draw_field(ctx: &mut Ctx, r: Rect, text: &str, placeholder: &str, focused: bool, hovered: bool) {
    ctx.painter.round_rect(r, R_SM, BG_ELEV);
    ctx.painter.stroke_round_rect(
        r,
        R_SM,
        if focused {
            ACCENT
        } else if hovered {
            super::mix(BORDER, TEXT_3, 0.5)
        } else {
            BORDER
        },
        1.0,
    );
    let inner = r.inset(8.0, 0.0);
    // The selection is painted under the glyphs so the text stays readable.
    if focused {
        let (a, b) = ctx.selection();
        let (a, b) = (a.min(text.len()), b.min(text.len()));
        if a < b {
            let x0 = inner.x + ctx.painter.text_width(&text[..a], FS_BODY, Weight::Regular);
            let x1 = inner.x + ctx.painter.text_width(&text[..b], FS_BODY, Weight::Regular);
            let band = Rect::new(x0, r.y + 4.0, (x1 - x0).max(1.0), r.h - 8.0);
            ctx.painter.round_rect(band, 2.0, super::with_alpha(ACCENT, 0.35));
        }
    }
    if text.is_empty() && !placeholder.is_empty() {
        ctx.painter
            .label(inner, placeholder, FS_BODY, Weight::Regular, TEXT_3, Align::Left);
    } else {
        ctx.painter.label(inner, text, FS_BODY, Weight::Regular, TEXT, Align::Left);
    }
    if focused && ctx.caret_visible() {
        let caret = ctx.caret.min(text.len());
        let prefix = &text[..caret];
        let x = inner.x + ctx.painter.text_width(prefix, FS_BODY, Weight::Regular);
        ctx.painter
            .rect(Rect::new(x.round(), r.y + 5.0, 1.0, r.h - 10.0), ACCENT_HI);
    }
}

/// Removes the selected range, if any. Returns whether the buffer changed.
fn delete_selection(ctx: &mut Ctx) -> bool {
    let (a, b) = ctx.selection();
    if a >= b {
        return false;
    }
    ctx.edit_text.replace_range(a..b, "");
    ctx.caret = a;
    ctx.sel_anchor = a;
    true
}

/// Inserts text at the caret, replacing the selection. Used by both typing and
/// pasting so the two behave identically.
fn insert(ctx: &mut Ctx, text: &str) {
    delete_selection(ctx);
    let caret = ctx.caret.min(ctx.edit_text.len());
    ctx.edit_text.insert_str(caret, text);
    ctx.caret = caret + text.len();
    ctx.sel_anchor = ctx.caret;
}

/// Applies this frame's key and text input to `ctx.edit_text`.
fn edit_string(ctx: &mut Ctx) -> bool {
    let mut changed = false;
    let keys = std::mem::take(&mut ctx.keys);
    let (ctrl, shift) = (ctx.mods.ctrl, ctx.mods.shift);
    for key in &keys {
        match key {
            Key::Backspace => {
                if delete_selection(ctx) {
                    changed = true;
                } else if ctx.caret > 0 {
                    let prev = prev_boundary(&ctx.edit_text, ctx.caret);
                    ctx.edit_text.replace_range(prev..ctx.caret, "");
                    ctx.caret = prev;
                    ctx.sel_anchor = prev;
                    changed = true;
                }
            }
            Key::Delete => {
                if delete_selection(ctx) {
                    changed = true;
                } else if ctx.caret < ctx.edit_text.len() {
                    let next = next_boundary(&ctx.edit_text, ctx.caret);
                    ctx.edit_text.replace_range(ctx.caret..next, "");
                    changed = true;
                }
            }
            // Without shift, an arrow collapses an existing selection to its
            // near edge rather than stepping past it.
            Key::Left => {
                let (a, b) = ctx.selection();
                ctx.caret = if !shift && a < b { a } else { prev_boundary(&ctx.edit_text, ctx.caret) };
                if !shift {
                    ctx.sel_anchor = ctx.caret;
                }
            }
            Key::Right => {
                let (a, b) = ctx.selection();
                ctx.caret = if !shift && a < b { b } else { next_boundary(&ctx.edit_text, ctx.caret) };
                if !shift {
                    ctx.sel_anchor = ctx.caret;
                }
            }
            Key::Home => {
                ctx.caret = 0;
                if !shift {
                    ctx.sel_anchor = 0;
                }
            }
            Key::End => {
                ctx.caret = ctx.edit_text.len();
                if !shift {
                    ctx.sel_anchor = ctx.caret;
                }
            }
            Key::Char('a') if ctrl => {
                ctx.sel_anchor = 0;
                ctx.caret = ctx.edit_text.len();
                ctx.reset_caret_blink();
            }
            Key::Char('c') if ctrl => {
                let (a, b) = ctx.selection();
                if a < b {
                    crate::clipboard::set(&ctx.edit_text[a..b]);
                }
            }
            Key::Char('x') if ctrl => {
                let (a, b) = ctx.selection();
                if a < b {
                    crate::clipboard::set(&ctx.edit_text[a..b].to_string());
                    delete_selection(ctx);
                    changed = true;
                }
            }
            Key::Char('v') if ctrl => {
                // A field is one line, so newlines and tabs from the clipboard
                // become spaces rather than invisible breaks.
                let pasted = crate::clipboard::get().map(|s| {
                    s.chars()
                        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
                        .collect::<String>()
                });
                if let Some(text) = pasted.filter(|s| !s.is_empty()) {
                    insert(ctx, &text);
                    changed = true;
                }
            }
            _ => {}
        }
    }
    ctx.keys = keys;

    let text = std::mem::take(&mut ctx.text);
    if !text.is_empty() {
        insert(ctx, &text);
        changed = true;
    }
    if changed {
        ctx.reset_caret_blink();
    }
    changed
}

fn prev_boundary(s: &str, i: usize) -> usize {
    let mut j = i.min(s.len());
    while j > 0 {
        j -= 1;
        if s.is_char_boundary(j) {
            break;
        }
    }
    j
}

fn next_boundary(s: &str, i: usize) -> usize {
    let mut j = (i + 1).min(s.len());
    while j < s.len() && !s.is_char_boundary(j) {
        j += 1;
    }
    j
}

/// A numeric field. Typing is free-form; the value only leaves when it parses.
pub fn number_field(ctx: &mut Ctx, id: WidgetId, r: Rect, value: f32, decimals: usize) -> Option<f32> {
    let shown = format_num(value, decimals);
    let out = text_field(ctx, id, r, &shown, "")?;
    out.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

pub fn format_num(value: f32, decimals: usize) -> String {
    if decimals == 0 {
        format!("{}", value.round() as i64)
    } else {
        let s = format!("{:.*}", decimals, value);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// A closed dropdown. Returns the index the user picked (one frame after the
/// click, once the popup has closed).
pub fn dropdown(ctx: &mut Ctx, id: WidgetId, r: Rect, items: &[String], selected: usize) -> Option<usize> {
    if let Some(index) = ctx.take_popup_result(id) {
        return Some(index);
    }
    let (hovered, clicked) = ctx.interact(id, r);
    if hovered {
        ctx.cursor = Cursor::Hand;
    }
    let open = ctx.popup_open(id);
    ctx.painter.round_rect(r, R_SM, BG_ELEV);
    ctx.painter
        .stroke_round_rect(r, R_SM, if open || hovered { ACCENT } else { BORDER }, 1.0);
    let label = items.get(selected).cloned().unwrap_or_default();
    ctx.painter.label(
        Rect::new(r.x + 8.0, r.y, r.w - 28.0, r.h),
        &label,
        FS_BODY,
        Weight::Regular,
        TEXT,
        Align::Left,
    );
    // Chevron.
    let cx = r.right() - 14.0;
    let cy = r.cy();
    let mut pb = PathBuilder::new();
    pb.move_to(cx - 4.0, cy - 2.0);
    pb.line_to(cx, cy + 2.5);
    pb.line_to(cx + 4.0, cy - 2.0);
    if let Some(p) = pb.finish() {
        ctx.painter.stroke_path(&p, TEXT_3, 1.5);
    }

    if clicked && !open {
        ctx.open_popup(id, r, items.to_vec(), selected);
    }
    None
}

/// Section heading inside the inspector.
pub fn group_label(ctx: &mut Ctx, r: Rect, text: &str) {
    ctx.painter
        .label(r, text, FS_SMALL, Weight::Bold, TEXT_3, Align::Left);
    ctx.painter
        .hline(r.x, r.right(), r.bottom() - 1.0, BORDER_SOFT);
}

pub fn field_label(ctx: &mut Ctx, r: Rect, text: &str) {
    ctx.painter.label(r, text, FS_SMALL, Weight::Regular, TEXT_2, Align::Left);
}

pub fn hint(ctx: &mut Ctx, r: Rect, text: &str) -> f32 {
    wrap_text(ctx, r, text, FS_SMALL, TEXT_3)
}

/// Draws wrapped text, returning the height it used.
pub fn wrap_text(ctx: &mut Ctx, r: Rect, text: &str, size: f32, color: Color) -> f32 {
    let line_h = ctx.painter.line_height(size, Weight::Regular);
    let ascent = ctx.painter.ascent(size, Weight::Regular);
    let mut y = r.y;
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{} {}", line, word) };
        if ctx.painter.text_width(&candidate, size, Weight::Regular) > r.w && !line.is_empty() {
            ctx.painter.text(r.x, y + ascent, &line, size, Weight::Regular, color);
            y += line_h;
            line = word.to_string();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        ctx.painter.text(r.x, y + ascent, &line, size, Weight::Regular, color);
        y += line_h;
    }
    y - r.y
}

// -------------------------------------------------------------------- icons

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    SkipStart,
    SkipEnd,
    Loop,
    Plus,
    Close,
    Media,
    Audio,
    Text,
    Stock,
    Trash,
    Scissors,
    Undo,
    Redo,
    CutLeft,
    CutRight,
    ZoomIn,
    ZoomOut,
    Minimize,
    Maximize,
    Restore,
}

pub fn draw_icon(ctx: &mut Ctx, icon: Icon, r: Rect, color: Color) {
    // Icons are authored on a 24x24 grid and scaled into the button.
    let s = r.w.min(r.h) * 0.62 / 24.0;
    let ox = r.cx() - 12.0 * s;
    let oy = r.cy() - 12.0 * s;
    let p = |x: f32, y: f32| (ox + x * s, oy + y * s);
    let mut pb = PathBuilder::new();
    let mut stroke_width = 1.9 * s.max(0.4);
    let mut filled = false;

    match icon {
        Icon::Play => {
            filled = true;
            let (x, y) = p(7.0, 4.5);
            pb.move_to(x, y);
            let (x, y) = p(19.5, 12.0);
            pb.line_to(x, y);
            let (x, y) = p(7.0, 19.5);
            pb.line_to(x, y);
            pb.close();
        }
        Icon::Pause => {
            filled = true;
            for x0 in [6.5, 14.0] {
                let (x, y) = p(x0, 5.0);
                pb.push_rect(tiny_skia::Rect::from_xywh(x, y, 3.5 * s, 14.0 * s).unwrap());
            }
        }
        Icon::SkipStart | Icon::SkipEnd => {
            filled = true;
            let flip = icon == Icon::SkipEnd;
            let fx = |x: f32| if flip { 24.0 - x } else { x };
            let (x, y) = p(fx(19.0), 5.0);
            pb.move_to(x, y);
            let (x, y) = p(fx(8.0), 12.0);
            pb.line_to(x, y);
            let (x, y) = p(fx(19.0), 19.0);
            pb.line_to(x, y);
            pb.close();
            let (x, y) = p(fx(6.5) - if flip { 0.0 } else { 0.0 }, 5.0);
            pb.push_rect(tiny_skia::Rect::from_xywh(x.min(x), y, 2.0 * s, 14.0 * s).unwrap());
        }
        Icon::Loop => {
            let (x, y) = p(5.0, 9.0);
            pb.move_to(x, y);
            let (x, y) = p(19.0, 9.0);
            pb.line_to(x, y);
            let (x, y) = p(15.5, 5.5);
            pb.move_to(x, y);
            let (x, y) = p(19.0, 9.0);
            pb.line_to(x, y);
            let (x, y) = p(15.5, 12.5);
            pb.line_to(x, y);
            let (x, y) = p(19.0, 15.0);
            pb.move_to(x, y);
            let (x, y) = p(5.0, 15.0);
            pb.line_to(x, y);
            let (x, y) = p(8.5, 11.5);
            pb.move_to(x, y);
            let (x, y) = p(5.0, 15.0);
            pb.line_to(x, y);
            let (x, y) = p(8.5, 18.5);
            pb.line_to(x, y);
        }
        Icon::Plus => {
            let (x, y) = p(12.0, 5.0);
            pb.move_to(x, y);
            let (x, y) = p(12.0, 19.0);
            pb.line_to(x, y);
            let (x, y) = p(5.0, 12.0);
            pb.move_to(x, y);
            let (x, y) = p(19.0, 12.0);
            pb.line_to(x, y);
            stroke_width = 2.2 * s.max(0.4);
        }
        Icon::Close => {
            let (x, y) = p(6.5, 6.5);
            pb.move_to(x, y);
            let (x, y) = p(17.5, 17.5);
            pb.line_to(x, y);
            let (x, y) = p(17.5, 6.5);
            pb.move_to(x, y);
            let (x, y) = p(6.5, 17.5);
            pb.line_to(x, y);
        }
        Icon::Media => {
            let (x, y) = p(3.5, 5.5);
            pb.push_rect(tiny_skia::Rect::from_xywh(x, y, 17.0 * s, 13.0 * s).unwrap());
            let (x, y) = p(6.0, 15.5);
            pb.move_to(x, y);
            let (x, y) = p(10.5, 10.0);
            pb.line_to(x, y);
            let (x, y) = p(14.0, 14.0);
            pb.line_to(x, y);
            let (x, y) = p(17.0, 11.0);
            pb.line_to(x, y);
            let (x, y) = p(19.5, 15.5);
            pb.line_to(x, y);
        }
        Icon::Audio => {
            for (i, h) in [5.0f32, 10.0, 15.0, 9.0, 4.0].iter().enumerate() {
                let x0 = 5.0 + i as f32 * 3.6;
                let (x, y) = p(x0, 12.0 - h / 2.0);
                pb.push_rect(tiny_skia::Rect::from_xywh(x, y, 1.9 * s, h * s).unwrap());
            }
            filled = true;
        }
        Icon::Text => {
            let (x, y) = p(5.0, 6.0);
            pb.move_to(x, y);
            let (x, y) = p(19.0, 6.0);
            pb.line_to(x, y);
            let (x, y) = p(12.0, 6.0);
            pb.move_to(x, y);
            let (x, y) = p(12.0, 19.0);
            pb.line_to(x, y);
        }
        Icon::Stock => {
            let (x, y) = p(4.0, 4.0);
            pb.push_rect(tiny_skia::Rect::from_xywh(x, y, 12.0 * s, 12.0 * s).unwrap());
            let (x, y) = p(8.0, 8.0);
            pb.push_rect(tiny_skia::Rect::from_xywh(x, y, 12.0 * s, 12.0 * s).unwrap());
        }
        Icon::Trash => {
            let (x, y) = p(6.0, 7.0);
            pb.move_to(x, y);
            let (x, y) = p(18.0, 7.0);
            pb.line_to(x, y);
            let (x, y) = p(7.5, 7.0);
            pb.move_to(x, y);
            let (x, y) = p(8.5, 19.5);
            pb.line_to(x, y);
            let (x, y) = p(15.5, 19.5);
            pb.line_to(x, y);
            let (x, y) = p(16.5, 7.0);
            pb.line_to(x, y);
            let (x, y) = p(10.0, 7.0);
            pb.move_to(x, y);
            let (x, y) = p(10.5, 4.5);
            pb.line_to(x, y);
            let (x, y) = p(13.5, 4.5);
            pb.line_to(x, y);
            let (x, y) = p(14.0, 7.0);
            pb.line_to(x, y);
        }
        Icon::Scissors => {
            let (x, y) = p(6.0, 5.0);
            pb.move_to(x, y);
            let (x, y) = p(17.0, 17.0);
            pb.line_to(x, y);
            let (x, y) = p(18.0, 5.0);
            pb.move_to(x, y);
            let (x, y) = p(7.0, 17.0);
            pb.line_to(x, y);
            let (cx, cy) = p(6.5, 19.0);
            pb.push_circle(cx, cy, 2.2 * s);
            let (cx, cy) = p(17.5, 19.0);
            pb.push_circle(cx, cy, 2.2 * s);
        }
        Icon::CutLeft | Icon::CutRight => {
            // A playhead bar with an arrow pointing away from the side that
            // gets discarded, and a short stub showing the trimmed edge.
            let flip = icon == Icon::CutRight;
            let fx = |x: f32| if flip { 24.0 - x } else { x };
            let (x, y) = p(fx(12.0), 4.0);
            pb.move_to(x, y);
            let (x, y) = p(fx(12.0), 20.0);
            pb.line_to(x, y);
            let (x, y) = p(fx(12.0), 12.0);
            pb.move_to(x, y);
            let (x, y) = p(fx(5.0), 12.0);
            pb.line_to(x, y);
            let (x, y) = p(fx(8.5), 8.5);
            pb.move_to(x, y);
            let (x, y) = p(fx(5.0), 12.0);
            pb.line_to(x, y);
            let (x, y) = p(fx(8.5), 15.5);
            pb.line_to(x, y);
            let (x, y) = p(fx(17.0), 7.0);
            pb.move_to(x, y);
            let (x, y) = p(fx(17.0), 17.0);
            pb.line_to(x, y);
        }
        Icon::ZoomIn | Icon::ZoomOut => {
            let (cx, cy) = p(12.0, 12.0);
            pb.push_circle(cx, cy, 7.5 * s);
            let (x, y) = p(7.5, 12.0);
            pb.move_to(x, y);
            let (x, y) = p(16.5, 12.0);
            pb.line_to(x, y);
            if icon == Icon::ZoomIn {
                let (x, y) = p(12.0, 7.5);
                pb.move_to(x, y);
                let (x, y) = p(12.0, 16.5);
                pb.line_to(x, y);
            }
        }
        Icon::Undo | Icon::Redo => {
            let flip = icon == Icon::Redo;
            let fx = |x: f32| if flip { 24.0 - x } else { x };
            let (x, y) = p(fx(8.0), 7.0);
            pb.move_to(x, y);
            let (x, y) = p(fx(4.5), 10.5);
            pb.line_to(x, y);
            let (x, y) = p(fx(8.0), 14.0);
            pb.line_to(x, y);
            let (x, y) = p(fx(4.5), 10.5);
            pb.move_to(x, y);
            let (x, y) = p(fx(14.0), 10.5);
            pb.line_to(x, y);
            let (x, y) = p(fx(18.5), 15.0);
            pb.line_to(x, y);
            let (x, y) = p(fx(16.0), 19.0);
            pb.line_to(x, y);
        }
        Icon::Minimize => {
            let (x, y) = p(5.0, 12.0);
            pb.move_to(x, y);
            let (x, y) = p(19.0, 12.0);
            pb.line_to(x, y);
        }
        Icon::Maximize => {
            let (x, y) = p(5.5, 5.5);
            pb.push_rect(tiny_skia::Rect::from_xywh(x, y, 13.0 * s, 13.0 * s).unwrap());
        }
        Icon::Restore => {
            let (x, y) = p(8.5, 5.5);
            pb.push_rect(tiny_skia::Rect::from_xywh(x, y, 10.0 * s, 10.0 * s).unwrap());
            let (x, y) = p(5.5, 8.5);
            pb.move_to(x, y);
            let (x, y) = p(5.5, 18.5);
            pb.line_to(x, y);
            let (x, y) = p(15.5, 18.5);
            pb.line_to(x, y);
            let (x, y) = p(15.5, 15.5);
            pb.line_to(x, y);
        }
    }

    let Some(path) = pb.finish() else { return };
    if filled {
        ctx.painter.fill_path(&path, color);
    } else {
        ctx.painter.stroke_path(&path, color, stroke_width);
    }
}

/// A thin invisible strip sitting in the gap between two panes. While it is
/// being dragged this returns the pointer's motion along the resize axis
/// (`vertical` picks Y over X) for the caller to add onto whichever side's
/// size it owns; it draws nothing itself, since the gap either side of it
/// already reads as the seam.
pub fn resize_bar(ctx: &mut Ctx, id: WidgetId, r: Rect, vertical: bool) -> f32 {
    let (hovered, _) = ctx.interact(id, r);
    if hovered || ctx.is_active(id) {
        ctx.cursor = if vertical { Cursor::ResizeV } else { Cursor::ResizeH };
    }
    if ctx.is_active(id) {
        if vertical {
            ctx.mouse_delta.1
        } else {
            ctx.mouse_delta.0
        }
    } else {
        0.0
    }
}

/// Panel background with a blank header strip (room for the tear-off handle
/// and a pop-out icon), used for every workspace pane.
pub fn panel(ctx: &mut Ctx, r: Rect) -> Rect {
    ctx.painter.rect(r, BG_PANEL);
    let (head, body) = r.split_top(PANEL_HEAD_H);
    ctx.painter.hline(r.x, r.right(), head.bottom() - 1.0, BORDER);
    body
}

/// Draws a vertical scrollbar for a scrollable region and applies the wheel.
pub fn scroll(ctx: &mut Ctx, id: WidgetId, view: Rect, content_h: f32, offset: &mut f32) {
    let overflow = (content_h - view.h).max(0.0);
    if ctx.hovered(view) && !ctx.any_popup_open() {
        *offset = (*offset - ctx.wheel * 48.0).clamp(0.0, overflow);
    }
    *offset = offset.clamp(0.0, overflow);
    if overflow <= 0.5 {
        return;
    }
    let bar_w = 5.0;
    let track = Rect::new(view.right() - bar_w - 3.0, view.y + 3.0, bar_w, view.h - 6.0);
    let thumb_h = (track.h * view.h / content_h).max(28.0);
    let t = *offset / overflow;
    let thumb = Rect::new(track.x, track.y + (track.h - thumb_h) * t, bar_w, thumb_h);
    let (hovered, _) = ctx.interact(id, track);
    ctx.painter.round_rect(track, bar_w / 2.0, super::with_alpha(BG_ELEV, 0.6));
    ctx.painter.round_rect(
        thumb,
        bar_w / 2.0,
        if hovered || ctx.is_active(id) { TEXT_3 } else { super::with_alpha(TEXT_3, 0.7) },
    );
    if ctx.is_active(id) {
        let t = ((ctx.mouse.1 - track.y - thumb_h / 2.0) / (track.h - thumb_h).max(1.0)).clamp(0.0, 1.0);
        *offset = t * overflow;
    }
}

/// A rounded thumbnail with an optional image; falls back to a neutral tile.
/// Radius for a media/project thumbnail tile — matches `R_LG` so the hover
/// accent border drawn over it (home.rs, library.rs) lines up exactly.
const THUMB_RADIUS: f32 = R_LG;

pub fn thumb(ctx: &mut Ctx, r: Rect, image: Option<(&[u8], u32, u32)>) {
    ctx.painter.round_rect(r, THUMB_RADIUS, BG_APP);
    if let Some((rgba, w, h)) = image {
        // Cover-fit inside the tile.
        let scale = (r.w / w as f32).max(r.h / h as f32);
        let (dw, dh) = (w as f32 * scale, h as f32 * scale);
        let dst = Rect::new(r.cx() - dw / 2.0, r.cy() - dh / 2.0, dw, dh);
        let prev = ctx.painter.push_clip(r);
        ctx.painter.image(rgba, w, h, dst, 1.0);
        ctx.painter.set_clip(prev);
        // `blit` only clips to the (rectangular) clip rect, so the image's
        // own corners come out square; paint them back over with the same
        // background the empty tile uses, to fake rounding on top of it.
        ctx.painter.round_corners(r, THUMB_RADIUS, BG_APP);
    }
    ctx.painter.stroke_round_rect(r, THUMB_RADIUS, BORDER_SOFT, 1.0);
    let _ = round_rect_path;
}
