//! The 2D painter. Everything the app draws goes through here, into a single
//! CPU pixmap that is then blitted to the window.

use super::font::{Fonts, Weight};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect as SkRect, Shader, Transform};

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub const ZERO: Rect = Rect::new(0.0, 0.0, 0.0, 0.0);

    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn inset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, (self.w - dx * 2.0).max(0.0), (self.h - dy * 2.0).max(0.0))
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect::new(x, y, (r - x).max(0.0), (b - y).max(0.0))
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    /// Splits `amount` pixels off the top, returning (taken, remainder).
    pub fn split_top(&self, amount: f32) -> (Rect, Rect) {
        let a = amount.min(self.h);
        (Rect::new(self.x, self.y, self.w, a), Rect::new(self.x, self.y + a, self.w, self.h - a))
    }

    pub fn split_bottom(&self, amount: f32) -> (Rect, Rect) {
        let a = amount.min(self.h);
        (
            Rect::new(self.x, self.bottom() - a, self.w, a),
            Rect::new(self.x, self.y, self.w, self.h - a),
        )
    }

    pub fn split_left(&self, amount: f32) -> (Rect, Rect) {
        let a = amount.min(self.w);
        (Rect::new(self.x, self.y, a, self.h), Rect::new(self.x + a, self.y, self.w - a, self.h))
    }

    pub fn split_right(&self, amount: f32) -> (Rect, Rect) {
        let a = amount.min(self.w);
        (
            Rect::new(self.right() - a, self.y, a, self.h),
            Rect::new(self.x, self.y, self.w - a, self.h),
        )
    }
}

pub type Color = [u8; 4];

pub const fn rgb(hex: u32) -> Color {
    [((hex >> 16) & 0xff) as u8, ((hex >> 8) & 0xff) as u8, (hex & 0xff) as u8, 255]
}

pub const fn rgba(hex: u32, alpha: u8) -> Color {
    [((hex >> 16) & 0xff) as u8, ((hex >> 8) & 0xff) as u8, (hex & 0xff) as u8, alpha]
}

/// Blends `over` on top of `under` by `t` (0 = under, 1 = over).
pub fn mix(under: Color, over: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
    [l(under[0], over[0]), l(under[1], over[1]), l(under[2], over[2]), l(under[3], over[3])]
}

pub fn with_alpha(c: Color, a: f32) -> Color {
    [c[0], c[1], c[2], (c[3] as f32 * a.clamp(0.0, 1.0)) as u8]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

pub struct Painter {
    pub pixmap: Pixmap,
    pub fonts: Fonts,
    /// Device pixels per logical pixel. Every public coordinate here is logical;
    /// the scale is applied once, at the point of rasterization.
    scale: f32,
    clip: Rect,
    clip_mask: Option<tiny_skia::Mask>,
    clip_mask_rect: Rect,
    /// Reused column table for the axis-aligned blit, so the hot path in the
    /// preview allocates nothing per frame.
    blit_cols: Vec<(u32, u32, u16)>,
}

/// Scales a logical rect into device space.
fn dev(r: Rect, s: f32) -> Rect {
    Rect::new(r.x * s, r.y * s, r.w * s, r.h * s)
}

impl Painter {
    pub fn new(width: u32, height: u32, scale: f32, fonts: Fonts) -> Painter {
        let pixmap = Pixmap::new(width.max(1), height.max(1)).expect("pixmap allocation");
        let scale = if scale > 0.1 { scale } else { 1.0 };
        let clip = Rect::new(0.0, 0.0, width as f32 / scale, height as f32 / scale);
        Painter { pixmap, fonts, scale, clip, clip_mask: None, clip_mask_rect: Rect::ZERO, blit_cols: Vec::new() }
    }

    pub fn resize(&mut self, width: u32, height: u32, scale: f32) {
        let scale = if scale > 0.1 { scale } else { 1.0 };
        if self.pixmap.width() == width && self.pixmap.height() == height && self.scale == scale {
            return;
        }
        self.pixmap = Pixmap::new(width.max(1), height.max(1)).expect("pixmap allocation");
        self.scale = scale;
        self.clip_mask = None;
        self.clip_mask_rect = Rect::ZERO;
        self.clip = Rect::new(0.0, 0.0, width as f32 / scale, height as f32 / scale);
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Logical size of the drawing surface.
    pub fn size(&self) -> (f32, f32) {
        (self.pixmap.width() as f32 / self.scale, self.pixmap.height() as f32 / self.scale)
    }

    pub fn full_rect(&self) -> Rect {
        let (w, h) = self.size();
        Rect::new(0.0, 0.0, w, h)
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// Sets the clip rect, returning the previous one so callers can restore it.
    pub fn set_clip(&mut self, rect: Rect) -> Rect {
        let prev = self.clip;
        self.clip = rect.intersect(&self.full_rect());
        prev
    }

    pub fn push_clip(&mut self, rect: Rect) -> Rect {
        let prev = self.clip;
        self.clip = rect.intersect(&prev);
        prev
    }

    /// True when `bounds` (logical) lies wholly inside the current clip, so the
    /// draw needs no mask at all. Almost every draw is in this case, and taking
    /// it is what keeps the clip mask from being rebuilt many times per frame.
    fn inside_clip(&self, bounds: tiny_skia::Rect) -> bool {
        let c = self.clip;
        bounds.left() >= c.x - 0.01
            && bounds.top() >= c.y - 0.01
            && bounds.right() <= c.right() + 0.01
            && bounds.bottom() <= c.bottom() + 0.01
    }

    /// Builds (or reuses) the rect clip mask. Returns false when the clip
    /// covers everything, in which case no mask is needed at all.
    fn ensure_clip_mask(&mut self) -> bool {
        if self.clip == self.full_rect() {
            return false;
        }
        if self.clip_mask.is_none() || self.clip_mask_rect != self.clip {
            let Some(mut mask) = tiny_skia::Mask::new(self.pixmap.width(), self.pixmap.height()) else {
                return false;
            };
            let c = dev(self.clip, self.scale);
            if let Some(r) = SkRect::from_xywh(c.x, c.y, c.w.max(0.01), c.h.max(0.01)) {
                let mut pb = PathBuilder::new();
                pb.push_rect(r);
                if let Some(path) = pb.finish() {
                    mask.fill_path(&path, FillRule::Winding, false, Transform::identity());
                }
            }
            self.clip_mask = Some(mask);
            self.clip_mask_rect = self.clip;
        }
        true
    }

    pub fn clear(&mut self, color: Color) {
        self.pixmap
            .fill(tiny_skia::Color::from_rgba8(color[0], color[1], color[2], color[3]));
    }

    // ------------------------------------------------------------- primitives

    pub fn rect(&mut self, r: Rect, color: Color) {
        self.round_rect(r, 0.0, color);
    }

    pub fn round_rect(&mut self, r: Rect, radius: f32, color: Color) {
        if r.is_empty() || color[3] == 0 {
            return;
        }
        // Square corners are the overwhelmingly common case (panels, rows,
        // hairlines, waveform bars). Filling them directly skips building a
        // path, allocating it, and running the anti-aliased rasterizer.
        if radius <= 0.01 {
            self.fill_rect_px(r, color);
            return;
        }
        let Some(path) = round_rect_path(r, radius) else { return };
        self.fill_path(&path, color);
    }

    /// Fills a rounded rect with a left-to-right linear gradient through
    /// `colors`, evenly spaced.
    pub fn round_rect_gradient(&mut self, r: Rect, radius: f32, colors: &[Color]) {
        if r.is_empty() || colors.is_empty() {
            return;
        }
        let Some(path) = round_rect_path(r, radius) else { return };
        let n = colors.len();
        let stops: Vec<tiny_skia::GradientStop> = colors
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let t = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
                tiny_skia::GradientStop::new(t, sk_color(*c))
            })
            .collect();
        let Some(shader) = tiny_skia::LinearGradient::new(
            tiny_skia::Point::from_xy(r.x, r.cy()),
            tiny_skia::Point::from_xy(r.right(), r.cy()),
            stops,
            tiny_skia::SpreadMode::Pad,
            Transform::identity(),
        ) else {
            return;
        };
        let clipped = !self.inside_clip(path.bounds()) && self.ensure_clip_mask();
        let Painter { pixmap, clip_mask, scale, .. } = self;
        let mut paint = Paint::default();
        paint.anti_alias = true;
        paint.shader = shader;
        let mask = if clipped { clip_mask.as_ref() } else { None };
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::from_scale(*scale, *scale), mask);
    }

    /// Paints `color` into the four corner slivers between `r` and its
    /// rounded-rect inset — the difference between the two, via even-odd
    /// fill. Used to fake rounded corners on top of a square-cornered image:
    /// `blit` has no clip-mask support of its own (only its `mask` option,
    /// which is sampled in the *image's* own space for video masks, not the
    /// destination bounds), so the cheapest way to round one is to draw it
    /// square and paint over the corners with whatever sits behind it.
    pub fn round_corners(&mut self, r: Rect, radius: f32, color: Color) {
        if r.is_empty() || radius <= 0.01 || color[3] == 0 {
            return;
        }
        let mut pb = PathBuilder::new();
        pb.push_rect(SkRect::from_xywh(r.x, r.y, r.w, r.h).unwrap());
        let rad = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
        let k = rad * 0.5523;
        let (l, t, rr, b) = (r.x, r.y, r.right(), r.bottom());
        pb.move_to(l + rad, t);
        pb.line_to(rr - rad, t);
        pb.cubic_to(rr - rad + k, t, rr, t + rad - k, rr, t + rad);
        pb.line_to(rr, b - rad);
        pb.cubic_to(rr, b - rad + k, rr - rad + k, b, rr - rad, b);
        pb.line_to(l + rad, b);
        pb.cubic_to(l + rad - k, b, l, b - rad + k, l, b - rad);
        pb.line_to(l, t + rad);
        pb.cubic_to(l, t + rad - k, l + rad - k, t, l + rad, t);
        pb.close();
        let Some(path) = pb.finish() else { return };

        let clipped = !self.inside_clip(path.bounds()) && self.ensure_clip_mask();
        let Painter { pixmap, clip_mask, scale, .. } = self;
        let mut paint = Paint::default();
        paint.anti_alias = true;
        paint.shader = Shader::SolidColor(sk_color(color));
        let mask = if clipped { clip_mask.as_ref() } else { None };
        pixmap.fill_path(&path, &paint, FillRule::EvenOdd, Transform::from_scale(*scale, *scale), mask);
    }

    /// Pixel-snapped, un-anti-aliased rectangle fill straight into the pixmap.
    fn fill_rect_px(&mut self, r: Rect, color: Color) {
        let area = r.intersect(&self.clip);
        if area.is_empty() {
            return;
        }
        let s = self.scale;
        let (pw, ph) = (self.pixmap.width() as f32, self.pixmap.height() as f32);
        let x0 = (area.x * s).round().clamp(0.0, pw) as u32;
        let y0 = (area.y * s).round().clamp(0.0, ph) as u32;
        let x1 = (area.right() * s).round().clamp(0.0, pw) as u32;
        let y1 = (area.bottom() * s).round().clamp(0.0, ph) as u32;
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let w = self.pixmap.width();
        let data = self.pixmap.pixels_mut();
        if color[3] == 255 {
            let Some(px) = tiny_skia::PremultipliedColorU8::from_rgba(color[0], color[1], color[2], 255) else {
                return;
            };
            for y in y0..y1 {
                let base = (y * w) as usize;
                data[base + x0 as usize..base + x1 as usize].fill(px);
            }
        } else {
            for y in y0..y1 {
                let base = (y * w) as usize;
                for x in x0..x1 {
                    blend_pixel(&mut data[base + x as usize], color, color[3]);
                }
            }
        }
    }

    pub fn stroke_round_rect(&mut self, r: Rect, radius: f32, color: Color, width: f32) {
        if r.is_empty() || color[3] == 0 {
            return;
        }
        let Some(path) = round_rect_path(r.inset(width / 2.0, width / 2.0), (radius - width / 2.0).max(0.0)) else {
            return;
        };
        self.stroke_path(&path, color, width);
    }

    pub fn stroke_path(&mut self, path: &tiny_skia::Path, color: Color, width: f32) {
        let mut bounds = path.bounds();
        bounds = tiny_skia::Rect::from_ltrb(
            bounds.left() - width,
            bounds.top() - width,
            bounds.right() + width,
            bounds.bottom() + width,
        )
        .unwrap_or(bounds);
        let clipped = !self.inside_clip(bounds) && self.ensure_clip_mask();
        let Painter { pixmap, clip_mask, scale, .. } = self;
        let mut paint = Paint::default();
        paint.anti_alias = true;
        paint.shader = Shader::SolidColor(sk_color(color));
        let stroke = tiny_skia::Stroke { width, ..Default::default() };
        let mask = if clipped { clip_mask.as_ref() } else { None };
        pixmap.stroke_path(path, &paint, &stroke, Transform::from_scale(*scale, *scale), mask);
    }

    pub fn fill_path(&mut self, path: &tiny_skia::Path, color: Color) {
        let clipped = !self.inside_clip(path.bounds()) && self.ensure_clip_mask();
        let Painter { pixmap, clip_mask, scale, .. } = self;
        let mut paint = Paint::default();
        paint.anti_alias = true;
        paint.shader = Shader::SolidColor(sk_color(color));
        let mask = if clipped { clip_mask.as_ref() } else { None };
        pixmap.fill_path(path, &paint, FillRule::Winding, Transform::from_scale(*scale, *scale), mask);
    }

    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Color, width: f32) {
        let mut pb = PathBuilder::new();
        pb.move_to(x0, y0);
        pb.line_to(x1, y1);
        let Some(path) = pb.finish() else { return };
        self.stroke_path(&path, color, width);
    }

    /// A connected multi-segment line, stroked as one path. Prefer this over
    /// calling `line` in a loop for anything traced point-by-point (a curve,
    /// a waveform): each `line`/`stroke_path` call builds its own path and
    /// pays for its own anti-aliased stroke fill, so tracing an N-segment
    /// curve that way is N allocations and N fills every single redraw
    /// instead of one — the cost is paid just for the widget being visible,
    /// not only while it's being interacted with.
    pub fn polyline(&mut self, pts: &[(f32, f32)], color: Color, width: f32) {
        let mut iter = pts.iter();
        let Some(&(x0, y0)) = iter.next() else { return };
        let mut pb = PathBuilder::new();
        pb.move_to(x0, y0);
        for &(x, y) in iter {
            pb.line_to(x, y);
        }
        let Some(path) = pb.finish() else { return };
        self.stroke_path(&path, color, width);
    }

    /// A one-pixel hairline, snapped to the pixel grid so it stays crisp.
    pub fn hline(&mut self, x0: f32, x1: f32, y: f32, color: Color) {
        self.rect(Rect::new(x0, y.floor(), x1 - x0, 1.0), color);
    }

    pub fn vline(&mut self, x: f32, y0: f32, y1: f32, color: Color) {
        self.rect(Rect::new(x.floor(), y0, 1.0, y1 - y0), color);
    }

    // ------------------------------------------------------------------- text

    pub fn text_width(&mut self, text: &str, size: f32, weight: Weight) -> f32 {
        self.fonts.width(text, size * self.scale, weight) / self.scale
    }

    /// Baseline offset from the top of a line box, in logical pixels.
    pub fn ascent(&self, size: f32, weight: Weight) -> f32 {
        self.fonts.ascent(size * self.scale, weight) / self.scale
    }

    pub fn line_height(&self, size: f32, weight: Weight) -> f32 {
        self.fonts.line_height(size * self.scale, weight) / self.scale
    }

    pub fn ellipsize(&mut self, text: &str, size: f32, weight: Weight, max_width: f32) -> String {
        let s = self.scale;
        self.fonts.ellipsize(text, size * s, weight, max_width * s)
    }

    /// Byte index of the character boundary nearest `x` logical pixels in.
    pub fn index_at_x(&mut self, text: &str, size: f32, weight: Weight, x: f32) -> usize {
        let s = self.scale;
        self.fonts.index_at_x(text, size * s, weight, x * s)
    }

    /// Draws `text` with its baseline at `y`. Returns the advance width.
    pub fn text(&mut self, x: f32, y: f32, text: &str, size: f32, weight: Weight, color: Color) -> f32 {
        // Destructured so the glyph cache and the pixmap are borrowed disjointly.
        let Painter { pixmap, fonts, clip, scale, .. } = self;
        let scale = *scale;
        let clip = dev(*clip, scale);
        let size = size * scale;
        let (x, y) = (x * scale, y * scale);
        let mut pen = x;
        let (cw, ch) = (pixmap.width() as i32, pixmap.height() as i32);
        for c in text.chars() {
            if c == ' ' {
                pen += fonts.glyph(' ', size, weight).advance;
                continue;
            }
            let g = fonts.glyph(c, size, weight);
            let gx = (pen + g.xmin).round() as i32;
            let gy = (y + g.ytop).round() as i32;
            let (gw, gh, advance) = (g.width as i32, g.height as i32, g.advance);
            let coverage = &g.coverage;
            let data = pixmap.pixels_mut();
            for row in 0..gh {
                let py = gy + row;
                if py < 0 || py >= ch || (py as f32) < clip.y || (py as f32) >= clip.bottom() {
                    continue;
                }
                for col in 0..gw {
                    let px = gx + col;
                    if px < 0 || px >= cw || (px as f32) < clip.x || (px as f32) >= clip.right() {
                        continue;
                    }
                    let a = coverage[(row * gw + col) as usize];
                    if a == 0 {
                        continue;
                    }
                    let alpha = a as u32 * color[3] as u32 / 255;
                    blend_pixel(&mut data[(py * cw + px) as usize], color, alpha as u8);
                }
            }
            pen += advance;
        }
        (pen - x) / scale
    }

    // -------------------------------------------------------- styled text
    //
    // A text *clip*'s own font family and letter spacing, as opposed to the
    // app's own UI chrome (which never varies either and always goes
    // through the plain `text`/`label`/etc above). Kept as a separate,
    // parallel set of methods rather than adding parameters to those —
    // dozens of call sites across the whole app draw plain UI text, and
    // none of them need a family or spacing.

    pub fn text_width_styled(&mut self, text: &str, size: f32, weight: Weight, family: &str, letter_spacing: f32) -> f32 {
        let idx = self.fonts.family_index(family, weight);
        let s = self.scale;
        // `text_styled` below adds `letter_spacing` after every character,
        // trailing one included, so this has to match exactly or centered
        // and right-aligned text would sit slightly off from what actually
        // gets drawn.
        let base = self.fonts.width_at(idx, text, size * s);
        let extra = letter_spacing * s * text.chars().count() as f32;
        (base + extra) / s
    }

    pub fn ascent_styled(&mut self, size: f32, weight: Weight, family: &str) -> f32 {
        let idx = self.fonts.family_index(family, weight);
        self.fonts.ascent_at(idx, size * self.scale) / self.scale
    }

    pub fn line_height_styled(&mut self, size: f32, weight: Weight, family: &str) -> f32 {
        let idx = self.fonts.family_index(family, weight);
        self.fonts.line_height_at(idx, size * self.scale) / self.scale
    }

    /// Same as `text`, but through a text clip's own chosen family and with
    /// extra space added after each character's normal advance.
    #[allow(clippy::too_many_arguments)]
    pub fn text_styled(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        size: f32,
        weight: Weight,
        family: &str,
        letter_spacing: f32,
        color: Color,
    ) -> f32 {
        let idx = self.fonts.family_index(family, weight);
        let Painter { pixmap, fonts, clip, scale, .. } = self;
        let scale = *scale;
        let clip = dev(*clip, scale);
        let size = size * scale;
        let spacing = letter_spacing * scale;
        let (x, y) = (x * scale, y * scale);
        let mut pen = x;
        let (cw, ch) = (pixmap.width() as i32, pixmap.height() as i32);
        for c in text.chars() {
            if c == ' ' {
                pen += fonts.glyph_at(idx, ' ', size).advance + spacing;
                continue;
            }
            let g = fonts.glyph_at(idx, c, size);
            let gx = (pen + g.xmin).round() as i32;
            let gy = (y + g.ytop).round() as i32;
            let (gw, gh, advance) = (g.width as i32, g.height as i32, g.advance);
            let coverage = &g.coverage;
            let data = pixmap.pixels_mut();
            for row in 0..gh {
                let py = gy + row;
                if py < 0 || py >= ch || (py as f32) < clip.y || (py as f32) >= clip.bottom() {
                    continue;
                }
                for col in 0..gw {
                    let px = gx + col;
                    if px < 0 || px >= cw || (px as f32) < clip.x || (px as f32) >= clip.right() {
                        continue;
                    }
                    let a = coverage[(row * gw + col) as usize];
                    if a == 0 {
                        continue;
                    }
                    let alpha = a as u32 * color[3] as u32 / 255;
                    blend_pixel(&mut data[(py * cw + px) as usize], color, alpha as u8);
                }
            }
            pen += advance + spacing;
        }
        (pen - x) / scale
    }

    /// Draws text inside `r`, vertically centred and horizontally aligned,
    /// truncating with an ellipsis when it does not fit.
    pub fn label(&mut self, r: Rect, text: &str, size: f32, weight: Weight, color: Color, align: Align) -> f32 {
        let shown = self.ellipsize(text, size, weight, r.w);
        let w = self.text_width(&shown, size, weight);
        let x = match align {
            Align::Left => r.x,
            Align::Center => r.x + (r.w - w) / 2.0,
            Align::Right => r.right() - w,
        };
        let ascent = self.ascent(size, weight);
        let line = self.line_height(size, weight);
        let baseline = r.y + (r.h - line) / 2.0 + ascent;
        self.text(x, baseline, &shown, size, weight, color);
        w
    }

    // ------------------------------------------------------------------ image

    /// Straight scaled blit, used for thumbnails and preview stills.
    pub fn image(&mut self, rgba: &[u8], sw: u32, sh: u32, dst: Rect, alpha: f32) {
        self.blit(rgba, sw, sh, dst, &Blit { alpha, ..Default::default() });
    }

    /// The one image path everything goes through. `src` selects the part of the
    /// source that fills `dst` (that is how crop works), `reveal` limits which
    /// part of `dst` is painted (that is how wipes work), `rotation` spins about
    /// the destination centre, and `mask` multiplies an 8-bit coverage plane
    /// sampled in the destination's own space.
    pub fn blit(&mut self, rgba: &[u8], sw: u32, sh: u32, dst: Rect, opts: &Blit) {
        if dst.is_empty() || sw == 0 || sh == 0 || opts.alpha <= 0.002 {
            return;
        }
        let dst = dev(dst, self.scale);
        let clip = dev(self.clip, self.scale);

        // The overwhelmingly common case: an upright layer with no mask. It gets
        // a dedicated scanline loop, which is what keeps the preview cheap.
        if opts.rotation.abs() < 1e-4 && opts.mask.is_none() {
            self.blit_upright(rgba, sw, sh, dst, clip, opts);
            return;
        }
        // Rotation grows the affected area, so widen the scan box to cover it.
        let scan = if opts.rotation.abs() > 1e-4 {
            let half = dst.w.hypot(dst.h) / 2.0;
            Rect::new(dst.cx() - half, dst.cy() - half, half * 2.0, half * 2.0)
        } else {
            dst
        }
        .intersect(&clip);
        if scan.is_empty() {
            return;
        }

        // Inverse rotation: destination pixel -> clip-local space.
        let (sin, cos) = opts.rotation.sin_cos();
        let (cx, cy) = (dst.cx(), dst.cy());
        let (su0, sv0, su1, sv1) = opts.src;
        let (rl, rt, rr, rb) = opts.reveal;

        let cw = self.pixmap.width() as i32;
        let x0 = scan.x.floor().max(0.0) as i32;
        let x1 = scan.right().ceil().min(self.pixmap.width() as f32) as i32;
        let y0 = scan.y.floor().max(0.0) as i32;
        let y1 = scan.bottom().ceil().min(self.pixmap.height() as f32) as i32;
        let global_a = opts.alpha.clamp(0.0, 1.0);
        let (mw, mh) = opts.mask_size;

        let data = self.pixmap.pixels_mut();
        for py in y0..y1 {
            for px in x0..x1 {
                let dx = px as f32 + 0.5 - cx;
                let dy = py as f32 + 0.5 - cy;
                let (lx, ly) = (dx * cos + dy * sin, -dx * sin + dy * cos);
                let u = (lx + dst.w / 2.0) / dst.w;
                let v = (ly + dst.h / 2.0) / dst.h;
                if u < rl || u >= rr || v < rt || v >= rb || !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let sx = ((su0 + u * (su1 - su0)) * sw as f32).clamp(0.0, sw as f32 - 1.0);
                let sy = ((sv0 + v * (sv1 - sv0)) * sh as f32).clamp(0.0, sh as f32 - 1.0);
                let src = sample_bilinear(rgba, sw, sh, sx, sy);
                let mut a = src[3] as f32 * global_a;
                if let Some(cov) = opts.mask {
                    let mx = (u * mw as f32) as u32;
                    let my = (v * mh as f32) as u32;
                    let i = (my.min(mh - 1) * mw + mx.min(mw - 1)) as usize;
                    a *= cov[i] as f32 / 255.0;
                }
                if a < 0.5 {
                    continue;
                }
                blend_pixel(&mut data[(py * cw + px) as usize], src, a as u8);
            }
        }
    }
}

impl Painter {
    /// Axis-aligned bilinear blit. Source coordinates are precomputed per
    /// column and interpolation runs in 8-bit fixed point; a fully opaque layer
    /// skips blending altogether and writes straight through.
    fn blit_upright(&mut self, rgba: &[u8], sw: u32, sh: u32, dst: Rect, clip: Rect, opts: &Blit) {
        let (su0, sv0, su1, sv1) = opts.src;
        let (rl, rt, rr, rb) = opts.reveal;
        // Turn the reveal window into device bounds once, instead of testing
        // every pixel against it.
        let area = Rect::new(
            dst.x + rl * dst.w,
            dst.y + rt * dst.h,
            (rr - rl) * dst.w,
            (rb - rt) * dst.h,
        )
        .intersect(&clip);
        if area.is_empty() {
            return;
        }

        let pw = self.pixmap.width();
        let x0 = area.x.floor().max(0.0) as u32;
        let x1 = (area.right().ceil().min(pw as f32)).max(0.0) as u32;
        let y0 = area.y.floor().max(0.0) as u32;
        let y1 = (area.bottom().ceil().min(self.pixmap.height() as f32)).max(0.0) as u32;
        if x1 <= x0 || y1 <= y0 {
            return;
        }

        let sx_span = (su1 - su0) * sw as f32;
        let sy_span = (sv1 - sv0) * sh as f32;
        let global_a = (opts.alpha.clamp(0.0, 1.0) * 255.0) as u32;
        if global_a == 0 {
            return;
        }

        self.blit_cols.clear();
        self.blit_cols.reserve((x1 - x0) as usize);
        for x in x0..x1 {
            let u = (x as f32 + 0.5 - dst.x) / dst.w;
            let sx = (su0 * sw as f32 + u * sx_span).clamp(0.0, sw as f32 - 1.0);
            let xi = sx as u32;
            let frac = ((sx - xi as f32) * 256.0) as u16;
            self.blit_cols.push((xi, (xi + 1).min(sw - 1), frac));
        }

        let cols = std::mem::take(&mut self.blit_cols);
        let data = self.pixmap.pixels_mut();
        for y in y0..y1 {
            let v = (y as f32 + 0.5 - dst.y) / dst.h;
            let sy = (sv0 * sh as f32 + v * sy_span).clamp(0.0, sh as f32 - 1.0);
            let yi = sy as u32;
            let fy = ((sy - yi as f32) * 256.0) as u32;
            let row0 = (yi * sw) as usize * 4;
            let row1 = (yi + 1).min(sh - 1) as usize * sw as usize * 4;
            let out = (y * pw) as usize;

            for (i, &(xa, xb, fx)) in cols.iter().enumerate() {
                let fx = fx as u32;
                let (xa, xb) = (xa as usize * 4, xb as usize * 4);
                let mut px = [0u8; 4];
                for c in 0..4 {
                    let top = lerp8(rgba[row0 + xa + c], rgba[row0 + xb + c], fx);
                    let bot = lerp8(rgba[row1 + xa + c], rgba[row1 + xb + c], fx);
                    px[c] = ((top * (256 - fy) + bot * fy) >> 8) as u8;
                }
                let a = px[3] as u32 * global_a / 255;
                if a == 0 {
                    continue;
                }
                let slot = &mut data[out + x0 as usize + i];
                if a >= 255 {
                    if let Some(p) = tiny_skia::PremultipliedColorU8::from_rgba(px[0], px[1], px[2], 255) {
                        *slot = p;
                    }
                } else {
                    blend_pixel(slot, px, a as u8);
                }
            }
        }
        self.blit_cols = cols;
    }
}

#[inline]
fn lerp8(a: u8, b: u8, t: u32) -> u32 {
    (a as u32 * (256 - t) + b as u32 * t) >> 8
}

/// Options for [`Painter::blit`].
pub struct Blit<'a> {
    pub alpha: f32,
    pub rotation: f32,
    /// Sub-rect of the source that fills the destination, as `(u0, v0, u1, v1)`.
    pub src: (f32, f32, f32, f32),
    /// Sub-rect of the destination that is actually painted.
    pub reveal: (f32, f32, f32, f32),
    pub mask: Option<&'a [u8]>,
    pub mask_size: (u32, u32),
}

impl Default for Blit<'_> {
    fn default() -> Self {
        Blit {
            alpha: 1.0,
            rotation: 0.0,
            src: (0.0, 0.0, 1.0, 1.0),
            reveal: (0.0, 0.0, 1.0, 1.0),
            mask: None,
            mask_size: (1, 1),
        }
    }
}

fn sample_bilinear(rgba: &[u8], sw: u32, sh: u32, x: f32, y: f32) -> Color {
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(sw - 1);
    let y1 = (y0 + 1).min(sh - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let at = |px: u32, py: u32| {
        let i = ((py * sw + px) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    };
    let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    let lerp = |p: u8, q: u8, t: f32| p as f32 + (q as f32 - p as f32) * t;
    let mut out = [0u8; 4];
    for i in 0..4 {
        let top = lerp(a[i], b[i], fx);
        let bot = lerp(c[i], d[i], fx);
        out[i] = (top + (bot - top) * fy) as u8;
    }
    out
}

/// Source-over onto a premultiplied destination pixel.
#[inline]
fn blend_pixel(dst: &mut tiny_skia::PremultipliedColorU8, color: Color, alpha: u8) {
    let a = alpha as u32;
    let inv = 255 - a;
    let (dr, dg, db, da) = (dst.red() as u32, dst.green() as u32, dst.blue() as u32, dst.alpha() as u32);
    let sr = color[0] as u32 * a / 255;
    let sg = color[1] as u32 * a / 255;
    let sb = color[2] as u32 * a / 255;
    let out = tiny_skia::PremultipliedColorU8::from_rgba(
        (sr + dr * inv / 255).min(255) as u8,
        (sg + dg * inv / 255).min(255) as u8,
        (sb + db * inv / 255).min(255) as u8,
        (a + da * inv / 255).min(255) as u8,
    );
    if let Some(p) = out {
        *dst = p;
    }
}

fn sk_color(c: Color) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c[0], c[1], c[2], c[3])
}

pub fn round_rect_path(r: Rect, radius: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let rad = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    if rad <= 0.01 {
        pb.push_rect(SkRect::from_xywh(r.x, r.y, r.w, r.h)?);
        return pb.finish();
    }
    // Circular arcs approximated with cubics; the classic 0.5523 magic number.
    let k = rad * 0.5523;
    let (l, t, rr, b) = (r.x, r.y, r.right(), r.bottom());
    pb.move_to(l + rad, t);
    pb.line_to(rr - rad, t);
    pb.cubic_to(rr - rad + k, t, rr, t + rad - k, rr, t + rad);
    pb.line_to(rr, b - rad);
    pb.cubic_to(rr, b - rad + k, rr - rad + k, b, rr - rad, b);
    pb.line_to(l + rad, b);
    pb.cubic_to(l + rad - k, b, l, b - rad + k, l, b - rad);
    pb.line_to(l, t + rad);
    pb.cubic_to(l, t + rad - k, l + rad - k, t, l + rad, t);
    pb.close();
    pb.finish()
}
