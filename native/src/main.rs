// MotionWeight — a native, CPU-rendered video editor.
//
// No console window on Windows, in debug builds as well as release: a stray
// terminal behind the app is noise even during development.
#![windows_subsystem = "windows"]

#[cfg(feature = "harness")]
mod harness;

mod app;
mod audio;
mod brand;
mod clipboard;
mod color;
mod decode;
mod exporter;
mod ffmpeg;
mod mask;
mod media;
mod model;
mod preview;
mod projects;
mod stock;
mod store;
mod transitions;
mod ui;
mod views;

use std::num::NonZeroU32;
use std::rc::Rc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WKey, NamedKey};
use winit::window::{Window, WindowId};

/// Carves a detached pane's window into a rounded rect at the OS level, via
/// `SetWindowRgn`. Panes render with `with_decorations(false)` — no title
/// bar of their own — so without this they'd just be plain rectangles; the
/// app's fake "paint over the corners" rounding (`Painter::round_corners`)
/// only works when there's a background behind it to paint *with* (the main
/// window's gutters), which a standalone pane doesn't have.
#[cfg(windows)]
fn round_window_corners(window: &Window, radius_logical: f32) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};

    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
    let size = window.outer_size();
    let radius = (radius_logical * window.scale_factor() as f32).round() as i32;
    unsafe {
        let rgn = CreateRoundRectRgn(
            0,
            0,
            size.width as i32 + 1,
            size.height as i32 + 1,
            radius * 2,
            radius * 2,
        );
        // SetWindowRgn takes ownership of the region handle; don't free it.
        SetWindowRgn(h.hwnd.get() as _, rgn, 1);
    }
}

#[cfg(not(windows))]
fn round_window_corners(_window: &Window, _radius_logical: f32) {}

use ui::{Cursor, Ctx, Key, Mods, Painter};

const MIN_SIZE: (u32, u32) = (1100, 700);

/// Redraws driven by raw input (mouse moves in particular) are capped to this
/// interval — Windows can deliver `CursorMoved` far faster than any display
/// refreshes, and without a cap every one of those repaints the whole window
/// in software, which is what "dragging feels laggy" actually was: a CPU-bound
/// flood of redundant full frames, not a rendering-cost problem.
///
/// 8ms (~125Hz) used to be worth it for very high-refresh monitors, but it
/// outpaces the far more common 60Hz display — every one of those extra
/// presents on a windowed (non-maximized) surface still costs DWM a full
/// recomposite of the desktop behind it, work a maximized, screen-covering
/// window can often skip via a cheaper flip path. That's what made the UI
/// feel laggier windowed than maximized even though nothing on screen
/// actually changed faster than the display could show it. 16ms (~60Hz)
/// still reads as perfectly smooth for a desktop UI and roughly halves that
/// wasted composition work.
const MIN_FRAME: std::time::Duration = std::time::Duration::from_millis(16);

/// A pane that has been pulled out of the main window. It carries its own
/// surface and its own input state; the application state behind it is shared,
/// so an edit made here shows up everywhere on the next frame.
struct Pane {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    /// Kept alive only because the surface was made from it.
    _context: softbuffer::Context<Rc<Window>>,
    ctx: Ctx,
    panel: app::Panel,
    last_cursor: Cursor,
    last_click: std::time::Instant,
    last_click_pos: (f32, f32),
    last_render: std::time::Instant,
    redraw_due: bool,
    /// Whether this pane's window has, at some point since it was created,
    /// stopped overlapping the main window. Guards drag-to-redock: without
    /// it, a pane spawned right under the cursor at tear-off — still
    /// overlapping the main window it just left — would redock itself on
    /// the very next `Moved` event.
    left_dock_zone: bool,
    /// Set while the pane is being dragged and currently overlaps the main
    /// window (and has satisfied `left_dock_zone`). `Moved` events can nest
    /// inside the OS's own modal move loop, where destroying the window
    /// mid-drag is asking for trouble, so redocking itself is deferred to
    /// `about_to_wait` — reliably outside that loop, and it only ever runs
    /// once the drag has actually stopped, which doubles as "drop" rather
    /// than redocking the instant the pane brushes past the main window.
    ready_to_redock: bool,
}

pub struct Host {
    pub window: Option<Rc<Window>>,
    pub surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    pub context: Option<softbuffer::Context<Rc<Window>>>,
    pub app: Option<app::App>,
    pub ctx: Option<Ctx>,
    last_click: std::time::Instant,
    last_click_pos: (f32, f32),
    last_cursor: Cursor,
    /// When the most recent real input event (not a redraw) arrived, so
    /// `render()` can report how long it sat queued before this frame
    /// actually started computing — a very different number from
    /// `ctx.frame_ms`, which only times the render itself.
    last_input_at: Option<std::time::Instant>,
    /// When the main window was last actually repainted, so input-driven
    /// redraws can be capped to `MIN_FRAME` instead of firing on every raw
    /// `CursorMoved`.
    last_render: std::time::Instant,
    /// Set when input arrived since the last repaint but the redraw was
    /// deferred by the `MIN_FRAME` cap; `about_to_wait` turns it into an
    /// actual `request_redraw` once the interval has elapsed.
    redraw_due: bool,
    /// Detached panes, keyed by their window.
    panes: std::collections::HashMap<WindowId, Pane>,
    #[cfg(feature = "harness")]
    pub harness: harness::Harness,
}

impl Host {
    fn new() -> Host {
        Host {
            window: None,
            surface: None,
            context: None,
            app: None,
            ctx: None,
            last_click: std::time::Instant::now() - std::time::Duration::from_secs(5),
            last_click_pos: (0.0, 0.0),
            last_cursor: Cursor::Default,
            last_input_at: None,
            last_render: std::time::Instant::now() - std::time::Duration::from_secs(1),
            redraw_due: false,
            panes: std::collections::HashMap::new(),
            #[cfg(feature = "harness")]
            harness: harness::Harness::new(),
        }
    }

    fn render(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(surface), Some(ctx), Some(app)) =
            (self.window.as_ref(), self.surface.as_mut(), self.ctx.as_mut(), self.app.as_mut())
        else {
            return;
        };
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        ctx.painter.resize(w, h, window.scale_factor() as f32);

        if let Some(t) = self.last_input_at.take() {
            ctx.input_latency_ms = t.elapsed().as_secs_f32() * 1000.0;
        }

        app.window_maximized = window.is_maximized();

        let started = std::time::Instant::now();
        ctx.begin_frame();
        app.frame(ctx);
        ctx.end_frame();

        if ctx.cursor != self.last_cursor {
            window.set_cursor(cursor_icon(ctx.cursor));
            self.last_cursor = ctx.cursor;
        }

        // The app draws its own titlebar in place of the OS one; these are
        // the moves/clicks it can't perform itself, only the host can.
        if app.want_drag_window {
            let _ = window.drag_window();
        }
        match app.window_action.take() {
            Some(app::WindowAction::Minimize) => window.set_minimized(true),
            Some(app::WindowAction::ToggleMaximize) => window.set_maximized(!window.is_maximized()),
            Some(app::WindowAction::Close) => {
                app.shutdown();
                event_loop.exit();
                return;
            }
            None => {}
        }

        let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else { return };
        if surface.resize(nw, nh).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else { return };

        // tiny-skia stores premultiplied RGBA; the window wants 0x00RRGGBB.
        // The pixmap is opaque, so a straight channel shuffle is all it takes.
        // Doing it a word at a time rather than a byte at a time matters: this
        // loop runs over every pixel of the window on every single frame, and
        // the four separate byte loads defeated the vectorizer.
        ctx.mark("present.setup");
        let src = ctx.painter.pixmap.data();
        for (dst, px) in buffer.iter_mut().zip(src.chunks_exact(4)) {
            let v = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
            *dst = (v & 0x0000_ff00) | ((v & 0x00ff_0000) >> 16) | ((v & 0x0000_00ff) << 16);
        }
        let _ = buffer.present();
        if let Some(ctx) = self.ctx.as_mut() {
            ctx.mark("present.blit");
            ctx.frame_ms = started.elapsed().as_secs_f32() * 1000.0;
        }

        // Panes are repainted off the back of the main window's frame rather
        // than from `about_to_wait`. Invalidating two windows from the same
        // place, every tick, starves one of them on Windows: its WM_PAINT is
        // never reached because the other one is already dirty again.
        for pane in self.panes.values() {
            pane.window.request_redraw();
        }
        self.last_render = std::time::Instant::now();
    }

    /// Requests the next repaint, capped to `MIN_FRAME`: if enough time has
    /// passed since the last one, redraw right away; otherwise mark one as
    /// due and let `about_to_wait` fire it once the interval is up. Input
    /// state itself is never delayed — only the (expensive) repaint is.
    fn request_redraw_throttled(&mut self, event_loop: &ActiveEventLoop) {
        if self.last_render.elapsed() >= MIN_FRAME {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        } else {
            self.redraw_due = true;
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.last_render + MIN_FRAME));
        }
    }

    /// Opens windows for panes popped out this frame, and closes the ones that
    /// were docked again — whether by their own button or by the window's X.
    ///
    /// A pane torn off by dragging its header carries a `grab` point (see
    /// `editor::drag_handle`): where inside its own rect the drag started.
    /// The new window is placed so that same point lands under the cursor's
    /// *screen* position — computed from the main window's own screen
    /// position plus the last logical cursor it saw, since winit has no
    /// direct "where is the mouse on screen" query — and `drag_window()` is
    /// called on it immediately while the button is still down, handing the
    /// still-live OS drag straight to the new window. The net effect is a
    /// pane that rips free and keeps riding the cursor, like pulling a tab
    /// out of a browser's tab strip, rather than a window that pops up
    /// somewhere else while the drag dies on the old one.
    fn sync_panes(&mut self, event_loop: &ActiveEventLoop) {
        let cursor_screen: Option<(f64, f64)> = match (&self.window, &self.ctx) {
            (Some(w), Some(ctx)) => w.outer_position().ok().map(|origin| {
                let scale = w.scale_factor();
                (origin.x as f64 + ctx.mouse.0 as f64 * scale, origin.y as f64 + ctx.mouse.1 as f64 * scale)
            }),
            _ => None,
        };
        let scale = self.window.as_ref().map(|w| w.scale_factor()).unwrap_or(1.0);

        let Some(app) = self.app.as_mut() else { return };
        for (panel, grab) in app.take_detach_requests() {
            let (w, h) = panel.window_size();
            let mut attrs = Window::default_attributes()
                .with_title(format!("MotionWeight \u{2014} {}", panel.title()))
                .with_window_icon(brand::window_icon())
                .with_inner_size(winit::dpi::LogicalSize::new(w, h))
                .with_min_inner_size(winit::dpi::LogicalSize::new(260.0, 200.0))
                // The pane draws its own titlebar (app::TITLEBAR_H), styled
                // like the main window's, so the OS one is redundant.
                .with_decorations(false);
            if let Some((cx, cy)) = cursor_screen {
                attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(
                    cx - grab.0 as f64 * scale,
                    cy - grab.1 as f64 * scale,
                ));
            }
            let Ok(window) = event_loop.create_window(attrs) else { continue };
            window.set_ime_allowed(true);
            round_window_corners(&window, ui::theme::R_LG);
            // The left button is still down from the drag that triggered
            // this — hooking it now is what makes the window follow the
            // cursor instead of just appearing wherever it was placed above.
            let _ = window.drag_window();
            let window = Rc::new(window);
            let Ok(context) = softbuffer::Context::new(window.clone()) else { continue };
            let Ok(surface) = softbuffer::Surface::new(&context, window.clone()) else { continue };
            let size = window.inner_size();
            let painter = Painter::new(
                size.width.max(1),
                size.height.max(1),
                window.scale_factor() as f32,
                ui::font::Fonts::load(),
            );
            let id = window.id();
            self.panes.insert(
                id,
                Pane {
                    window,
                    surface,
                    _context: context,
                    ctx: Ctx::new(painter),
                    panel,
                    last_cursor: Cursor::Default,
                    last_click: std::time::Instant::now() - std::time::Duration::from_secs(5),
                    last_click_pos: (0.0, 0.0),
                    last_render: std::time::Instant::now() - std::time::Duration::from_secs(1),
                    redraw_due: false,
                    left_dock_zone: false,
                    ready_to_redock: false,
                },
            );
        }

        let stale: Vec<WindowId> = self
            .panes
            .iter()
            .filter(|(_, pane)| !app.is_detached(pane.panel))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            // Dropping the last handle is what actually closes the window.
            self.panes.remove(&id);
        }
    }

    fn render_pane(&mut self, id: WindowId) {
        let Some(app) = self.app.as_mut() else { return };
        let Some(pane) = self.panes.get_mut(&id) else { return };
        let size = pane.window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        pane.ctx.painter.resize(w, h, pane.window.scale_factor() as f32);

        // Only a pane the user is actually working in needs to push the main
        // window to repaint; an idle repaint would restart the storm above.
        let interacted = pane.ctx.mouse_pressed
            || pane.ctx.mouse_released
            || pane.ctx.mouse_down
            || pane.ctx.wheel.abs() > 1e-4
            || !pane.ctx.keys.is_empty()
            || !pane.ctx.text.is_empty();

        pane.ctx.begin_frame();
        let action = app.frame_panel(&mut pane.ctx, pane.panel, pane.window.is_maximized());
        pane.ctx.end_frame();

        if pane.ctx.cursor != pane.last_cursor {
            pane.window.set_cursor(cursor_icon(pane.ctx.cursor));
            pane.last_cursor = pane.ctx.cursor;
        }

        let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else { return };
        if pane.surface.resize(nw, nh).is_err() {
            return;
        }
        let Ok(mut buffer) = pane.surface.buffer_mut() else { return };
        let src = pane.ctx.painter.pixmap.data();
        for (dst, px) in buffer.iter_mut().zip(src.chunks_exact(4)) {
            let v = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
            *dst = (v & 0x0000_ff00) | ((v & 0x00ff_0000) >> 16) | ((v & 0x0000_00ff) << 16);
        }
        let _ = buffer.present();

        match action {
            app::PaneAction::Close => {
                // Closing a pane redocks it — same as the OS's own X via
                // `CloseRequested` — so this is the one action that has to
                // run after the pane borrow above ends.
                let panel = pane.panel;
                self.panes.remove(&id);
                if let Some(app) = self.app.as_mut() {
                    app.redock(panel);
                }
                if let Some(main) = &self.window {
                    main.request_redraw();
                }
                return;
            }
            app::PaneAction::Minimize => pane.window.set_minimized(true),
            app::PaneAction::ToggleMaximize => pane.window.set_maximized(!pane.window.is_maximized()),
            app::PaneAction::Drag => {
                let _ = pane.window.drag_window();
            }
            app::PaneAction::None => {}
        }

        // An edit made in a pane has to show up in the main window too.
        if interacted {
            if let Some(main) = &self.window {
                main.request_redraw();
            }
        }
        if let Some(pane) = self.panes.get_mut(&id) {
            pane.last_render = std::time::Instant::now();
        }
    }

    fn pane_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::CloseRequested) {
            if let (Some(app), Some(pane)) = (self.app.as_mut(), self.panes.get(&id)) {
                app.redock(pane.panel);
            }
            self.panes.remove(&id);
            if let Some(main) = &self.window {
                main.request_redraw();
            }
            return;
        }
        if let WindowEvent::Moved(pos) = event {
            self.check_pane_redock(id, pos);
            return;
        }
        if let WindowEvent::Resized(_) = event {
            if let Some(pane) = self.panes.get(&id) {
                // A maximized pane fills the screen like any other maximized
                // window — corners square, not clipped into a floating card.
                let radius = if pane.window.is_maximized() { 0.0 } else { ui::theme::R_LG };
                round_window_corners(&pane.window, radius);
            }
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            self.render_pane(id);
            return;
        }
        let Some(pane) = self.panes.get_mut(&id) else { return };
        apply_input(&mut pane.ctx, &event, &mut pane.last_click, &mut pane.last_click_pos);
        // Same MIN_FRAME cap as the main window: raw CursorMoved otherwise
        // floods this pane with uncapped full repaints while dragging.
        if pane.last_render.elapsed() >= MIN_FRAME {
            pane.window.request_redraw();
        } else {
            pane.redraw_due = true;
            event_loop.set_control_flow(ControlFlow::WaitUntil(pane.last_render + MIN_FRAME));
        }
    }

    /// Drag-to-redock, step one: there is no button for this any more, only
    /// dragging a detached pane's window back over the main window — like
    /// dropping a timeline clip back into a lane. `left_dock_zone` is the
    /// hysteresis that stops a pane from redocking itself the instant it's
    /// created (still overlapping the main window it was just torn out of);
    /// once it has genuinely left, crossing back over arms `ready_to_redock`
    /// for `about_to_wait` to actually act on.
    fn check_pane_redock(&mut self, id: WindowId, pos: winit::dpi::PhysicalPosition<i32>) {
        let Some(main) = &self.window else { return };
        let Ok(main_origin) = main.outer_position() else { return };
        let main_size = main.outer_size();

        let Some(pane) = self.panes.get_mut(&id) else { return };
        // Maximizing (or restoring from it) moves and resizes the window
        // programmatically, not by the user dragging it — treating that as
        // a drop-to-redock would silently dock the pane back into the main
        // window the instant the maximize button is clicked, instead of
        // actually maximizing it. Only a genuine drag, with the button held,
        // should ever count.
        if pane.window.is_maximized() || !pane.ctx.mouse_down {
            return;
        }
        let pane_size = pane.window.outer_size();
        let overlaps = rects_overlap(
            (pos.x, pos.y, pane_size.width as i32, pane_size.height as i32),
            (main_origin.x, main_origin.y, main_size.width as i32, main_size.height as i32),
        );

        if !overlaps {
            pane.left_dock_zone = true;
            pane.ready_to_redock = false;
            return;
        }
        pane.ready_to_redock = pane.left_dock_zone;
    }

    /// Drag-to-redock, step two: closes and docks every pane whose drag
    /// ended (or is merely paused) over the main window. Run from
    /// `about_to_wait` rather than straight out of the `Moved` handler,
    /// since that can fire while still nested inside the OS's own modal
    /// move loop for the very window this would destroy.
    fn drain_redocks(&mut self) {
        let ready: Vec<(WindowId, app::Panel)> = self
            .panes
            .iter()
            .filter(|(_, pane)| pane.ready_to_redock)
            .map(|(id, pane)| (*id, pane.panel))
            .collect();
        if ready.is_empty() {
            return;
        }
        for (id, panel) in ready {
            self.panes.remove(&id);
            if let Some(app) = self.app.as_mut() {
                app.redock(panel);
            }
        }
        if let Some(main) = &self.window {
            main.request_redraw();
        }
    }
}

/// Whether two axis-aligned `(x, y, w, h)` rects share any area.
fn rects_overlap(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
    a.0 < b.0 + b.2 && a.0 + a.2 > b.0 && a.1 < b.1 + b.3 && a.1 + a.3 > b.1
}

fn cursor_icon(c: Cursor) -> winit::window::CursorIcon {
    use winit::window::CursorIcon as I;
    match c {
        Cursor::Default => I::Default,
        Cursor::Hand => I::Pointer,
        Cursor::Text => I::Text,
        Cursor::ResizeH => I::EwResize,
        Cursor::ResizeV => I::NsResize,
        Cursor::ResizeNwSe => I::NwseResize,
        Cursor::ResizeNeSw => I::NeswResize,
        Cursor::Move => I::Move,
    }
}

/// Folds one window event into a context's input state. Shared by the main
/// window and every detached pane so the two can never drift apart. Returns
/// whether this was real user input, as opposed to a resize or a redraw.
fn apply_input(
    ctx: &mut Ctx,
    event: &WindowEvent,
    last_click: &mut std::time::Instant,
    last_click_pos: &mut (f32, f32),
) -> bool {
    match event {
        WindowEvent::CursorMoved { position, .. } => {
            // Input arrives in device pixels; the UI works in logical ones.
            let s = ctx.painter.scale();
            let p = (position.x as f32 / s, position.y as f32 / s);
            ctx.mouse_delta = (p.0 - ctx.mouse.0, p.1 - ctx.mouse.1);
            ctx.mouse = p;
            true
        }
        WindowEvent::CursorLeft { .. } => {
            ctx.mouse = (-1000.0, -1000.0);
            false
        }
        WindowEvent::MouseInput { state, button, .. } => match (button, state) {
            (MouseButton::Left, ElementState::Pressed) => {
                ctx.mouse_down = true;
                ctx.mouse_pressed = true;
                let near = (ctx.mouse.0 - last_click_pos.0).abs() < 4.0
                    && (ctx.mouse.1 - last_click_pos.1).abs() < 4.0;
                if near && last_click.elapsed().as_millis() < 420 {
                    ctx.double_click = true;
                }
                *last_click = std::time::Instant::now();
                *last_click_pos = ctx.mouse;
                true
            }
            (MouseButton::Left, ElementState::Released) => {
                ctx.mouse_down = false;
                ctx.mouse_released = true;
                true
            }
            (MouseButton::Right, ElementState::Pressed) => {
                ctx.right_pressed = true;
                true
            }
            _ => false,
        },
        WindowEvent::MouseWheel { delta, .. } => {
            ctx.wheel += match delta {
                MouseScrollDelta::LineDelta(_, y) => *y,
                MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
            };
            true
        }
        WindowEvent::ModifiersChanged(m) => {
            let s = m.state();
            ctx.mods = Mods { ctrl: s.control_key(), shift: s.shift_key(), alt: s.alt_key() };
            false
        }
        WindowEvent::Ime(Ime::Commit(s)) => {
            ctx.text.push_str(s);
            true
        }
        WindowEvent::KeyboardInput { event, .. } => {
            if event.state != ElementState::Pressed {
                return false;
            }
            match &event.logical_key {
                WKey::Named(named) => {
                    let key = match named {
                        NamedKey::Backspace => Some(Key::Backspace),
                        NamedKey::Delete => Some(Key::Delete),
                        NamedKey::ArrowLeft => Some(Key::Left),
                        NamedKey::ArrowRight => Some(Key::Right),
                        NamedKey::ArrowUp => Some(Key::Up),
                        NamedKey::ArrowDown => Some(Key::Down),
                        NamedKey::Home => Some(Key::Home),
                        NamedKey::End => Some(Key::End),
                        NamedKey::Enter => Some(Key::Enter),
                        NamedKey::Escape => Some(Key::Escape),
                        NamedKey::Tab => Some(Key::Tab),
                        NamedKey::Space => Some(Key::Space),
                        _ => None,
                    };
                    if let Some(k) = key {
                        ctx.keys.push(k);
                    }
                    if *named == NamedKey::Space {
                        if let Some(t) = &event.text {
                            ctx.text.push_str(t);
                        }
                    }
                }
                WKey::Character(s) => {
                    if let Some(c) = s.chars().next() {
                        ctx.keys.push(Key::Char(c.to_ascii_lowercase()));
                    }
                    if !ctx.mods.ctrl && !ctx.mods.alt {
                        ctx.text.push_str(s);
                    }
                }
                _ => {}
            }
            true
        }
        _ => false,
    }
}

impl ApplicationHandler for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("MotionWeight")
            .with_window_icon(brand::window_icon())
            .with_inner_size(winit::dpi::LogicalSize::new(1440.0, 880.0))
            .with_min_inner_size(winit::dpi::LogicalSize::new(MIN_SIZE.0 as f64, MIN_SIZE.1 as f64))
            // The app draws its own titlebar (app::TITLEBAR_H) with its own
            // minimize/maximize/close buttons, so the OS one is redundant.
            .with_decorations(false);
        let Ok(window) = event_loop.create_window(attrs) else { return };
        // The emoji picker and every other IME deliver their result as an IME
        // commit, not as key presses; without this they type nothing at all.
        window.set_ime_allowed(true);
        let window = Rc::new(window);

        let Ok(context) = softbuffer::Context::new(window.clone()) else { return };
        let Ok(surface) = softbuffer::Surface::new(&context, window.clone()) else { return };

        let size = window.inner_size();
        let fonts = ui::font::Fonts::load();
        let painter = Painter::new(
            size.width.max(1),
            size.height.max(1),
            window.scale_factor() as f32,
            fonts,
        );
        self.ctx = Some(Ctx::new(painter));
        self.app = Some(app::App::new());
        self.context = Some(context);
        self.surface = Some(surface);
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.panes.contains_key(&id) {
            self.pane_event(event_loop, id, event);
            return;
        }
        // Asking for a redraw from inside a redraw would spin the loop at 100%
        // of a core forever, so only input events schedule the next frame.
        match event {
            WindowEvent::CloseRequested => {
                if let Some(app) = self.app.as_mut() {
                    app.shutdown();
                }
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(_) => {}
            WindowEvent::RedrawRequested => {
                #[cfg(feature = "harness")]
                harness::step(self, event_loop);
                self.render(event_loop);
                return;
            }
            other => {
                let Some(ctx) = self.ctx.as_mut() else { return };
                if apply_input(ctx, &other, &mut self.last_click, &mut self.last_click_pos) {
                    self.last_input_at = Some(std::time::Instant::now());
                }
            }
        }
        self.request_redraw_throttled(event_loop);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.drain_redocks();

        let mut wake: Option<std::time::Instant> = None;
        let now = std::time::Instant::now();

        // A redraw deferred by the MIN_FRAME cap in `request_redraw_throttled`
        // (main window) or `pane_event` (a detached pane) comes due here
        // rather than being fired straight from the input handler.
        if self.redraw_due {
            let deadline = self.last_render + MIN_FRAME;
            if now >= deadline {
                self.redraw_due = false;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            } else {
                wake = Some(wake.map_or(deadline, |w| w.min(deadline)));
            }
        }
        for pane in self.panes.values_mut() {
            if !pane.redraw_due {
                continue;
            }
            let deadline = pane.last_render + MIN_FRAME;
            if now >= deadline {
                pane.redraw_due = false;
                pane.window.request_redraw();
            } else {
                wake = Some(wake.map_or(deadline, |w| w.min(deadline)));
            }
        }

        // Redraws are otherwise driven by input. We only add a timer when
        // something is actually moving, so an idle editor costs no CPU at all.
        #[cfg(feature = "harness")]
        let scripted = self.harness.running();
        #[cfg(not(feature = "harness"))]
        let scripted = false;
        let animating = self.app.as_ref().map(|a| a.wants_animation()).unwrap_or(false) || scripted;
        let blinking = self.ctx.as_ref().map(|c| c.focus != 0).unwrap_or(false);

        if animating || blinking {
            let ms = if animating { 16 } else { 250 };
            let deadline = now + std::time::Duration::from_millis(ms);
            wake = Some(wake.map_or(deadline, |w| w.min(deadline)));
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }

        event_loop.set_control_flow(match wake {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
        self.sync_panes(event_loop);
    }
}

fn main() {
    let Ok(event_loop) = EventLoop::new() else { return };
    let mut host = Host::new();
    let _ = event_loop.run_app(&mut host);
}
