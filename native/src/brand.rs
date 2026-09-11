//! The application mark.
//!
//! `logo.png` is baked into the executable rather than loaded from disk, so the
//! app is still a single file you can move anywhere. It is the one at the root
//! of the project directory, so replacing that file and rebuilding is all it
//! takes to rebrand.

use crate::ui::{Ctx, Rect};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

const LOGO_PNG: &[u8] = include_bytes!("../../logo.png");

/// The mark's raw decoded pixmap, in tiny-skia's own (premultiplied) format —
/// for callers that want to composite it with `draw_pixmap` rather than blit
/// straight RGBA, e.g. the transitions gallery painting it onto a demo panel.
pub(crate) fn pixmap() -> Option<&'static tiny_skia::Pixmap> {
    static PIXMAP: OnceLock<Option<tiny_skia::Pixmap>> = OnceLock::new();
    PIXMAP.get_or_init(|| tiny_skia::Pixmap::decode_png(LOGO_PNG).ok()).as_ref()
}

/// The mark at full resolution, as straight (non-premultiplied) RGBA — which
/// is what `Painter::image` and winit's icon both want. Decoded once.
fn source() -> Option<&'static (Vec<u8>, u32, u32)> {
    static SOURCE: OnceLock<Option<(Vec<u8>, u32, u32)>> = OnceLock::new();
    SOURCE
        .get_or_init(|| {
            let pixmap = tiny_skia::Pixmap::decode_png(LOGO_PNG).ok()?;
            let (w, h) = (pixmap.width(), pixmap.height());
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for px in pixmap.pixels() {
                let c = px.demultiply();
                rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
            }
            Some((rgba, w, h))
        })
        .as_ref()
}

/// The mark box-filtered down to exactly `size` square device pixels.
///
/// Drawing a 500px source straight into a 28px slot would let the bilinear
/// sampler pick four texels out of every ~320 and alias the feather's edges to
/// bits. Averaging the whole footprint once, and caching it, gives a clean mark
/// and costs the frame nothing.
pub fn scaled(size: u32) -> Option<Arc<Vec<u8>>> {
    static CACHE: OnceLock<Mutex<HashMap<u32, Arc<Vec<u8>>>>> = OnceLock::new();
    let size = size.clamp(1, 512);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().ok()?;
    if let Some(hit) = cache.get(&size) {
        return Some(hit.clone());
    }
    let (src, sw, sh) = source()?;
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        let y0 = (y * sh / size).min(sh - 1);
        let y1 = (((y + 1) * sh).div_ceil(size)).clamp(y0 + 1, *sh);
        for x in 0..size {
            let x0 = (x * sw / size).min(sw - 1);
            let x1 = (((x + 1) * sw).div_ceil(size)).clamp(x0 + 1, *sw);
            // Average in premultiplied space, or transparent pixels drag their
            // (arbitrary) colour into the result and fringe the edges.
            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = ((sy * sw + sx) * 4) as usize;
                    let sa = src[i + 3] as u32;
                    r += src[i] as u32 * sa / 255;
                    g += src[i + 1] as u32 * sa / 255;
                    b += src[i + 2] as u32 * sa / 255;
                    a += sa;
                    n += 1;
                }
            }
            let n = n.max(1);
            let (r, g, b, a) = (r / n, g / n, b / n, a / n);
            let o = ((y * size + x) * 4) as usize;
            // Back to straight alpha for the consumers.
            let un = |v: u32| if a == 0 { 0 } else { (v * 255 / a).min(255) as u8 };
            out[o] = un(r);
            out[o + 1] = un(g);
            out[o + 2] = un(b);
            out[o + 3] = a as u8;
        }
    }
    let out = Arc::new(out);
    cache.insert(size, out.clone());
    Some(out)
}

/// Draws the mark centred in `r`, square and at its natural aspect.
pub fn draw(ctx: &mut Ctx, r: Rect) {
    let side = r.w.min(r.h);
    if side <= 0.5 {
        return;
    }
    let device = (side * ctx.painter.scale()).round().max(1.0) as u32;
    let Some(rgba) = scaled(device) else { return };
    let dst = Rect::new(r.cx() - side / 2.0, r.cy() - side / 2.0, side, side);
    ctx.painter.image(&rgba, device, device, dst, 1.0);
}

/// The mark as a window icon, for the title bar, the taskbar and Alt-Tab.
pub fn window_icon() -> Option<winit::window::Icon> {
    const SIZE: u32 = 64;
    let rgba = scaled(SIZE)?;
    winit::window::Icon::from_rgba(rgba.as_ref().clone(), SIZE, SIZE).ok()
}
