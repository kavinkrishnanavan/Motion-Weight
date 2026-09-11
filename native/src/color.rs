//! Per-pixel color grading: white balance, exposure, tone range, a tone
//! curve, and 3D LUT (`.cube`) sampling. Applied to a clip's decoded frame
//! before it's composited into the preview (see `preview::Player::draw_clip`).
//!
//! Export never touches these RGBA buffers — it hands the whole timeline to
//! ffmpeg as a filter graph — so the same adjustments are re-expressed there
//! as ffmpeg filters (see `ffmpeg::color_filters`) instead of being reused
//! from here. Both read from the same `ColorGrade` fields, so the two stay
//! in sync even though the code that applies them doesn't.

use crate::model::ColorGrade;

/// A parsed `.cube` 3D LUT: `size`^3 RGB triplets in 0..1, indexed
/// `r + g*size + b*size*size` per the `.cube` file format's own ordering
/// (red fastest, then green, then blue).
pub struct Lut3D {
    size: usize,
    data: Vec<[f32; 3]>,
}

impl Lut3D {
    pub fn load(path: &str) -> Option<Lut3D> {
        let text = std::fs::read_to_string(path).ok()?;
        Lut3D::parse(&text)
    }

    fn parse(text: &str) -> Option<Lut3D> {
        let mut size = 0usize;
        let mut data = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("LUT_3D_SIZE") {
                size = rest.trim().parse().ok()?;
                continue;
            }
            // TITLE, DOMAIN_MIN/MAX, LUT_1D_SIZE etc: not a data row, and not
            // one we need to act on — every value we care about is assumed
            // to already be the standard 0..1 domain over `size`^3 rows.
            if line.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false) {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(r), Some(g), Some(b)) = (parts.next(), parts.next(), parts.next()) else { continue };
            let (Ok(r), Ok(g), Ok(b)) = (r.parse::<f32>(), g.parse::<f32>(), b.parse::<f32>()) else { continue };
            data.push([r, g, b]);
        }
        if size < 2 || data.len() != size * size * size {
            return None;
        }
        Some(Lut3D { size, data })
    }

    #[inline]
    fn at(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        let s = self.size;
        self.data[r + g * s + b * s * s]
    }

    /// Trilinear sample; `r`/`g`/`b` in 0..1.
    fn sample(&self, r: f32, g: f32, b: f32) -> [f32; 3] {
        let max = (self.size - 1) as f32;
        let (fr, fg, fb) = (r.clamp(0.0, 1.0) * max, g.clamp(0.0, 1.0) * max, b.clamp(0.0, 1.0) * max);
        let (r0, g0, b0) = (fr.floor() as usize, fg.floor() as usize, fb.floor() as usize);
        let (r1, g1, b1) = ((r0 + 1).min(self.size - 1), (g0 + 1).min(self.size - 1), (b0 + 1).min(self.size - 1));
        let (tr, tg, tb) = (fr - r0 as f32, fg - g0 as f32, fb - b0 as f32);

        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| {
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
        };
        let c00 = lerp(self.at(r0, g0, b0), self.at(r1, g0, b0), tr);
        let c10 = lerp(self.at(r0, g1, b0), self.at(r1, g1, b0), tr);
        let c01 = lerp(self.at(r0, g0, b1), self.at(r1, g0, b1), tr);
        let c11 = lerp(self.at(r0, g1, b1), self.at(r1, g1, b1), tr);
        lerp(lerp(c00, c10, tg), lerp(c01, c11, tg), tb)
    }
}

/// A tone curve baked from the clip's control points into a 256-entry
/// lookup table, so applying it is one array index per channel per pixel.
struct CurveLut([f32; 256]);

impl CurveLut {
    fn build(points: &[(f32, f32)]) -> CurveLut {
        let mut pts: Vec<(f32, f32)> = if points.len() >= 2 { points.to_vec() } else { vec![(0.0, 0.0), (1.0, 1.0)] };
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut table = [0.0f32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            *slot = eval_piecewise(&pts, i as f32 / 255.0).clamp(0.0, 1.0);
        }
        CurveLut(table)
    }

    #[inline]
    fn eval(&self, v: f32) -> f32 {
        self.0[v.clamp(0.0, 255.0) as usize] * 255.0
    }
}

/// Evaluates the piecewise-linear curve through `pts` (sorted by x, at least
/// two points) at `x`. Public so the inspector's curve editor can trace the
/// exact same line it's about to bake into a `CurveLut`.
pub fn eval_curve(pts: &[(f32, f32)], x: f32) -> f32 {
    eval_piecewise(pts, x)
}

fn eval_piecewise(pts: &[(f32, f32)], x: f32) -> f32 {
    let first = pts[0];
    let last = *pts.last().unwrap();
    if x <= first.0 {
        return first.1;
    }
    if x >= last.0 {
        return last.1;
    }
    for w in pts.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if x >= x0 && x <= x1 {
            let t = if (x1 - x0).abs() < 1e-6 { 0.0 } else { (x - x0) / (x1 - x0) };
            return y0 + (y1 - y0) * t;
        }
    }
    x
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Grades `rgba` (straight, non-premultiplied RGBA8) in place. `lut`, when
/// given, is sampled after the scalar adjustments and curve, then blended in
/// at `grade.lut_strength`.
pub fn apply(rgba: &mut [u8], grade: &ColorGrade, lut: Option<&Lut3D>) {
    let exposure_mul = 2f32.powf(grade.exposure * 2.0);
    let temp = grade.temperature * 40.0;
    let tint = grade.tint * 40.0;
    let curve = CurveLut::build(&grade.curve);
    let has_highlights = grade.highlights.abs() > 1e-4;
    let has_shadows = grade.shadows.abs() > 1e-4;
    let lut_strength = lut.map(|_| grade.lut_strength.clamp(0.0, 1.0)).unwrap_or(0.0);

    for px in rgba.chunks_exact_mut(4) {
        let mut r = px[0] as f32;
        let mut g = px[1] as f32;
        let mut b = px[2] as f32;

        // Temperature slides blue against orange (red up, blue down, or the
        // reverse); tint slides magenta against green the same way.
        r += temp - tint * 0.5;
        g += tint;
        b += -temp - tint * 0.5;

        r *= exposure_mul;
        g *= exposure_mul;
        b *= exposure_mul;

        if has_highlights || has_shadows {
            let luma = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0;
            if has_highlights {
                let w = smoothstep(0.45, 1.0, luma);
                let amt = grade.highlights * 90.0 * w;
                r += amt;
                g += amt;
                b += amt;
            }
            if has_shadows {
                let w = 1.0 - smoothstep(0.0, 0.55, luma);
                let amt = grade.shadows * 90.0 * w;
                r += amt;
                g += amt;
                b += amt;
            }
        }

        r = curve.eval(r.clamp(0.0, 255.0));
        g = curve.eval(g.clamp(0.0, 255.0));
        b = curve.eval(b.clamp(0.0, 255.0));

        if lut_strength > 1e-4 {
            if let Some(lut) = lut {
                let s = lut.sample(r / 255.0, g / 255.0, b / 255.0);
                r += (s[0] * 255.0 - r) * lut_strength;
                g += (s[1] * 255.0 - g) * lut_strength;
                b += (s[2] * 255.0 - b) * lut_strength;
            }
        }

        px[0] = r.clamp(0.0, 255.0) as u8;
        px[1] = g.clamp(0.0, 255.0) as u8;
        px[2] = b.clamp(0.0, 255.0) as u8;
    }
}
