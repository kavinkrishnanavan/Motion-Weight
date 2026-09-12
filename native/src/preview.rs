//! The player: composites the timeline at proxy resolution straight into the
//! window, runs the playback clock, and owns the on-canvas manipulation handles
//! for both clip geometry and mask geometry.

use crate::color::{self, Lut3D};
use crate::decode::{ClipDecoder, Frame};
use crate::mask;
use crate::model::*;
use crate::store::{Store, SNAP_TARGETS};
use crate::transitions::transition_at;
use crate::ui::paint::Blit;
use crate::ui::*;
use std::collections::HashMap;
use std::rc::Rc;

/// Editor preview composites at proxy resolution; export still renders at the
/// project's real size. This is the single biggest lever on resident memory,
/// and it is what every other editor does by default.
pub const PROXY_W: u32 = 960;

const HANDLE_PX: f32 = 7.0;
const ROTATE_KNOB_PX: f32 = 22.0;
const MIN_SIZE: f32 = 0.03;
const MAX_SIZE: f32 = 4.0;
const SNAP_PX: f32 = 7.0;
const MASK_ACCENT: Color = WARN;

/// How long a decoder may sit unused before it is dropped.
const DECODER_IDLE_S: u64 = 6;
/// Hard ceiling on concurrent video decoders, which bounds frame memory.
const MAX_DECODERS: usize = 3;
const MAX_STILLS: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
}

const HANDLES: [Handle; 8] = [
    Handle::Nw,
    Handle::N,
    Handle::Ne,
    Handle::E,
    Handle::Se,
    Handle::S,
    Handle::Sw,
    Handle::W,
];

impl Handle {
    /// Unit offsets from the box centre, in -1..1.
    fn offset(self) -> (f32, f32) {
        match self {
            Handle::Nw => (-1.0, -1.0),
            Handle::N => (0.0, -1.0),
            Handle::Ne => (1.0, -1.0),
            Handle::E => (1.0, 0.0),
            Handle::Se => (1.0, 1.0),
            Handle::S => (0.0, 1.0),
            Handle::Sw => (-1.0, 1.0),
            Handle::W => (-1.0, 0.0),
        }
    }

    fn cursor(self) -> Cursor {
        match self {
            Handle::N | Handle::S => Cursor::ResizeV,
            Handle::E | Handle::W => Cursor::ResizeH,
            Handle::Nw | Handle::Se => Cursor::ResizeNwSe,
            Handle::Ne | Handle::Sw => Cursor::ResizeNeSw,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Grab {
    Move,
    Resize(Handle),
    MaskMove,
    MaskResize(Handle),
    MaskRotate,
}

struct Drag {
    grab: Grab,
    clip_id: Id,
    origin: (f32, f32),
    /// The clip as it was when the drag began, so every frame is an absolute
    /// transform of the original rather than an accumulation of deltas.
    base: Clip,
}

struct Still {
    frame: Frame,
    touched: std::time::Instant,
}

/// A mask raster is a path fill plus a 3-pass box blur over the whole plane —
/// real work that was previously redone every single redraw, including frames
/// where nothing about the clip or its mask had changed at all. Keyed by the
/// exact inputs that affect the result, so any actual edit still invalidates it.
struct CachedMask {
    key: Mask,
    size: (u32, u32),
    plane: mask::MaskPlane,
    touched: std::time::Instant,
}

/// Same idea as `CachedMask`, for `color::apply`'s output. `frame_key` is the
/// requested local time's bit pattern — while paused it's constant frame to
/// frame, so it doubles as "is this still the same source frame" without
/// needing the decoder's own borrow alive to check its actual timestamp.
struct GradedCache {
    frame_key: u32,
    grade: ColorGrade,
    buf: Vec<u8>,
}

pub struct Preview {
    decoders: HashMap<Id, ClipDecoder>,
    stills: HashMap<Id, Still>,
    mask_cache: HashMap<Id, CachedMask>,
    /// Parsed `.cube` files, keyed by path. `None` marks a path that failed
    /// to load, so a broken reference doesn't retry a disk read every frame.
    luts: HashMap<String, Option<Rc<Lut3D>>>,
    /// Last color-graded pixel buffer per clip, so a redraw triggered by
    /// something unrelated (any other widget in the app — this is a single
    /// immediate-mode window, so every redraw repaints the whole player)
    /// doesn't redo the per-pixel grading pass over a frame that hasn't
    /// actually changed. `color::apply` walks every pixel with several
    /// float ops each; on a paused, ungraded-nothing-changed frame that
    /// cost is pure waste, and it was what made unrelated UI edits (a text
    /// color swatch, the snap toggle) stall the player for double-digit
    /// milliseconds even though the video itself was idle.
    graded_cache: HashMap<Id, GradedCache>,
    drag: Option<Drag>,
    /// Playback clock, rebased whenever the playhead moves from outside.
    wall_start: std::time::Instant,
    playhead_start: f32,
    last_written: f32,
    frame_times: std::collections::VecDeque<std::time::Instant>,
    pub measured_fps: u32,
    pub stage: Rect,
}

impl Preview {
    pub fn new() -> Preview {
        Preview {
            decoders: HashMap::new(),
            stills: HashMap::new(),
            mask_cache: HashMap::new(),
            luts: HashMap::new(),
            graded_cache: HashMap::new(),
            drag: None,
            wall_start: std::time::Instant::now(),
            playhead_start: 0.0,
            last_written: -1.0,
            frame_times: std::collections::VecDeque::new(),
            measured_fps: 0,
            stage: Rect::ZERO,
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn release(&mut self) {
        self.decoders.clear();
        self.stills.clear();
        self.mask_cache.clear();
    }

    // ------------------------------------------------------------- playback

    pub fn play(&mut self, store: &mut Store) {
        if store.playing {
            return;
        }
        if store.playhead >= store.total_duration() - 0.02 {
            store.playhead = 0.0;
        }
        store.playing = true;
        self.playhead_start = store.playhead;
        self.wall_start = std::time::Instant::now();
        self.last_written = store.playhead;
    }

    pub fn pause(&mut self, store: &mut Store) {
        store.playing = false;
        self.last_written = -1.0;
    }

    /// Advances the playhead from the wall clock. Returns true when playback
    /// reached the end and stopped.
    fn advance(&mut self, store: &mut Store) -> bool {
        if !store.playing {
            self.last_written = -1.0;
            return false;
        }
        // A seek from elsewhere (a ruler click, a keyboard nudge) rebases the
        // clock instead of being overwritten by it.
        if self.last_written >= 0.0 && (store.playhead - self.last_written).abs() > 1e-6 {
            self.playhead_start = store.playhead;
            self.wall_start = std::time::Instant::now();
        }
        let elapsed = self.wall_start.elapsed().as_secs_f32() * store.playback_rate;
        let mut t = self.playhead_start + elapsed;
        let total = store.total_duration();
        let mut ended = false;
        if t >= total {
            if store.loop_playback && total > 0.05 {
                t = 0.0;
                self.playhead_start = 0.0;
                self.wall_start = std::time::Instant::now();
            } else {
                t = total;
                store.playing = false;
                ended = true;
            }
        }
        store.playhead = t;
        self.last_written = t;
        ended
    }

    fn tick_fps(&mut self) {
        let now = std::time::Instant::now();
        self.frame_times.push_back(now);
        while self
            .frame_times
            .front()
            .map(|t| now.duration_since(*t).as_secs_f32() > 1.0)
            .unwrap_or(false)
        {
            self.frame_times.pop_front();
        }
        self.measured_fps = self.frame_times.len() as u32;
    }

    // -------------------------------------------------------------- decoding

    /// Proxy dimensions for a clip, quantized so ordinary resizing does not
    /// restart the decoder on every pixel.
    fn proxy_size(clip: &Clip, pw: u32, ph: u32) -> (u32, u32) {
        let proxy_h = (PROXY_W as f32 * ph as f32 / pw.max(1) as f32).round().max(2.0) as u32;
        let quant = |v: f32, max: u32| {
            let px = (v * max as f32).round().max(32.0);
            ((px / 32.0).round() * 32.0).clamp(32.0, max as f32) as u32
        };
        (quant(clip.width, PROXY_W), quant(clip.height, proxy_h))
    }

    fn evict(&mut self) {
        let now = std::time::Instant::now();
        self.decoders
            .retain(|_, d| now.duration_since(d.last_touched).as_secs() < DECODER_IDLE_S);
        while self.decoders.len() > MAX_DECODERS {
            let oldest = self
                .decoders
                .iter()
                .min_by_key(|(_, d)| d.last_touched)
                .map(|(id, _)| *id);
            match oldest {
                Some(id) => {
                    self.decoders.remove(&id);
                }
                None => break,
            }
        }
        while self.stills.len() > MAX_STILLS {
            let oldest = self.stills.iter().min_by_key(|(_, s)| s.touched).map(|(id, _)| *id);
            match oldest {
                Some(id) => {
                    self.stills.remove(&id);
                }
                None => break,
            }
        }
        while self.mask_cache.len() > MAX_STILLS {
            let oldest = self.mask_cache.iter().min_by_key(|(_, m)| m.touched).map(|(id, _)| *id);
            match oldest {
                Some(id) => {
                    self.mask_cache.remove(&id);
                }
                None => break,
            }
        }
        // Not on the same idle timer as the caches above — a clip whose
        // decoder or still gets evicted while still on screen (e.g. the
        // `MAX_DECODERS` cap under many tracks) would otherwise leave an
        // orphaned graded entry behind that nothing ever reads again. Cheap
        // to skip entirely on the common frame where nothing was evicted,
        // rather than building the lookup set on every single redraw.
        if self.graded_cache.len() > self.decoders.len() + self.stills.len() {
            let live: std::collections::HashSet<Id> =
                self.decoders.keys().chain(self.stills.keys()).copied().collect();
            self.graded_cache.retain(|id, _| live.contains(id));
        }
    }

    /// Loads and caches the LUT at `path` (a no-op, returning the cached
    /// result, once it's been tried once — empty `path` always misses).
    fn lut_for(&mut self, path: &str) -> Option<Rc<Lut3D>> {
        if path.is_empty() {
            return None;
        }
        if !self.luts.contains_key(path) {
            let loaded = Lut3D::load(path).map(Rc::new);
            self.luts.insert(path.to_string(), loaded);
        }
        self.luts.get(path).cloned().flatten()
    }

    /// Renders a clip's localized mask, or returns a clone of the cached plane
    /// when the mask and target size have not changed since the last frame.
    fn mask_plane_for(&mut self, clip_id: Id, local_mask: Mask, size: (u32, u32)) -> mask::MaskPlane {
        let hit = self
            .mask_cache
            .get(&clip_id)
            .is_some_and(|c| c.key == local_mask && c.size == size);
        if !hit {
            let plane = mask::render(&local_mask, size.0, size.1);
            self.mask_cache.insert(
                clip_id,
                CachedMask { key: local_mask, size, plane, touched: std::time::Instant::now() },
            );
        }
        let cached = self.mask_cache.get_mut(&clip_id).unwrap();
        cached.touched = std::time::Instant::now();
        cached.plane.clone()
    }

    fn video_frame(&mut self, clip: &Clip, path: &str, local: f32, size: (u32, u32), fps: f32) -> Option<&Frame> {
        let needs_new = match self.decoders.get(&clip.id) {
            Some(d) => d.path != path || (d.width, d.height) != size,
            None => true,
        };
        if needs_new {
            if self.decoders.len() >= MAX_DECODERS {
                self.evict();
            }
            self.decoders
                .insert(clip.id, ClipDecoder::new(path, size.0, size.1, fps, local));
        }
        self.decoders.get_mut(&clip.id)?.frame_at(local)
    }

    fn still_frame(&mut self, asset_id: Id, path: &str, size: (u32, u32)) -> Option<&Frame> {
        let stale = match self.stills.get(&asset_id) {
            Some(s) => (s.frame.width, s.frame.height) != size,
            None => true,
        };
        if stale {
            let frame = crate::decode::decode_one(path, 0.0, size.0, size.1)?;
            self.stills
                .insert(asset_id, Still { frame, touched: std::time::Instant::now() });
        }
        let s = self.stills.get_mut(&asset_id)?;
        s.touched = std::time::Instant::now();
        Some(&s.frame)
    }

    // ------------------------------------------------------------- rendering

    /// Draws the player stage and handles all interaction inside it.
    /// Returns true when playback just ended on its own.
    pub fn draw(&mut self, ctx: &mut Ctx, store: &mut Store, area: Rect) -> bool {
        let ended = self.advance(store);
        self.tick_fps();
        self.evict();

        let settings = store.project.settings;
        let stage = fit_stage(area.inset(12.0, 12.0), settings.width as f32 / settings.height as f32);
        self.stage = stage;

        ctx.painter.rect(area, BG_APP);
        ctx.painter.round_rect(stage, 2.0, rgb(0x000000));

        let prev_clip = ctx.painter.push_clip(stage);
        // `covers` is a half-open [start, end) range, so the exact instant
        // playback stops at the timeline's own end matches no clip at all —
        // the preview would otherwise go blank right when it lands there,
        // reading as "frozen" rather than as having simply reached the end.
        // Querying a hair before it instead keeps the last frame on screen;
        // `store.playhead` itself (what the timecode and ruler read) is
        // untouched.
        let t = store.playhead;
        let total = store.total_duration();
        let query_t = if total > 0.0 && t >= total { (total - 0.001).max(0.0) } else { t };
        let active = store.active_at(query_t);
        for (_, clip_id) in &active {
            let Some(clip) = store.clip(*clip_id).cloned() else { continue };
            self.draw_clip(ctx, store, &clip, query_t, stage);
        }
        ctx.painter.set_clip(prev_clip);

        if store.show_grid || self.drag.is_some() {
            draw_grid(ctx, stage);
        }

        self.interact(ctx, store, stage);
        ctx.painter.stroke_round_rect(stage, 2.0, BORDER, 1.0);
        ended
    }

    fn draw_clip(&mut self, ctx: &mut Ctx, store: &Store, clip: &Clip, t: f32, stage: Rect) {
        let st = transition_at(clip, t);
        if st.alpha <= 0.004 {
            return;
        }
        let settings = store.project.settings;

        if clip.kind == ClipKind::Text {
            let scale = stage.w / settings.width as f32;
            let size = (clip.font_size * scale * st.scale).max(4.0);
            let cx = stage.x + (clip.x + st.offset_x) * stage.w;
            let cy = stage.y + (clip.y + st.offset_y) * stage.h;
            let w = ctx.painter.text_width(&clip.text, size, weight_of(clip));
            let ascent = ctx.painter.ascent(size, weight_of(clip));
            let line = ctx.painter.line_height(size, weight_of(clip));
            let x = match clip.align {
                TextAlign::Left => cx,
                TextAlign::Center => cx - w / 2.0,
                TextAlign::Right => cx - w,
            };
            let alpha = clip.opacity * st.alpha;
            if clip.bg_enabled {
                let pad = size * 0.22;
                let bg = model_color(&clip.bg_color, alpha * 0.6);
                ctx.painter.round_rect(
                    Rect::new(x - pad, cy - line / 2.0 - pad * 0.5, w + pad * 2.0, line + pad),
                    R_SM,
                    bg,
                );
            }
            let color = model_color(&clip.color, alpha);
            ctx.painter
                .text(x, cy - line / 2.0 + ascent, &clip.text, size, weight_of(clip), color);
            return;
        }

        if clip.kind == ClipKind::Audio {
            return;
        }

        let Some(asset) = store.asset(clip.asset_id) else { return };
        let path = asset.path.clone();
        let size = Self::proxy_size(clip, settings.width, settings.height);
        let fps = settings.fps as f32;
        let local = clip.trim_in + (t - clip.start);

        // The mask is authored in frame space; rasterize it in the clip's box.
        let mask_plane = clip.mask.is_active().then(|| {
            let local_mask = mask::localize(
                &clip.mask,
                clip.x,
                clip.y,
                clip.width,
                clip.height,
                settings.width.min(settings.height) as f32,
                size.0.min(size.1) as f32,
            );
            self.mask_plane_for(clip.id, local_mask, size)
        });

        let dst = clip_rect(clip, &st, stage);
        let crop = clip.crop;
        let src = (
            crop.left.clamp(0.0, 0.95),
            crop.top.clamp(0.0, 0.95),
            (1.0 - crop.right).clamp(0.05, 1.0),
            (1.0 - crop.bottom).clamp(0.05, 1.0),
        );
        let opts_alpha = clip.opacity * st.alpha;

        // Looked up before the frame borrow below starts, so `lut_for`'s and
        // the graded-cache check's `&mut self` never has to coexist with it.
        // An image's content never changes with time, so it gets a fixed
        // key instead of one that trails the playhead — otherwise every
        // scrub would look like a new frame and miss the cache for no reason.
        let active_grade = clip.color_grade.is_active();
        let lut = active_grade.then(|| self.lut_for(&clip.color_grade.lut_path)).flatten();
        let frame_key = if clip.kind == ClipKind::Image { 0 } else { local.to_bits() };
        let cached = active_grade
            .then(|| self.graded_cache.get(&clip.id))
            .flatten()
            .filter(|c| c.frame_key == frame_key && c.grade == clip.color_grade)
            .map(|c| c.buf.clone());

        let frame = if clip.kind == ClipKind::Image {
            self.still_frame(clip.asset_id, &path, size)
        } else {
            self.video_frame(clip, &path, local.max(0.0), size, fps)
        };
        let Some(frame) = frame else { return };
        let (fw, fh) = (frame.width, frame.height);

        // `fresh` marks a buffer this call actually computed, as opposed to
        // one pulled from `cached` — no sense writing the cache entry right
        // back to the value it already held.
        let (graded, fresh) = match cached {
            Some(buf) => (Some(buf), false),
            None if active_grade => {
                let mut buf = frame.rgba.clone();
                color::apply(&mut buf, &clip.color_grade, lut.as_deref());
                (Some(buf), true)
            }
            None => (None, false),
        };
        let rgba: &[u8] = match &graded {
            Some(buf) => buf,
            None => &frame.rgba,
        };

        let user_mask = mask_plane.as_ref().map(|m| (&m.data[..], (m.width, m.height)));
        render_transitioned(ctx, rgba, fw, fh, dst, src, opts_alpha, &st, clip.id, user_mask);

        if fresh {
            if let Some(buf) = graded {
                self.graded_cache
                    .insert(clip.id, GradedCache { frame_key, grade: clip.color_grade.clone(), buf });
            }
        }
    }

    // ----------------------------------------------------------- interaction

    fn interact(&mut self, ctx: &mut Ctx, store: &mut Store, stage: Rect) {
        let Some(sel) = store.selection else {
            self.drag = None;
            return;
        };
        let Some(clip) = store.clip(sel.clip_id).cloned() else {
            self.drag = None;
            return;
        };
        if clip.kind == ClipKind::Audio {
            return;
        }

        let mask_mode = store.mask_editing && clip.kind != ClipKind::Text && clip.mask.is_active();
        let box_rect = if clip.kind == ClipKind::Text {
            text_box(ctx, &clip, store, stage)
        } else {
            clip_rect(&clip, &Default::default(), stage)
        };

        // Continue an in-flight drag first: releasing outside the stage must
        // still finish cleanly.
        if let Some(drag) = &self.drag {
            let grab = drag.grab;
            let base = drag.base.clone();
            let origin = drag.origin;
            let clip_id = drag.clip_id;
            if !ctx.mouse_down {
                self.drag = None;
                store.end_history_group();
            } else {
                let d = (ctx.mouse.0 - origin.0, ctx.mouse.1 - origin.1);
                self.apply_drag(store, clip_id, grab, &base, origin, d, ctx.mods.shift, stage);
                ctx.cursor = match grab {
                    Grab::Move | Grab::MaskMove => Cursor::Move,
                    Grab::Resize(h) | Grab::MaskResize(h) => h.cursor(),
                    Grab::MaskRotate => Cursor::Hand,
                };
            }
            self.draw_overlay(ctx, store, &clip, box_rect, mask_mode, stage);
            return;
        }

        // Hit-test, outermost first: rotate knob, handles, then the body.
        let (target_rect, rotation) = if mask_mode {
            (mask_rect(&clip.mask, stage), clip.mask.rotation.to_radians())
        } else {
            (box_rect, 0.0)
        };

        let mut grab = None;
        if mask_mode {
            let knob = mask_rotate_knob(&clip.mask, stage);
            if knob.contains(ctx.mouse.0, ctx.mouse.1) {
                grab = Some(Grab::MaskRotate);
            }
        }
        if grab.is_none() {
            for h in handles_for(&clip, mask_mode) {
                let hr = handle_rect(target_rect, h, rotation);
                if hr.contains(ctx.mouse.0, ctx.mouse.1) {
                    grab = Some(if mask_mode { Grab::MaskResize(h) } else { Grab::Resize(h) });
                    break;
                }
            }
        }
        if grab.is_none() && point_in_rotated(target_rect, rotation, ctx.mouse) {
            grab = Some(if mask_mode { Grab::MaskMove } else { Grab::Move });
        }

        if let Some(g) = grab {
            ctx.cursor = match g {
                Grab::Move | Grab::MaskMove => Cursor::Move,
                Grab::Resize(h) | Grab::MaskResize(h) => h.cursor(),
                Grab::MaskRotate => Cursor::Hand,
            };
            if ctx.mouse_pressed && !ctx.press_consumed && stage.contains(ctx.mouse.0, ctx.mouse.1) {
                store.begin_history_group();
                self.drag = Some(Drag { grab: g, clip_id: clip.id, origin: ctx.mouse, base: clip.clone() });
                ctx.press_consumed = true;
            }
        }

        self.draw_overlay(ctx, store, &clip, box_rect, mask_mode, stage);
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_drag(
        &mut self,
        store: &mut Store,
        clip_id: Id,
        grab: Grab,
        base: &Clip,
        origin: (f32, f32),
        d: (f32, f32),
        shift: bool,
        stage: Rect,
    ) {
        let snap = store.snap_to_grid;
        let Some(clip) = store.clip_mut(clip_id) else { return };
        let (dx, dy) = (d.0 / stage.w, d.1 / stage.h);

        match grab {
            Grab::Move => {
                let mut x = base.x + dx;
                let mut y = base.y + dy;
                if snap {
                    x = snap_value(x, stage.w);
                    y = snap_value(y, stage.h);
                    // Edges snap too, not just the centre.
                    if clip.kind != ClipKind::Text {
                        x = snap_edges(x, base.width, stage.w);
                        y = snap_edges(y, base.height, stage.h);
                    }
                }
                clip.x = x;
                clip.y = y;
            }
            Grab::Resize(h) => {
                if clip.kind == ClipKind::Text {
                    // Text has no box of its own; the handles scale its point size.
                    let (ox, oy) = h.offset();
                    let delta = dx * ox + dy * oy;
                    clip.font_size = (base.font_size * (1.0 + delta * 2.0)).clamp(8.0, 400.0);
                } else {
                    // The edge opposite the handle stays put, which is what makes
                    // dragging a corner feel like resizing rather than moving.
                    let (ox, oy) = h.offset();
                    let (mut l, mut r) = (base.x - base.width / 2.0, base.x + base.width / 2.0);
                    let (mut tp, mut b) = (base.y - base.height / 2.0, base.y + base.height / 2.0);
                    if ox < 0.0 {
                        l = base.x - base.width / 2.0 + dx;
                    } else if ox > 0.0 {
                        r = base.x + base.width / 2.0 + dx;
                    }
                    if oy < 0.0 {
                        tp = base.y - base.height / 2.0 + dy;
                    } else if oy > 0.0 {
                        b = base.y + base.height / 2.0 + dy;
                    }
                    if snap {
                        if ox < 0.0 {
                            l = snap_value(l, stage.w);
                        } else if ox > 0.0 {
                            r = snap_value(r, stage.w);
                        }
                        if oy < 0.0 {
                            tp = snap_value(tp, stage.h);
                        } else if oy > 0.0 {
                            b = snap_value(b, stage.h);
                        }
                    }
                    let mut w = (r - l).clamp(MIN_SIZE, MAX_SIZE);
                    let mut hh = (b - tp).clamp(MIN_SIZE, MAX_SIZE);
                    // Shift on a corner keeps the original aspect ratio.
                    if shift && ox != 0.0 && oy != 0.0 && base.height > 1e-4 {
                        let aspect = base.width / base.height;
                        if w / hh > aspect {
                            w = hh * aspect;
                        } else {
                            hh = w / aspect;
                        }
                    }
                    clip.width = w;
                    clip.height = hh;
                    clip.x = if ox < 0.0 {
                        r - w / 2.0
                    } else if ox > 0.0 {
                        l + w / 2.0
                    } else {
                        base.x
                    };
                    clip.y = if oy < 0.0 {
                        b - hh / 2.0
                    } else if oy > 0.0 {
                        tp + hh / 2.0
                    } else {
                        base.y
                    };
                }
            }
            Grab::MaskMove => {
                clip.mask.x = base.mask.x + dx;
                clip.mask.y = base.mask.y + dy;
            }
            Grab::MaskResize(h) => {
                let (ox, oy) = h.offset();
                // Resize along the mask's own axes, so a rotated mask still
                // grows in the direction the handle points.
                let rot = -base.mask.rotation.to_radians();
                let (sin, cos) = rot.sin_cos();
                let lx = dx * cos - dy * sin;
                let ly = dx * sin + dy * cos;
                if ox != 0.0 {
                    clip.mask.width = (base.mask.width + lx * ox * 2.0).clamp(0.02, 4.0);
                }
                if oy != 0.0 {
                    clip.mask.height = (base.mask.height + ly * oy * 2.0).clamp(0.02, 4.0);
                }
            }
            Grab::MaskRotate => {
                // Follow the pointer's angle about the mask centre, relative to
                // where the grab started, so the knob does not jump on grab.
                let cx = stage.x + base.mask.x * stage.w;
                let cy = stage.y + base.mask.y * stage.h;
                let start = (origin.1 - cy).atan2(origin.0 - cx);
                let now = (origin.1 + d.1 - cy).atan2(origin.0 + d.0 - cx);
                let mut deg = base.mask.rotation + (now - start).to_degrees();
                if snap {
                    // 15-degree detents unless a modifier is held.
                    let snapped = (deg / 15.0).round() * 15.0;
                    if (deg - snapped).abs() < 4.0 {
                        deg = snapped;
                    }
                }
                clip.mask.rotation = ((deg + 180.0).rem_euclid(360.0)) - 180.0;
            }
        }
        store.touch();
    }

    fn draw_overlay(&self, ctx: &mut Ctx, store: &Store, clip: &Clip, box_rect: Rect, mask_mode: bool, stage: Rect) {
        let prev = ctx.painter.push_clip(stage.inset(-20.0, -20.0));

        // The clip box is always shown; the mask box takes over the handles when
        // the inspector's mask tab is open.
        let box_color = if mask_mode { with_alpha(ACCENT, 0.45) } else { ACCENT };
        ctx.painter.stroke_round_rect(box_rect, 0.0, box_color, 1.0);
        if !mask_mode {
            for h in handles_for(clip, false) {
                let hr = handle_rect(box_rect, h, 0.0);
                ctx.painter.round_rect(hr, 1.5, [255, 255, 255, 255]);
                ctx.painter.stroke_round_rect(hr, 1.5, ACCENT, 1.0);
            }
        } else {
            let mr = mask_rect(&clip.mask, stage);
            let rot = clip.mask.rotation.to_radians();
            draw_rotated_outline(ctx, mr, rot, MASK_ACCENT);
            for h in HANDLES {
                if matches!(clip.mask.shape, MaskShape::Linear) {
                    break;
                }
                if clip.mask.shape == MaskShape::Mirror && !matches!(h, Handle::N | Handle::S) {
                    continue;
                }
                let hr = handle_rect(mr, h, rot);
                ctx.painter.round_rect(hr, 1.5, [255, 255, 255, 255]);
                ctx.painter.stroke_round_rect(hr, 1.5, MASK_ACCENT, 1.0);
            }
            let knob = mask_rotate_knob(&clip.mask, stage);
            ctx.painter.round_rect(knob, knob.w / 2.0, MASK_ACCENT);
            ctx.painter
                .line(mr.cx(), mr.y, knob.cx(), knob.bottom(), MASK_ACCENT, 1.0);
        }

        // A live readout while dragging, so numbers are visible without looking away.
        if self.drag.is_some() {
            let s = store.project.settings;
            let text = format!(
                "X {}  Y {}   W {}  H {}",
                (clip.x * s.width as f32).round() as i64,
                (clip.y * s.height as f32).round() as i64,
                (clip.width * s.width as f32).round() as i64,
                (clip.height * s.height as f32).round() as i64
            );
            let w = ctx.painter.text_width(&text, FS_SMALL, Weight::Regular) + 16.0;
            let r = Rect::new(box_rect.cx() - w / 2.0, box_rect.y - 26.0, w, 20.0);
            ctx.painter.round_rect(r, R_SM, rgba(0x000000, 200));
            ctx.painter
                .label(r, &text, FS_SMALL, Weight::Regular, TEXT, Align::Center);
        }
        ctx.painter.set_clip(prev);
    }
}

// ------------------------------------------------------------------ helpers

fn weight_of(clip: &Clip) -> Weight {
    Weight::of(clip.bold, clip.italic)
}

fn model_color(hex: &str, alpha: f32) -> Color {
    let [r, g, b] = parse_hex(hex);
    [r, g, b, (alpha.clamp(0.0, 1.0) * 255.0) as u8]
}

/// Letterboxes the project's aspect ratio inside the available area.
pub fn fit_stage(area: Rect, aspect: f32) -> Rect {
    if area.is_empty() {
        return Rect::ZERO;
    }
    let area_aspect = area.w / area.h;
    let (w, h) = if area_aspect > aspect {
        (area.h * aspect, area.h)
    } else {
        (area.w, area.w / aspect)
    };
    Rect::new(
        (area.cx() - w / 2.0).round(),
        (area.cy() - h / 2.0).round(),
        w.round(),
        h.round(),
    )
}

/// Renders one RGBA frame through a resolved transition state: shape-wipe
/// masking, ghost blur, slicing, the main blit, then the flash/sparkle
/// overlays. Shared by the real preview (`draw_clip`, which brings its own
/// `user_mask` when the clip has one) and the transitions gallery — so a
/// gallery tile is guaranteed to show exactly what the effect really looks
/// like, not a separate approximation of it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_transitioned(
    ctx: &mut Ctx,
    frame_rgba: &[u8],
    fw: u32,
    fh: u32,
    dst: Rect,
    src: (f32, f32, f32, f32),
    opts_alpha: f32,
    st: &crate::transitions::TransitionState,
    seed: Id,
    user_mask: Option<(&[u8], (u32, u32))>,
) {
    // A shape-wipe transition (iris/star/heart/diamond) contributes its own
    // coverage plane, rasterized fresh each call since its progress changes
    // every frame — there is nothing worth caching.
    let wipe_plane = (st.wipe_shape != MaskShape::None).then(|| {
        let shape = Mask {
            shape: st.wipe_shape,
            x: 0.5,
            y: 0.5,
            width: st.wipe_progress,
            height: st.wipe_progress,
            rotation: st.wipe_rotation,
            feather: st.wipe_feather,
            invert: false,
        };
        mask::render(&shape, fw, fh)
    });
    // A caller-supplied mask and the wipe plane may both be active; combining
    // them is a plain elementwise multiply once both are known to share size.
    let combined_mask: Option<Vec<u8>> = match (&user_mask, &wipe_plane) {
        (Some((a, _)), Some(b)) => {
            Some(a.iter().zip(b.data.iter()).map(|(x, y)| (*x as u16 * *y as u16 / 255) as u8).collect())
        }
        _ => None,
    };
    let mask_bytes = combined_mask
        .as_deref()
        .or_else(|| wipe_plane.as_ref().map(|m| &m.data[..]))
        .or_else(|| user_mask.map(|(a, _)| a));
    let mask_size = wipe_plane
        .as_ref()
        .map(|m| (m.width, m.height))
        .or_else(|| user_mask.map(|(_, s)| s))
        .unwrap_or((fw, fh));

    let blit_opts = Blit {
        alpha: opts_alpha,
        rotation: st.rotate,
        src,
        reveal: (st.reveal_l, st.reveal_t, st.reveal_r, st.reveal_b),
        mask: mask_bytes,
        mask_size,
    };

    // A heavily block-averaged copy of the source stands in for a real mosaic
    // pixelation — cheap, and since neighbouring pixels within a block are
    // identical, the painter's bilinear sampling on upscale doesn't smear the
    // block edges away.
    let pixelated;
    let frame_rgba: &[u8] = if st.pixelate > 0.01 {
        let block = (1.0 + st.pixelate * st.pixelate * 36.0) as u32;
        pixelated = pixelate_rgba(frame_rgba, fw, fh, block);
        &pixelated
    } else {
        frame_rgba
    };

    if st.blur > 0.004 {
        // Cheap ghost-copy blur: a couple of larger, fainter copies of the
        // same frame behind the real one read as a fast radial streak
        // without needing a real per-pixel convolution.
        for (grow, alpha_mul) in [(1.16, 0.20), (1.08, 0.30)] {
            let ghost = dst.inset(-(grow - 1.0) * dst.w * 0.5, -(grow - 1.0) * dst.h * 0.5);
            ctx.painter.blit(
                frame_rgba,
                fw,
                fh,
                ghost,
                &Blit { alpha: opts_alpha * st.blur * alpha_mul, mask: None, ..blit_opts },
            );
        }
    }

    if st.streak > 0.004 {
        // A directional smear, unlike `blur`'s symmetric radial ghost: a
        // couple of copies displaced along (streak_dx, streak_dy), fainter
        // and farther out each step.
        for (t, alpha_mul) in [(0.35, 0.22), (0.7, 0.14)] {
            let ghost = Rect::new(dst.x + st.streak_dx * dst.w * t, dst.y + st.streak_dy * dst.h * t, dst.w, dst.h);
            ctx.painter.blit(
                frame_rgba,
                fw,
                fh,
                ghost,
                &Blit { alpha: opts_alpha * st.streak * alpha_mul, mask: None, ..blit_opts },
            );
        }
    }

    if st.slices > 0 {
        draw_sliced(ctx, frame_rgba, fw, fh, dst, &blit_opts, st);
    } else {
        ctx.painter.blit(frame_rgba, fw, fh, dst, &blit_opts);
    }

    if st.wash > 0.004 {
        let [r, g, b] = st.wash_rgb;
        ctx.painter.rect(dst, [r, g, b, (st.wash.clamp(0.0, 1.0) * 255.0) as u8]);
    }
    if st.flash > 0.004 {
        ctx.painter.rect(dst, rgba(0xfff6e0, (st.flash.clamp(0.0, 1.0) * 210.0) as u8));
    }
    if st.sparkle > 0.004 {
        draw_sparkles(ctx, seed, dst, st.sparkle);
    }
}

/// Block-averages `src` down to `block`-sized cells and replicates each
/// cell's color back across itself — a flat-color mosaic, not a real
/// downscale/upscale, so it stays crisp-edged rather than blurry.
fn pixelate_rgba(src: &[u8], w: u32, h: u32, block: u32) -> Vec<u8> {
    let block = block.max(1);
    let mut out = vec![0u8; src.len()];
    let mut by = 0;
    while by < h {
        let bh = block.min(h - by);
        let mut bx = 0;
        while bx < w {
            let bw = block.min(w - bx);
            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = ((y * w + x) * 4) as usize;
                    r += src[i] as u32;
                    g += src[i + 1] as u32;
                    b += src[i + 2] as u32;
                    a += src[i + 3] as u32;
                    n += 1;
                }
            }
            let n = n.max(1);
            let (r, g, b, a) = ((r / n) as u8, (g / n) as u8, (b / n) as u8, (a / n) as u8);
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = ((y * w + x) * 4) as usize;
                    out[i] = r;
                    out[i + 1] = g;
                    out[i + 2] = b;
                    out[i + 3] = a;
                }
            }
            bx += block;
        }
        by += block;
    }
    out
}

/// Splits a frame into `st.slices` equal strips along one axis and blits
/// each independently at its own offset — a shatter/glitch look that a
/// single `blit` call cannot produce, since it can only move a whole image,
/// not tear it apart.
fn draw_sliced(ctx: &mut Ctx, rgba_data: &[u8], fw: u32, fh: u32, dst: Rect, base: &Blit, st: &crate::transitions::TransitionState) {
    let n = (st.slices.max(1) as f32).max(1.0);
    let (su0, sv0, su1, sv1) = base.src;
    for i in 0..st.slices as u32 {
        let t0 = i as f32 / n;
        let t1 = (i as f32 + 1.0) / n;
        let mut strip_dst = dst;
        let strip_src;
        if st.slice_vertical {
            strip_dst.x = dst.x + dst.w * t0;
            strip_dst.w = dst.w * (t1 - t0);
            strip_src = (su0 + (su1 - su0) * t0, sv0, su0 + (su1 - su0) * t1, sv1);
            // Shatter: strips fly outward from the centre, farther the closer
            // they are to the clip's edge.
            let from_center = (t0 + t1) / 2.0 - 0.5;
            strip_dst.x += from_center * dst.w * st.slice_offset * 1.6;
        } else {
            strip_dst.y = dst.y + dst.h * t0;
            strip_dst.h = dst.h * (t1 - t0);
            strip_src = (su0, sv0 + (sv1 - sv0) * t0, su1, sv0 + (sv1 - sv0) * t1);
            // Glitch: a small deterministic jitter per row that settles to 0
            // as the transition completes.
            strip_dst.x += strip_noise(i) * dst.w * 0.06 * st.slice_offset;
        }
        ctx.painter.blit(
            rgba_data,
            fw,
            fh,
            strip_dst,
            &Blit { src: strip_src, reveal: (0.0, 0.0, 1.0, 1.0), ..*base },
        );
    }
}

/// Deterministic pseudo-random value in -1..1 for strip index `i` — a classic
/// sine hash, good enough for a jitter and needs no RNG dependency.
fn strip_noise(i: u32) -> f32 {
    let v = ((i as f32 + 1.0) * 12.9898).sin() * 43_758.547;
    (v - v.floor()) * 2.0 - 1.0
}

/// A handful of small four-point sparkle glyphs scattered across the clip's
/// box. Positions are fixed per clip (seeded by its id) so they don't
/// re-shuffle every frame — only size and alpha animate with `intensity`.
fn draw_sparkles(ctx: &mut Ctx, seed: Id, dst: Rect, intensity: f32) {
    const COUNT: usize = 9;
    for i in 0..COUNT {
        let h1 = sparkle_hash(seed, i as u32 * 2);
        let h2 = sparkle_hash(seed, i as u32 * 2 + 1);
        let phase = sparkle_hash(seed, i as u32 * 7 + 100);
        let local_a = (intensity * (0.55 + 0.45 * phase)).clamp(0.0, 1.0);
        if local_a < 0.02 {
            continue;
        }
        let cx = dst.x + h1 * dst.w;
        let cy = dst.y + h2 * dst.h;
        let size = dst.w.min(dst.h) * (0.018 + 0.03 * phase) * (0.5 + 0.5 * local_a);
        let mut pb = tiny_skia::PathBuilder::new();
        pb.move_to(cx, cy - size);
        pb.line_to(cx + size * 0.28, cy - size * 0.28);
        pb.line_to(cx + size, cy);
        pb.line_to(cx + size * 0.28, cy + size * 0.28);
        pb.line_to(cx, cy + size);
        pb.line_to(cx - size * 0.28, cy + size * 0.28);
        pb.line_to(cx - size, cy);
        pb.line_to(cx - size * 0.28, cy - size * 0.28);
        pb.close();
        if let Some(path) = pb.finish() {
            ctx.painter.fill_path(&path, rgba(0xfff8e6, (local_a * 235.0) as u8));
        }
    }
}

/// Deterministic pseudo-random value in 0..1 for sparkle `i` of clip `seed`.
fn sparkle_hash(seed: Id, i: u32) -> f32 {
    let x = (seed as f32) * 12.9898 + (i as f32) * 78.233;
    let v = x.sin() * 43_758.547;
    v - v.floor()
}

fn clip_rect(clip: &Clip, st: &crate::transitions::TransitionState, stage: Rect) -> Rect {
    let w = clip.width * st.scale * st.scale_x * stage.w;
    let h = clip.height * st.scale * stage.h;
    let cx = stage.x + (clip.x + st.offset_x * clip.width) * stage.w;
    let cy = stage.y + (clip.y + st.offset_y * clip.height) * stage.h;
    Rect::new(cx - w / 2.0, cy - h / 2.0, w, h)
}

fn mask_rect(m: &Mask, stage: Rect) -> Rect {
    let w = m.width * stage.w;
    let h = m.height * stage.h;
    Rect::new(
        stage.x + m.x * stage.w - w / 2.0,
        stage.y + m.y * stage.h - h / 2.0,
        w,
        h,
    )
}

fn mask_rotate_knob(m: &Mask, stage: Rect) -> Rect {
    let r = mask_rect(m, stage);
    let size = 10.0;
    Rect::new(r.cx() - size / 2.0, r.y - ROTATE_KNOB_PX, size, size)
}

fn text_box(ctx: &mut Ctx, clip: &Clip, store: &Store, stage: Rect) -> Rect {
    let scale = stage.w / store.project.settings.width as f32;
    let size = (clip.font_size * scale).max(4.0);
    let w = ctx.painter.text_width(&clip.text, size, weight_of(clip)).max(12.0);
    let h = ctx.painter.line_height(size, weight_of(clip));
    let cx = stage.x + clip.x * stage.w;
    let cy = stage.y + clip.y * stage.h;
    let x = match clip.align {
        TextAlign::Left => cx,
        TextAlign::Center => cx - w / 2.0,
        TextAlign::Right => cx - w,
    };
    Rect::new(x - 4.0, cy - h / 2.0 - 2.0, w + 8.0, h + 4.0)
}

fn handles_for(clip: &Clip, mask_mode: bool) -> Vec<Handle> {
    if mask_mode {
        return HANDLES.to_vec();
    }
    if clip.kind == ClipKind::Text {
        return vec![Handle::Se, Handle::Nw];
    }
    HANDLES.to_vec()
}

fn handle_rect(r: Rect, h: Handle, rotation: f32) -> Rect {
    let (ox, oy) = h.offset();
    let (lx, ly) = (ox * r.w / 2.0, oy * r.h / 2.0);
    let (sin, cos) = rotation.sin_cos();
    let x = r.cx() + lx * cos - ly * sin;
    let y = r.cy() + lx * sin + ly * cos;
    Rect::new(x - HANDLE_PX / 2.0, y - HANDLE_PX / 2.0, HANDLE_PX, HANDLE_PX)
}

fn point_in_rotated(r: Rect, rotation: f32, p: (f32, f32)) -> bool {
    let (sin, cos) = rotation.sin_cos();
    let dx = p.0 - r.cx();
    let dy = p.1 - r.cy();
    let lx = dx * cos + dy * sin;
    let ly = -dx * sin + dy * cos;
    lx.abs() <= r.w / 2.0 && ly.abs() <= r.h / 2.0
}

fn draw_rotated_outline(ctx: &mut Ctx, r: Rect, rotation: f32, color: Color) {
    let (sin, cos) = rotation.sin_cos();
    let corner = |ox: f32, oy: f32| {
        let (lx, ly) = (ox * r.w / 2.0, oy * r.h / 2.0);
        (r.cx() + lx * cos - ly * sin, r.cy() + lx * sin + ly * cos)
    };
    let pts = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
    for i in 0..4 {
        let a = pts[i];
        let b = pts[(i + 1) % 4];
        ctx.painter.line(a.0, a.1, b.0, b.1, color, 1.0);
    }
}

/// Pulls a normalized coordinate onto the nearest grid line if it is close.
fn snap_value(v: f32, span_px: f32) -> f32 {
    for target in SNAP_TARGETS {
        if ((v - target) * span_px).abs() <= SNAP_PX {
            return target;
        }
    }
    v
}

/// Snaps a box's leading and trailing edges, not just its centre.
fn snap_edges(centre: f32, size: f32, span_px: f32) -> f32 {
    for target in SNAP_TARGETS {
        let left = centre - size / 2.0;
        if ((left - target) * span_px).abs() <= SNAP_PX {
            return target + size / 2.0;
        }
        let right = centre + size / 2.0;
        if ((right - target) * span_px).abs() <= SNAP_PX {
            return target - size / 2.0;
        }
    }
    centre
}

fn draw_grid(ctx: &mut Ctx, stage: Rect) {
    let faint = rgba(0xffffff, 34);
    let strong = rgba(0xffffff, 70);
    for t in [1.0 / 3.0, 2.0 / 3.0] {
        ctx.painter
            .vline(stage.x + stage.w * t, stage.y, stage.bottom(), faint);
        ctx.painter
            .hline(stage.x, stage.right(), stage.y + stage.h * t, faint);
    }
    ctx.painter.vline(stage.cx(), stage.y, stage.bottom(), strong);
    ctx.painter.hline(stage.x, stage.right(), stage.cy(), strong);
}
