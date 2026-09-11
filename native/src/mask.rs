//! Mask rasterization. One routine drives both the live preview (as a
//! compositing stencil) and the exporter (written out as a raw gray plane for
//! ffmpeg's `alphamerge`), so what you see is exactly what renders.

use crate::model::{Mask, MaskShape};
use tiny_skia::{FillRule, PathBuilder, Transform};

/// An 8-bit coverage plane: 255 keeps the pixel, 0 drops it.
#[derive(Clone)]
pub struct MaskPlane {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Rasterizes `mask` into a `w` x `h` coverage plane whose origin is the
/// frame's top-left corner.
pub fn render(mask: &Mask, w: u32, h: u32) -> MaskPlane {
    let w = w.max(1);
    let h = h.max(1);
    let mut data = vec![0u8; (w * h) as usize];

    let cx = mask.x * w as f32;
    let cy = mask.y * h as f32;
    let mw = (mask.width * w as f32).max(1.0);
    let mh = (mask.height * h as f32).max(1.0);
    let feather = mask.feather.max(0.0) * w.min(h) as f32;
    let rot = mask.rotation.to_radians();

    match mask.shape {
        MaskShape::Linear | MaskShape::Mirror => {
            // Both are half-plane/band gradients, cheaper and crisper computed
            // per pixel than rasterized and blurred.
            let (sin, cos) = rot.sin_cos();
            let f = feather.max(0.001);
            let band = mh / 2.0;
            let mirror = mask.shape == MaskShape::Mirror;
            for py in 0..h {
                for px in 0..w {
                    let dx = px as f32 + 0.5 - cx;
                    let dy = py as f32 + 0.5 - cy;
                    let ly = -dx * sin + dy * cos;
                    let a = if mirror {
                        let d = ly.abs() - band;
                        if d <= 0.0 {
                            1.0
                        } else if d >= f {
                            0.0
                        } else {
                            1.0 - d / f
                        }
                    } else if ly <= -f / 2.0 {
                        1.0
                    } else if ly >= f / 2.0 {
                        0.0
                    } else {
                        0.5 - ly / f
                    };
                    data[(py * w + px) as usize] = (a * 255.0 + 0.5) as u8;
                }
            }
        }
        MaskShape::None => data.fill(255),
        MaskShape::Clock => {
            // A pie wedge sweeping clockwise from 12 o'clock — a per-pixel
            // angle test, not a path scaled by size, so it needs its own loop
            // rather than the generic path-fill branch below. `mask.width`
            // doubles as the 0..1 sweep fraction here (ClockWipe sets it
            // directly, unscaled by `WIPE_MAX`, unlike the grow-from-center
            // shapes).
            let sweep = mask.width.clamp(0.0, 1.0) * std::f32::consts::TAU;
            let start = -std::f32::consts::FRAC_PI_2 + rot;
            let f = feather.max(0.001);
            for py in 0..h {
                for px in 0..w {
                    let dx = px as f32 + 0.5 - cx;
                    let dy = py as f32 + 0.5 - cy;
                    let a = (dy.atan2(dx) - start).rem_euclid(std::f32::consts::TAU);
                    let d = a - sweep;
                    let cov = if d <= -f { 1.0 } else if d >= 0.0 { 0.0 } else { -d / f };
                    data[(py * w + px) as usize] = (cov * 255.0 + 0.5) as u8;
                }
            }
        }
        shape => {
            let mut pb = PathBuilder::new();
            match shape {
                MaskShape::Circle => {
                    pb.push_oval(tiny_skia::Rect::from_xywh(-mw / 2.0, -mh / 2.0, mw, mh).unwrap());
                }
                MaskShape::Rectangle => {
                    pb.push_rect(tiny_skia::Rect::from_xywh(-mw / 2.0, -mh / 2.0, mw, mh).unwrap());
                }
                MaskShape::Heart => heart_path(&mut pb, mw, mh),
                MaskShape::Star => star_path(&mut pb, mw, mh),
                _ => {}
            }
            if let Some(path) = pb.finish() {
                let mut m = tiny_skia::Mask::new(w, h).expect("mask allocation");
                let tf = Transform::from_translate(cx, cy).pre_rotate(mask.rotation);
                m.fill_path(&path, FillRule::Winding, true, tf);
                data.copy_from_slice(m.data());
            }
            if feather > 0.5 {
                blur(&mut data, w, h, feather * 0.5);
            }
        }
    }

    if mask.invert {
        for v in &mut data {
            *v = 255 - *v;
        }
    }

    MaskPlane { width: w, height: h, data }
}

fn heart_path(pb: &mut PathBuilder, w: f32, h: f32) {
    let hw = w / 2.0;
    pb.move_to(0.0, h * 0.42);
    pb.cubic_to(-hw * 0.55, h * 0.12, -hw, -h * 0.08, -hw, -h * 0.2);
    pb.cubic_to(-hw, -h * 0.48, -hw * 0.3, -h * 0.52, 0.0, -h * 0.26);
    pb.cubic_to(hw * 0.3, -h * 0.52, hw, -h * 0.48, hw, -h * 0.2);
    pb.cubic_to(hw, -h * 0.08, hw * 0.55, h * 0.12, 0.0, h * 0.42);
    pb.close();
}

fn star_path(pb: &mut PathBuilder, w: f32, h: f32) {
    let (rx, ry) = (w / 2.0, h / 2.0);
    const INNER: f32 = 0.42;
    for i in 0..10 {
        let k = if i % 2 == 0 { 1.0 } else { INNER };
        let a = -std::f32::consts::FRAC_PI_2 + (i as f32 * std::f32::consts::PI) / 5.0;
        let (px, py) = (a.cos() * rx * k, a.sin() * ry * k);
        if i == 0 {
            pb.move_to(px, py);
        } else {
            pb.line_to(px, py);
        }
    }
    pb.close();
}

/// Three separable box passes, which is close enough to a Gaussian for a soft
/// edge and stays O(n) in the radius.
pub fn blur(data: &mut [u8], w: u32, h: u32, sigma: f32) {
    let radius = ((sigma * 1.5).round() as i32).clamp(1, 128);
    let mut tmp = vec![0u8; data.len()];
    for _ in 0..3 {
        box_pass(data, &mut tmp, w as i32, h as i32, radius, true);
        box_pass(&tmp, data, w as i32, h as i32, radius, false);
    }
}

fn box_pass(src: &[u8], dst: &mut [u8], w: i32, h: i32, radius: i32, horizontal: bool) {
    let (outer, inner, step, line) = if horizontal { (h, w, 1, w) } else { (w, h, w, 1) };
    let window = (radius * 2 + 1) as u32;
    for o in 0..outer {
        let base = if horizontal { o * w } else { o };
        let at = |i: i32| src[(base + i.clamp(0, inner - 1) * step) as usize] as u32;
        let mut sum: u32 = 0;
        for i in -radius..=radius {
            sum += at(i);
        }
        for i in 0..inner {
            dst[(base + i * step) as usize] = (sum / window) as u8;
            sum = sum + at(i + radius + 1) - at(i - radius);
        }
    }
    let _ = line;
}

/// Rebases a frame-space mask into a clip's own box, which is the space the
/// exporter's stencil and the preview's per-clip stencil both use.
pub fn localize(mask: &Mask, clip_x: f32, clip_y: f32, clip_w: f32, clip_h: f32, frame_min: f32, box_min: f32) -> Mask {
    let left = clip_x - clip_w / 2.0;
    let top = clip_y - clip_h / 2.0;
    Mask {
        x: (mask.x - left) / clip_w.max(1e-4),
        y: (mask.y - top) / clip_h.max(1e-4),
        width: mask.width / clip_w.max(1e-4),
        height: mask.height / clip_h.max(1e-4),
        feather: mask.feather * frame_min / box_min.max(1.0),
        ..*mask
    }
}
