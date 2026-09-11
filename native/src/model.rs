//! Project data model. Mirrors the on-disk JSON, which is versioned only by
//! serde defaults: every field added later carries a `#[serde(default)]` so
//! older project files keep loading.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

pub type Id = u64;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Monotonic ids are enough here: they only need to be unique inside one
/// project, and projects are never merged.
pub fn next_id() -> Id {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Keeps `next_id` ahead of every id a freshly loaded project already uses.
pub fn bump_id_floor(seen: Id) {
    NEXT_ID.fetch_max(seen + 1, Ordering::Relaxed);
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Video,
    Image,
    Audio,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    Video,
    Image,
    Audio,
    Text,
}

impl ClipKind {
    pub fn label(self) -> &'static str {
        match self {
            ClipKind::Video => "Video",
            ClipKind::Image => "Image",
            ClipKind::Audio => "Audio",
            ClipKind::Text => "Text",
        }
    }

    pub fn ff_name(self) -> &'static str {
        match self {
            ClipKind::Video => "video",
            ClipKind::Image => "image",
            ClipKind::Audio => "audio",
            ClipKind::Text => "text",
        }
    }
}

impl From<AssetKind> for ClipKind {
    fn from(k: AssetKind) -> Self {
        match k {
            AssetKind::Video => ClipKind::Video,
            AssetKind::Image => ClipKind::Image,
            AssetKind::Audio => ClipKind::Audio,
        }
    }
}

/// Tracks lock to a family rather than an exact kind: video and photos are both
/// "visual" and share a track, while audio and text keep their own.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum TrackFamily {
    Visual,
    Audio,
    Text,
}

pub fn track_family(kind: ClipKind) -> TrackFamily {
    match kind {
        ClipKind::Audio => TrackFamily::Audio,
        ClipKind::Text => TrackFamily::Text,
        _ => TrackFamily::Visual,
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MediaAsset {
    pub id: Id,
    pub path: String,
    pub name: String,
    pub kind: AssetKind,
    pub duration: f32,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
    /// Normalized 0..1 peaks for the timeline waveform; empty until computed.
    #[serde(default)]
    pub waveform: Vec<f32>,
}

/// Rectangular source crop, as fractions of the source frame trimmed off each edge.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Crop {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Crop {
    pub fn is_active(&self) -> bool {
        self.left > 1e-4 || self.top > 1e-4 || self.right > 1e-4 || self.bottom > 1e-4
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum MaskShape {
    None,
    Linear,
    Mirror,
    Circle,
    Rectangle,
    Heart,
    Star,
    /// A pie-slice sweeping around the center like a clock hand — used only by
    /// the `ClockWipe` transition, not offered in the manual mask picker.
    Clock,
}

pub const MASK_SHAPES: [(MaskShape, &str); 7] = [
    (MaskShape::None, "None"),
    (MaskShape::Linear, "Linear"),
    (MaskShape::Mirror, "Mirror"),
    (MaskShape::Circle, "Circle"),
    (MaskShape::Rectangle, "Rectangle"),
    (MaskShape::Heart, "Heart"),
    (MaskShape::Star, "Star"),
];

/// Mask geometry lives in normalized frame coordinates, the same space as
/// `clip.x/y/width/height`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Mask {
    pub shape: MaskShape,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Degrees, clockwise.
    pub rotation: f32,
    /// Edge softness as a fraction of the frame's smaller side.
    pub feather: f32,
    pub invert: bool,
}

impl Default for Mask {
    fn default() -> Self {
        Mask {
            shape: MaskShape::None,
            x: 0.5,
            y: 0.5,
            width: 0.5,
            height: 0.5,
            rotation: 0.0,
            feather: 0.0,
            invert: false,
        }
    }
}

impl Mask {
    pub fn is_active(&self) -> bool {
        self.shape != MaskShape::None
    }
}

/// `#[serde(other)]` on `None` means a project referencing a transition kind
/// from a since-removed catalog (an older build's creative set, say) loads as
/// "no transition" instead of failing the whole project.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum TransitionType {
    Fade,
    CrossDissolve,
    DipToBlack,
    DipToWhite,
    WipeLeft,
    WipeRight,
    WipeUp,
    WipeDown,
    PushLeft,
    PushRight,
    PushUp,
    PushDown,
    SlideLeft,
    SlideRight,
    ZoomIn,
    ZoomOut,
    Spin,
    Spin360,
    CubeRotation,
    PageTurn,
    ClockWipe,
    Iris,
    Diamond,
    Circle,
    Heart,
    Glitch,
    Pixelate,
    MotionBlur,
    WhipPan,
    LightFlash,
    #[serde(other)]
    None,
}

pub const TRANSITIONS: [(TransitionType, &str); 30] = [
    (TransitionType::None, "None"),
    (TransitionType::Fade, "Fade"),
    (TransitionType::CrossDissolve, "Cross dissolve"),
    (TransitionType::DipToBlack, "Dip to black"),
    (TransitionType::DipToWhite, "Dip to white"),
    (TransitionType::WipeLeft, "Wipe left"),
    (TransitionType::WipeRight, "Wipe right"),
    (TransitionType::WipeUp, "Wipe up"),
    (TransitionType::WipeDown, "Wipe down"),
    (TransitionType::PushLeft, "Push left"),
    (TransitionType::PushRight, "Push right"),
    (TransitionType::PushUp, "Push up"),
    (TransitionType::PushDown, "Push down"),
    (TransitionType::SlideLeft, "Slide left"),
    (TransitionType::SlideRight, "Slide right"),
    (TransitionType::ZoomIn, "Zoom in"),
    (TransitionType::ZoomOut, "Zoom out"),
    (TransitionType::Spin, "Spin"),
    (TransitionType::Spin360, "360\u{b0} spin"),
    (TransitionType::CubeRotation, "Cube rotation"),
    (TransitionType::PageTurn, "Page turn"),
    (TransitionType::ClockWipe, "Clock wipe"),
    (TransitionType::Iris, "Iris"),
    (TransitionType::Diamond, "Diamond"),
    (TransitionType::Circle, "Circle"),
    (TransitionType::Heart, "Heart"),
    (TransitionType::Glitch, "Glitch"),
    (TransitionType::Pixelate, "Pixelate"),
    (TransitionType::MotionBlur, "Motion blur"),
    (TransitionType::WhipPan, "Whip pan"),
];

impl TransitionType {
    /// The out-transition that mirrors a given in-transition.
    pub fn opposite(self) -> TransitionType {
        use TransitionType::*;
        match self {
            WipeLeft => WipeRight,
            WipeRight => WipeLeft,
            WipeUp => WipeDown,
            WipeDown => WipeUp,
            PushLeft => PushRight,
            PushRight => PushLeft,
            PushUp => PushDown,
            PushDown => PushUp,
            SlideLeft => SlideRight,
            SlideRight => SlideLeft,
            ZoomIn => ZoomOut,
            ZoomOut => ZoomIn,
            DipToBlack => DipToBlack,
            DipToWhite => DipToWhite,
            other => other,
        }
    }

    pub fn ff_name(self) -> &'static str {
        use TransitionType::*;
        match self {
            None => "none",
            Fade => "fade",
            CrossDissolve => "crossDissolve",
            DipToBlack => "dipToBlack",
            DipToWhite => "dipToWhite",
            WipeLeft => "wipeLeft",
            WipeRight => "wipeRight",
            WipeUp => "wipeUp",
            WipeDown => "wipeDown",
            PushLeft => "pushLeft",
            PushRight => "pushRight",
            PushUp => "pushUp",
            PushDown => "pushDown",
            SlideLeft => "slideLeft",
            SlideRight => "slideRight",
            ZoomIn => "zoomIn",
            ZoomOut => "zoomOut",
            Spin => "spin",
            Spin360 => "spin360",
            CubeRotation => "cubeRotation",
            PageTurn => "pageTurn",
            ClockWipe => "clockWipe",
            Iris => "iris",
            Diamond => "diamond",
            Circle => "circle",
            Heart => "heart",
            Glitch => "glitch",
            Pixelate => "pixelate",
            MotionBlur => "motionBlur",
            WhipPan => "whipPan",
            LightFlash => "lightFlash",
        }
    }

    pub fn label(self) -> &'static str {
        TRANSITIONS.iter().find(|(t, _)| *t == self).map(|(_, l)| *l).unwrap_or("None")
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    pub kind: TransitionType,
    pub duration: f32,
}

impl Default for Transition {
    fn default() -> Self {
        Transition { kind: TransitionType::None, duration: 0.5 }
    }
}

impl Transition {
    pub fn is_active(&self) -> bool {
        self.kind != TransitionType::None && self.duration > 0.001
    }
}

/// A starting point for the color-grade sliders. Selecting one just sets the
/// underlying fields (see `ColorGrade::apply_preset`) — rendering never
/// switches on the preset itself, so a since-removed preset in an old save
/// still loads (`#[serde(other)]`) and simply shows as a custom grade.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ColorPreset {
    Vivid,
    Warm,
    Cool,
    Vintage,
    Faded,
    #[serde(other)]
    Custom,
}

pub const COLOR_PRESETS: [(ColorPreset, &str); 6] = [
    (ColorPreset::Custom, "Custom"),
    (ColorPreset::Vivid, "Vivid"),
    (ColorPreset::Warm, "Warm"),
    (ColorPreset::Cool, "Cool"),
    (ColorPreset::Vintage, "Vintage"),
    (ColorPreset::Faded, "Faded"),
];

/// Per-clip color grade: white balance, exposure, tone range, a tone curve,
/// and an optional 3D LUT. Values are unitless -1..1 offsets (0 = no change)
/// except `curve` (control points in 0..1 frame space) and the LUT fields.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ColorGrade {
    #[serde(default)]
    pub preset: ColorPreset,
    #[serde(default)]
    pub temperature: f32,
    #[serde(default)]
    pub tint: f32,
    #[serde(default)]
    pub exposure: f32,
    #[serde(default)]
    pub highlights: f32,
    #[serde(default)]
    pub shadows: f32,
    #[serde(default = "identity_curve")]
    pub curve: Vec<(f32, f32)>,
    /// Absolute path to a `.cube` 3D LUT file, or empty for none.
    #[serde(default)]
    pub lut_path: String,
    #[serde(default = "one")]
    pub lut_strength: f32,
}

impl Default for ColorPreset {
    fn default() -> Self {
        ColorPreset::Custom
    }
}

fn identity_curve() -> Vec<(f32, f32)> {
    vec![(0.0, 0.0), (1.0, 1.0)]
}

impl Default for ColorGrade {
    fn default() -> Self {
        ColorGrade {
            preset: ColorPreset::Custom,
            temperature: 0.0,
            tint: 0.0,
            exposure: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            curve: identity_curve(),
            lut_path: String::new(),
            lut_strength: 1.0,
        }
    }
}

impl ColorGrade {
    pub fn is_active(&self) -> bool {
        self.temperature.abs() > 1e-4
            || self.tint.abs() > 1e-4
            || self.exposure.abs() > 1e-4
            || self.highlights.abs() > 1e-4
            || self.shadows.abs() > 1e-4
            || !self.lut_path.is_empty()
            || self.curve != identity_curve()
    }

    /// Resets every slider and the curve to the preset's own values, but
    /// leaves the LUT alone — a LUT is a separate creative choice layered on
    /// top, not something a look preset should silently clear.
    pub fn apply_preset(&mut self, preset: ColorPreset) {
        let (lut_path, lut_strength) = (std::mem::take(&mut self.lut_path), self.lut_strength);
        *self = ColorGrade { lut_path, lut_strength, preset, ..ColorGrade::default() };
        match preset {
            ColorPreset::Custom => {}
            ColorPreset::Vivid => {
                self.exposure = 0.05;
                self.highlights = -0.15;
                self.shadows = 0.15;
                self.curve = vec![(0.0, 0.0), (0.25, 0.18), (0.75, 0.85), (1.0, 1.0)];
            }
            ColorPreset::Warm => {
                self.temperature = 0.35;
                self.tint = 0.05;
            }
            ColorPreset::Cool => {
                self.temperature = -0.35;
                self.tint = -0.05;
            }
            ColorPreset::Vintage => {
                self.temperature = 0.15;
                self.shadows = 0.2;
                self.highlights = -0.2;
                self.curve = vec![(0.0, 0.06), (1.0, 0.94)];
            }
            ColorPreset::Faded => {
                self.exposure = 0.1;
                self.shadows = 0.25;
                self.highlights = -0.1;
                self.curve = vec![(0.0, 0.08), (1.0, 0.92)];
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// Both clip flavours share one struct. A tagged enum reads nicer in isolation,
/// but every call site wants `start`/`duration`/`x`/`y`/`opacity` regardless of
/// kind and the UI edits them uniformly, so the flat form keeps the code short.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Clip {
    pub id: Id,
    pub kind: ClipKind,
    pub start: f32,
    pub duration: f32,

    // Media
    #[serde(default)]
    pub asset_id: Id,
    #[serde(default)]
    pub trim_in: f32,
    #[serde(default)]
    pub width: f32,
    #[serde(default)]
    pub height: f32,
    #[serde(default = "one")]
    pub volume: f32,
    #[serde(default)]
    pub crop: Crop,
    #[serde(default)]
    pub mask: Mask,
    #[serde(default)]
    pub color_grade: ColorGrade,
    #[serde(default)]
    pub transition_in: Transition,
    #[serde(default)]
    pub transition_out: Transition,
    /// Audio fade lengths in seconds, measured from the clip's own start/end.
    #[serde(default)]
    pub fade_in: f32,
    #[serde(default)]
    pub fade_out: f32,

    // Shared
    pub x: f32,
    pub y: f32,
    #[serde(default = "one")]
    pub opacity: f32,

    // Text
    #[serde(default)]
    pub text: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "default_white")]
    pub color: String,
    #[serde(default = "default_font")]
    pub font_family: String,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default = "default_align")]
    pub align: TextAlign,
    #[serde(default)]
    pub bg_enabled: bool,
    #[serde(default = "default_black")]
    pub bg_color: String,
}

fn one() -> f32 {
    1.0
}
fn default_font_size() -> f32 {
    64.0
}
fn default_white() -> String {
    "#ffffff".into()
}
fn default_black() -> String {
    "#000000".into()
}
fn default_font() -> String {
    "Arial".into()
}
fn default_align() -> TextAlign {
    TextAlign::Center
}

impl Clip {
    pub fn end(&self) -> f32 {
        self.start + self.duration
    }

    pub fn covers(&self, t: f32) -> bool {
        t >= self.start && t < self.end()
    }

    pub fn is_visual(&self) -> bool {
        matches!(self.kind, ClipKind::Video | ClipKind::Image)
    }

    pub fn new_media(kind: ClipKind, asset_id: Id, start: f32, duration: f32, width: f32, height: f32) -> Clip {
        Clip {
            id: next_id(),
            kind,
            start,
            duration,
            asset_id,
            trim_in: 0.0,
            width,
            height,
            volume: 1.0,
            crop: Crop::default(),
            mask: Mask::default(),
            color_grade: ColorGrade::default(),
            transition_in: Transition::default(),
            transition_out: Transition::default(),
            fade_in: 0.0,
            fade_out: 0.0,
            x: 0.5,
            y: 0.5,
            opacity: 1.0,
            text: String::new(),
            font_size: 64.0,
            color: "#ffffff".into(),
            font_family: "Arial".into(),
            bold: false,
            italic: false,
            align: TextAlign::Center,
            bg_enabled: false,
            bg_color: "#000000".into(),
        }
    }

    pub fn new_text(start: f32) -> Clip {
        let mut c = Clip::new_media(ClipKind::Text, 0, start, 3.0, 0.0, 0.0);
        c.text = "Text".into();
        c.bold = true;
        c
    }

    /// Playback gain for this clip at absolute timeline time `t`, including fades.
    pub fn gain_at(&self, t: f32) -> f32 {
        let base = self.volume.clamp(0.0, 1.0);
        if base <= 0.0 {
            return 0.0;
        }
        let mut gain = base;
        let local = t - self.start;
        if self.fade_in > 0.0 && local < self.fade_in {
            gain *= (local / self.fade_in).max(0.0);
        }
        if self.fade_out > 0.0 {
            let remaining = self.duration - local;
            if remaining < self.fade_out {
                gain *= (remaining / self.fade_out).max(0.0);
            }
        }
        gain.clamp(0.0, 1.0)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Track {
    pub id: Id,
    /// `None` until the first clip lands and fixes the track's family.
    pub kind: Option<ClipKind>,
    pub clips: Vec<Clip>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct ProjectSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        ProjectSettings { width: 1920, height: 1080, fps: 30 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Project {
    #[serde(default)]
    pub settings: ProjectSettings,
    #[serde(default)]
    pub assets: Vec<MediaAsset>,
    #[serde(default)]
    pub tracks: Vec<Track>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProjectMeta {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection {
    pub track_id: Id,
    pub clip_id: Id,
}

pub const FONT_FAMILIES: [&str; 5] = ["Arial", "Georgia", "Impact", "Times New Roman", "Courier New"];

pub const RESOLUTION_PRESETS: [(&str, u32, u32); 4] = [
    ("16:9  1920x1080", 1920, 1080),
    ("9:16  1080x1920", 1080, 1920),
    ("1:1  1080x1080", 1080, 1080),
    ("4:5  1080x1350", 1080, 1350),
];

/// Normalized width/height that contain-fit an asset's aspect ratio inside the frame.
pub fn fit_clip_size(aw: u32, ah: u32, pw: u32, ph: u32) -> (f32, f32) {
    if aw == 0 || ah == 0 {
        return (1.0, 1.0);
    }
    let asset = aw as f32 / ah as f32;
    let project = pw as f32 / ph as f32;
    if asset >= project {
        (1.0, project / asset)
    } else {
        (asset / project, 1.0)
    }
}

/// Parses `#rrggbb` into an RGB triple, falling back to white.
pub fn parse_hex(hex: &str) -> [u8; 3] {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return [255, 255, 255];
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(255);
    [byte(0), byte(2), byte(4)]
}
