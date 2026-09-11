//! Text rasterization.
//!
//! Fonts are memory-mapped from the system font directory and parsed lazily:
//! only the glyphs actually drawn are ever outlined, and the outline data stays
//! in the file-backed mapping. Parsing whole faces up front is by far the most
//! expensive thing a text stack can do — it cost this app more memory than every
//! pixel buffer put together — so nothing here touches a glyph until it is asked
//! for by name and size.

use ab_glyph::{Font as _, FontRef, PxScale, ScaleFont};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Weight {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl Weight {
    pub fn of(bold: bool, italic: bool) -> Weight {
        match (bold, italic) {
            (true, true) => Weight::BoldItalic,
            (true, false) => Weight::Bold,
            (false, true) => Weight::Italic,
            (false, false) => Weight::Regular,
        }
    }
}

pub struct Glyph {
    pub width: usize,
    pub height: usize,
    /// Horizontal offset from the pen position to the bitmap's left edge.
    pub xmin: f32,
    /// Vertical offset from the baseline to the bitmap's top edge (y grows down).
    pub ytop: f32,
    pub advance: f32,
    pub coverage: Vec<u8>,
}

struct Face {
    font: FontRef<'static>,
}

/// Face index used for the emoji/symbol fallback, kept clear of the four real
/// weights so it can share the same glyph cache.
const FALLBACK_IDX: u8 = 200;

pub struct Fonts {
    faces: Vec<Face>,
    /// Segoe UI Emoji (or the symbol face), consulted for any character the
    /// text faces have no glyph for. Without it a pasted or IME-inserted emoji
    /// renders as a blank box.
    fallback: Option<Face>,
    cache: HashMap<(u8, char, u32), Glyph>,
    /// Advance-width cache for whole strings, which dominates layout cost.
    widths: HashMap<(u8, u32, u64), f32>,
}

/// Leaks the mapping on purpose: it is file-backed, so it costs no private
/// memory, and it must outlive the parser that borrows it. Each file is mapped
/// at most once for the life of the process — detaching a pane builds a second
/// `Fonts`, and re-mapping every face for each one would leak a handle per
/// window opened.
fn map_font(name: &str) -> Option<&'static [u8]> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static MAPPED: OnceLock<Mutex<HashMap<String, Option<&'static [u8]>>>> = OnceLock::new();
    let cache = MAPPED.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().ok()?;
    if let Some(hit) = cache.get(name) {
        return *hit;
    }
    let mapped = (|| {
        let file = std::fs::File::open(format!("C:/Windows/Fonts/{}", name)).ok()?;
        let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
        let bytes: &'static [u8] = Box::leak(Box::new(map));
        Some(bytes)
    })();
    cache.insert(name.to_string(), mapped);
    mapped
}

fn load_face(candidates: &[&str]) -> Option<Face> {
    for name in candidates {
        let Some(bytes) = map_font(name) else { continue };
        if let Ok(font) = FontRef::try_from_slice(bytes) {
            return Some(Face { font });
        }
    }
    None
}

impl Fonts {
    pub fn load() -> Fonts {
        // Four faces, in Weight order. Mapping them costs nothing measurable now
        // that outlines stay in the file, so italics render in the preview
        // exactly as they will in the export.
        let wanted: [&[&str]; 4] = [
            &["segoeui.ttf", "arial.ttf", "tahoma.ttf"],
            &["segoeuib.ttf", "arialbd.ttf", "tahomabd.ttf"],
            &["segoeuii.ttf", "ariali.ttf"],
            &["segoeuiz.ttf", "arialbi.ttf"],
        ];
        let mut faces: Vec<Option<Face>> = wanted.iter().map(|c| load_face(c)).collect();
        if faces[0].is_none() {
            panic!("no usable system font found in C:/Windows/Fonts");
        }
        // Any face we could not find falls back to the regular one.
        for i in 1..faces.len() {
            if faces[i].is_none() {
                faces[i] = load_face(wanted[0]);
            }
        }
        let faces = faces.into_iter().flatten().collect();
        // Segoe UI Emoji carries plain outlines alongside its colour layers, so
        // ab_glyph renders it as monochrome silhouettes — not colourful, but
        // legible, which is what matters for a caption being typed.
        let fallback = load_face(&["seguiemj.ttf", "seguisym.ttf", "seguisym.ttf"]);
        Fonts { faces, fallback, cache: HashMap::new(), widths: HashMap::new() }
    }

    fn face_index(&self, weight: Weight) -> u8 {
        let i = weight as u8;
        if (i as usize) < self.faces.len() {
            i
        } else {
            0
        }
    }

    fn face(&self, idx: u8) -> &FontRef<'static> {
        match self.fallback.as_ref() {
            Some(f) if idx == FALLBACK_IDX => &f.font,
            _ => &self.faces[(idx as usize).min(self.faces.len() - 1)].font,
        }
    }

    /// Which face actually owns this character: the requested weight, or the
    /// emoji/symbol fallback when the weight has nothing to draw.
    fn resolve(&self, ch: char, weight: Weight) -> u8 {
        let idx = self.face_index(weight);
        if self.faces[idx as usize].font.glyph_id(ch).0 != 0 {
            return idx;
        }
        match &self.fallback {
            Some(f) if f.font.glyph_id(ch).0 != 0 => FALLBACK_IDX,
            _ => idx,
        }
    }

    pub fn glyph(&mut self, ch: char, size: f32, weight: Weight) -> &Glyph {
        let idx = self.resolve(ch, weight);
        // Rounded to the nearest quarter pixel: an on-canvas resize handle or a
        // size field being dragged reports a new float on essentially every
        // mouse-move, which otherwise means every glyph in the string gets
        // outlined and rasterized fresh on every single frame of the drag
        // instead of reusing what was just drawn a pixel ago.
        let size = (size * 4.0).round() / 4.0;
        let key = (idx, ch, size.to_bits());
        if !self.cache.contains_key(&key) {
            let glyph = rasterize(self.face(idx), ch, size);
            if self.cache.len() > 3000 {
                self.cache.clear();
            }
            self.cache.insert(key, glyph);
        }
        &self.cache[&key]
    }

    pub fn width(&mut self, text: &str, size: f32, weight: Weight) -> f32 {
        let idx = self.face_index(weight);
        let size = (size * 4.0).round() / 4.0;
        let key = (idx, size.to_bits(), hash_str(text));
        if let Some(w) = self.widths.get(&key) {
            return *w;
        }
        let mut total = 0.0;
        for ch in text.chars() {
            total += self.glyph(ch, size, weight).advance;
        }
        if self.widths.len() > 4096 {
            self.widths.clear();
        }
        self.widths.insert(key, total);
        total
    }

    /// Distance from the top of a line box down to the baseline.
    pub fn ascent(&self, size: f32, weight: Weight) -> f32 {
        let idx = self.face_index(weight) as usize;
        self.faces[idx].font.as_scaled(PxScale::from(size)).ascent()
    }

    pub fn line_height(&self, size: f32, weight: Weight) -> f32 {
        let idx = self.face_index(weight) as usize;
        let f = self.faces[idx].font.as_scaled(PxScale::from(size));
        f.ascent() - f.descent() + f.line_gap()
    }

    /// Truncates to fit `max_width`, appending an ellipsis when it has to cut.
    pub fn ellipsize(&mut self, text: &str, size: f32, weight: Weight, max_width: f32) -> String {
        if self.width(text, size, weight) <= max_width {
            return text.to_string();
        }
        let dots = self.width("...", size, weight);
        let mut out = String::new();
        let mut used = 0.0;
        for ch in text.chars() {
            let w = self.glyph(ch, size, weight).advance;
            if used + w + dots > max_width {
                break;
            }
            used += w;
            out.push(ch);
        }
        out.push_str("...");
        out
    }

    /// Byte index of the character boundary nearest `x` pixels into `text`.
    pub fn index_at_x(&mut self, text: &str, size: f32, weight: Weight, x: f32) -> usize {
        let mut used = 0.0;
        for (i, ch) in text.char_indices() {
            let w = self.glyph(ch, size, weight).advance;
            if x < used + w / 2.0 {
                return i;
            }
            used += w;
        }
        text.len()
    }
}

fn rasterize(font: &FontRef<'static>, ch: char, size: f32) -> Glyph {
    let scaled = font.as_scaled(PxScale::from(size));
    let id = font.glyph_id(ch);
    let advance = scaled.h_advance(id);
    let positioned = id.with_scale(PxScale::from(size));

    let Some(outlined) = font.outline_glyph(positioned) else {
        return Glyph { width: 0, height: 0, xmin: 0.0, ytop: 0.0, advance, coverage: Vec::new() };
    };
    let bounds = outlined.px_bounds();
    let width = bounds.width().ceil().max(0.0) as usize;
    let height = bounds.height().ceil().max(0.0) as usize;
    let mut coverage = vec![0u8; width * height];
    outlined.draw(|x, y, c| {
        let (x, y) = (x as usize, y as usize);
        if x < width && y < height {
            coverage[y * width + x] = (c * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    });
    Glyph {
        width,
        height,
        xmin: bounds.min.x,
        ytop: bounds.min.y,
        advance,
        coverage,
    }
}

fn hash_str(s: &str) -> u64 {
    // FNV-1a: short strings, called constantly, no allocation.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
