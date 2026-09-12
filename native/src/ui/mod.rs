//! A small immediate-mode UI toolkit: input state, widget interaction, and the
//! handful of controls the editor needs. Keeping it immediate-mode is what
//! removes a whole class of bugs the DOM version had — focus, for instance, is
//! just a field here, so typing can never be interrupted by a re-render.

pub mod font;
pub mod paint;
pub mod theme;
pub mod widgets;

pub use font::Weight;
pub use paint::{mix, rgb, rgba, with_alpha, Align, Color, Painter, Rect};

/// Keys the editor cares about. Everything else arrives as text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Enter,
    Escape,
    Tab,
    Space,
    Char(char),
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cursor {
    Default,
    Hand,
    Text,
    ResizeH,
    ResizeV,
    ResizeNwSe,
    ResizeNeSw,
    Move,
}

pub type WidgetId = u64;

pub fn id_of(name: &str, index: u64) -> WidgetId {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in name.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= index.wrapping_mul(0x9e3779b97f4a7c15);
    h.wrapping_mul(0x100000001b3)
}

struct Popup {
    id: WidgetId,
    rect: Rect,
    items: Vec<String>,
    selected: usize,
    scroll: f32,
}

pub struct Ctx {
    pub painter: Painter,

    // Input, refreshed each frame.
    pub mouse: (f32, f32),
    pub mouse_delta: (f32, f32),
    pub mouse_down: bool,
    pub mouse_pressed: bool,
    pub mouse_released: bool,
    pub double_click: bool,
    pub right_pressed: bool,
    pub wheel: f32,
    pub keys: Vec<Key>,
    pub text: String,
    pub mods: Mods,

    // Interaction state, persistent across frames.
    pub hot: WidgetId,
    pub active: WidgetId,
    pub focus: WidgetId,
    /// Where the pointer went down, for drag deltas that survive jitter.
    pub drag_origin: (f32, f32),
    pub drag_payload: Option<DragPayload>,

    pub cursor: Cursor,
    /// Live buffer for the focused text field, so a half-typed value never
    /// reaches the model.
    pub edit_text: String,
    pub caret: usize,
    /// The other end of the selection. Equal to `caret` when nothing is
    /// selected, which is the only state the field had before.
    pub sel_anchor: usize,
    caret_blink: std::time::Instant,

    popup: Option<Popup>,
    popup_result: Option<(WidgetId, usize)>,
    /// Set when a widget consumed the press, so background handlers stand down.
    pub press_consumed: bool,
    pub wants_redraw: bool,
    /// Wall time the previous frame took, in milliseconds.
    pub frame_ms: f32,
    /// How long the most recent real input (mouse/keyboard) sat queued
    /// before this frame started computing — distinct from `frame_ms`,
    /// which only times the render itself. Holds its last value between
    /// renders that had no fresh input, e.g. animation-driven redraws.
    pub input_latency_ms: f32,
    /// Per-section timings for the test harness; empty without it.
    pub prof: Vec<(&'static str, f32)>,
    mark_at: std::time::Instant,
    pub tooltip: Option<(Rect, String)>,
}

/// What is currently being dragged from the library into the timeline.
#[derive(Clone, Debug)]
pub enum DragPayload {
    Asset(crate::model::Id),
    Text,
    StockPhoto(usize),
    StockVideo(usize),
}

impl Ctx {
    pub fn new(painter: Painter) -> Ctx {
        Ctx {
            painter,
            mouse: (-1.0, -1.0),
            mouse_delta: (0.0, 0.0),
            mouse_down: false,
            mouse_pressed: false,
            mouse_released: false,
            double_click: false,
            right_pressed: false,
            wheel: 0.0,
            keys: Vec::new(),
            text: String::new(),
            mods: Mods::default(),
            hot: 0,
            active: 0,
            focus: 0,
            drag_origin: (0.0, 0.0),
            drag_payload: None,
            cursor: Cursor::Default,
            edit_text: String::new(),
            caret: 0,
            sel_anchor: 0,
            caret_blink: std::time::Instant::now(),
            popup: None,
            popup_result: None,
            press_consumed: false,
            wants_redraw: true,
            frame_ms: 0.0,
            input_latency_ms: 0.0,
            prof: Vec::new(),
            mark_at: std::time::Instant::now(),
            tooltip: None,
        }
    }

    /// Records the time since the previous mark under `name`. Compiled out
    /// without the harness feature.
    #[inline]
    pub fn mark(&mut self, name: &'static str) {
        #[cfg(feature = "harness")]
        {
            let now = std::time::Instant::now();
            let dt = now.duration_since(self.mark_at).as_secs_f32() * 1000.0;
            self.mark_at = now;
            self.prof.push((name, dt));
        }
        let _ = name;
    }

    pub fn begin_frame(&mut self) {
        self.prof.clear();
        self.mark_at = std::time::Instant::now();
        self.hot = 0;
        self.cursor = Cursor::Default;
        self.press_consumed = false;
        self.tooltip = None;
    }

    pub fn end_frame(&mut self) {
        self.draw_popup();
        self.draw_tooltip();
        if self.mouse_released {
            self.active = 0;
            // A drag ends when the button comes up, wherever that happened.
            // Leaving the payload set was what made a drop outside the timeline
            // leave a ghost stuck to the cursor and drop a clip on the next
            // click — the source of most of the inconsistency between one
            // dragged item and the next.
            self.drag_payload = None;
        }
        self.keys.clear();
        self.text.clear();
        self.mouse_pressed = false;
        self.mouse_released = false;
        self.double_click = false;
        self.right_pressed = false;
        self.wheel = 0.0;
        self.mouse_delta = (0.0, 0.0);
    }

    pub fn hovered(&self, r: Rect) -> bool {
        r.contains(self.mouse.0, self.mouse.1)
    }

    /// Standard hot/active bookkeeping. Returns `(hovered, clicked)`.
    pub fn interact(&mut self, id: WidgetId, r: Rect) -> (bool, bool) {
        let hovered = self.hovered(r) && self.popup.is_none();
        if hovered {
            self.hot = id;
        }
        let mut clicked = false;
        if hovered && self.mouse_pressed && self.active == 0 {
            self.active = id;
            self.drag_origin = self.mouse;
            self.press_consumed = true;
        }
        if self.active == id && self.mouse_released {
            if hovered {
                clicked = true;
            }
            self.active = 0;
        }
        (hovered, clicked)
    }

    pub fn is_active(&self, id: WidgetId) -> bool {
        self.active == id
    }

    pub fn set_focus(&mut self, id: WidgetId, caret: usize) {
        self.focus = id;
        self.caret = caret;
        self.sel_anchor = caret;
        self.caret_blink = std::time::Instant::now();
    }

    /// The selected byte range, ordered and clamped to the buffer.
    pub fn selection(&self) -> (usize, usize) {
        let len = self.edit_text.len();
        let a = self.sel_anchor.min(self.caret).min(len);
        let b = self.sel_anchor.max(self.caret).min(len);
        (a, b)
    }

    pub fn caret_visible(&self) -> bool {
        self.caret_blink.elapsed().as_millis() % 1060 < 560
    }

    pub fn reset_caret_blink(&mut self) {
        self.caret_blink = std::time::Instant::now();
    }

    pub fn request_redraw(&mut self) {
        self.wants_redraw = true;
    }

    // ---------------------------------------------------------------- popups

    pub fn open_popup(&mut self, id: WidgetId, rect: Rect, items: Vec<String>, selected: usize) {
        self.popup = Some(Popup { id, rect, items, selected, scroll: 0.0 });
    }

    pub fn popup_open(&self, id: WidgetId) -> bool {
        self.popup.as_ref().map(|p| p.id == id).unwrap_or(false)
    }

    pub fn any_popup_open(&self) -> bool {
        self.popup.is_some()
    }

    /// Result of a popup that closed on the previous frame.
    pub fn take_popup_result(&mut self, id: WidgetId) -> Option<usize> {
        match self.popup_result {
            Some((pid, index)) if pid == id => {
                self.popup_result = None;
                Some(index)
            }
            _ => None,
        }
    }

    fn draw_popup(&mut self) {
        let Some(popup) = self.popup.take() else { return };
        let row = theme::ROW_H;
        let max_rows = 9.0;
        let visible = (popup.items.len() as f32).min(max_rows);
        let mut list = Rect::new(popup.rect.x, popup.rect.bottom() + 2.0, popup.rect.w, visible * row + 8.0);
        let (_, screen_h) = self.painter.size();
        if list.bottom() > screen_h - 4.0 {
            list.y = (popup.rect.y - list.h - 2.0).max(4.0);
        }

        let prev = self.painter.set_clip(self.painter.full_rect());
        self.painter.round_rect(
            Rect::new(list.x + 1.0, list.y + 2.0, list.w, list.h),
            theme::R_MD,
            rgba(0x000000, 90),
        );
        self.painter.round_rect(list, theme::R_MD, theme::BG_ELEV);
        self.painter.stroke_round_rect(list, theme::R_MD, theme::BORDER, 1.0);

        let inner = list.inset(4.0, 4.0);
        let clipped = self.painter.push_clip(inner);
        let mut chosen: Option<usize> = None;
        for (i, item) in popup.items.iter().enumerate() {
            let y = inner.y + i as f32 * row - popup.scroll;
            let r = Rect::new(inner.x, y, inner.w, row);
            if r.bottom() < inner.y || r.y > inner.bottom() {
                continue;
            }
            let hovered = r.contains(self.mouse.0, self.mouse.1);
            if hovered {
                self.painter.round_rect(r, theme::R_SM, theme::BG_HOVER);
            }
            let color = if i == popup.selected { theme::ACCENT_HI } else { theme::TEXT };
            self.painter
                .label(r.inset(8.0, 0.0), item, theme::FS_BODY, Weight::Regular, color, Align::Left);
            if hovered && self.mouse_pressed {
                chosen = Some(i);
            }
        }
        self.painter.set_clip(clipped);
        self.painter.set_clip(prev);

        let dismissed = self.mouse_pressed && !list.contains(self.mouse.0, self.mouse.1);
        if let Some(index) = chosen {
            self.popup_result = Some((popup.id, index));
            self.press_consumed = true;
            self.request_redraw();
        } else if dismissed || self.keys.contains(&Key::Escape) {
            self.press_consumed = true;
        } else {
            let mut popup = popup;
            let overflow = (popup.items.len() as f32 * row - inner.h).max(0.0);
            popup.scroll = (popup.scroll - self.wheel).clamp(0.0, overflow);
            self.popup = Some(popup);
            // A popup swallows the frame's input so the page beneath stays put.
            self.press_consumed = true;
        }
    }

    fn draw_tooltip(&mut self) {
        let Some((anchor, text)) = self.tooltip.take() else { return };
        let pad = 7.0;
        let w = self.painter.text_width(&text, theme::FS_SMALL, Weight::Regular) + pad * 2.0;
        let h = 22.0;
        let (screen_w, _) = self.painter.size();
        let x = (anchor.cx() - w / 2.0).clamp(4.0, screen_w - w - 4.0);
        let y = if anchor.y > h + 8.0 { anchor.y - h - 6.0 } else { anchor.bottom() + 6.0 };
        let r = Rect::new(x, y, w, h);
        let prev = self.painter.set_clip(self.painter.full_rect());
        self.painter.round_rect(r, theme::R_SM, rgba(0x000000, 220));
        self.painter.stroke_round_rect(r, theme::R_SM, theme::BORDER, 1.0);
        self.painter
            .label(r, &text, theme::FS_SMALL, Weight::Regular, theme::TEXT_2, Align::Center);
        self.painter.set_clip(prev);
    }
}

pub use theme::*;
