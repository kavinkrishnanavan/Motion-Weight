//! Resolved transition state for one instant. Shared by the live preview and by
//! the still-frame exporter, which bakes the state into clip geometry.

use crate::model::{Clip, MaskShape, TransitionType};

/// A shape-wipe mask is rendered at `mask.width == mask.height == WIPE_MAX`
/// (a fraction of the clip's own box) so that even a concave shape (star) or
/// an off-center one (heart) fully covers every corner of the box once the
/// transition completes, instead of leaving slivers uncovered.
pub const WIPE_MAX: f32 = 1.6;

/// Offsets and the reveal rect are fractions of the clip's own width/height.
#[derive(Clone, Copy, Debug)]
pub struct TransitionState {
    pub alpha: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub scale: f32,
    /// Radians, clockwise.
    pub rotate: f32,
    /// Visible sub-rect of the clip, in 0..1 clip-local coordinates.
    pub reveal_l: f32,
    pub reveal_t: f32,
    pub reveal_r: f32,
    pub reveal_b: f32,
    /// Strength (0..1) of a warm/white overlay flash, peaking at the cut.
    pub flash: f32,
    /// Strength (0..1) of a sparkle-sprite overlay.
    pub sparkle: f32,
    /// Strength (0..1) of a soft ghost-copy blur/streak, used by the zoom-y
    /// effects that want to feel fast rather than merely scaled.
    pub blur: f32,
    /// A shape-wipe stencil, when active; `MaskShape::None` means off.
    pub wipe_shape: MaskShape,
    /// Bounding size fed to `mask::render` for the wipe shape, as a fraction
    /// of the clip's own box (0 = a pinpoint, `WIPE_MAX` = fully covers it).
    pub wipe_progress: f32,
    /// Degrees, clockwise — lets a rotated rectangle stand in for a diamond.
    pub wipe_rotation: f32,
    /// Splits the clip into this many strips for a shatter/glitch effect; 0
    /// means off.
    pub slices: u8,
    pub slice_vertical: bool,
    /// 0..1 magnitude of the per-strip displacement.
    pub slice_offset: f32,
    /// Feather (as a fraction of the clip's shorter side) for `wipe_shape`'s
    /// edge — separates a crisp mechanical wipe (`Circle`) from a soft one
    /// (`Iris`) even though both use the same underlying shape.
    pub wipe_feather: f32,
    /// Strength (0..1) of a flat, opaque color wash over the clip — used by
    /// `DipToBlack`/`DipToWhite` for a genuine dip through a solid color, as
    /// opposed to `Fade`'s alpha fade (which just reveals whatever is
    /// underneath, indistinguishable from a black dip on a black canvas).
    pub wash: f32,
    pub wash_rgb: [u8; 3],
    /// Independent horizontal scale multiplier, on top of `scale` — lets
    /// `CubeRotation` squish only the width, unlike the uniform zooms.
    pub scale_x: f32,
    /// 0..1 blockiness for `Pixelate` — 0 is sharp, 1 is the coarsest mosaic.
    pub pixelate: f32,
    /// Strength (0..1) of a directional ghost-copy smear, distinct from
    /// `blur`'s symmetric radial one — used by `MotionBlur`/`WhipPan`.
    pub streak: f32,
    /// Per-ghost-step displacement for `streak`, as a fraction of the clip's
    /// own box.
    pub streak_dx: f32,
    pub streak_dy: f32,
}

impl Default for TransitionState {
    fn default() -> Self {
        TransitionState {
            alpha: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            scale: 1.0,
            rotate: 0.0,
            reveal_l: 0.0,
            reveal_t: 0.0,
            reveal_r: 1.0,
            reveal_b: 1.0,
            flash: 0.0,
            sparkle: 0.0,
            blur: 0.0,
            wipe_shape: MaskShape::None,
            wipe_progress: 0.0,
            wipe_rotation: 0.0,
            slices: 0,
            slice_vertical: true,
            slice_offset: 0.0,
            wipe_feather: 0.03,
            wash: 0.0,
            wash_rgb: [0, 0, 0],
            scale_x: 1.0,
            pixelate: 0.0,
            streak: 0.0,
            streak_dx: 0.0,
            streak_dy: 0.0,
        }
    }
}

/// Cubic ease: 0 and 1 stay put, the middle picks up and sheds speed
/// smoothly. Used to give `Slide` a softer glide than `Push`'s constant-speed
/// motion, and `CrossDissolve` a gentler blend than `Fade`'s linear ramp.
fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// `p` runs 0 (fully transitioned away) to 1 (fully on screen).
pub(crate) fn apply(st: &mut TransitionState, kind: TransitionType, p: f32, is_out: bool) {
    use TransitionType::*;
    let q = 1.0 - p;
    let dir = if is_out { -1.0 } else { 1.0 };
    match kind {
        Fade => st.alpha *= p,
        // Same alpha ramp as `Fade`, but eased and with a touch of softness at
        // the midpoint — the closest a single-clip renderer can get to a true
        // two-clip blend, while still reading as visibly different from
        // `Fade`'s crisp linear cut.
        CrossDissolve => {
            st.alpha *= smoothstep(p);
            st.blur = st.blur.max((1.0 - (p - 0.5).abs() * 2.0).max(0.0) * 0.35);
        }
        // The clip itself stays fully opaque; a solid color washes over it to
        // full coverage and back, which is a real dip through a color rather
        // than `Fade`'s alpha (indistinguishable from a dip on a black stage).
        DipToBlack => st.wash = st.wash.max(q),
        DipToWhite => {
            st.wash = st.wash.max(q);
            st.wash_rgb = [255, 255, 255];
        }
        WipeLeft => {
            if is_out {
                st.reveal_r = st.reveal_r.min(p);
            } else {
                st.reveal_l = st.reveal_l.max(q);
            }
        }
        WipeRight => {
            if is_out {
                st.reveal_l = st.reveal_l.max(q);
            } else {
                st.reveal_r = st.reveal_r.min(p);
            }
        }
        WipeUp => {
            if is_out {
                st.reveal_b = st.reveal_b.min(p);
            } else {
                st.reveal_t = st.reveal_t.max(q);
            }
        }
        WipeDown => {
            if is_out {
                st.reveal_t = st.reveal_t.max(q);
            } else {
                st.reveal_b = st.reveal_b.min(p);
            }
        }
        // Push moves at constant speed edge-to-edge — a rigid, mechanical
        // motion, unlike `Slide`'s eased glide below.
        PushLeft => st.offset_x += dir * q,
        PushRight => st.offset_x -= dir * q,
        PushUp => st.offset_y += dir * q,
        PushDown => st.offset_y -= dir * q,
        SlideLeft => st.offset_x += dir * smoothstep(q),
        SlideRight => st.offset_x -= dir * smoothstep(q),
        // Starts smaller ("zoomed out") and grows to its natural size.
        ZoomIn => {
            st.alpha *= p;
            st.scale *= if is_out { 1.0 + q * 0.4 } else { 0.6 + 0.4 * p };
        }
        // Starts much larger ("zoomed in") and shrinks down, the opposite
        // feel from `ZoomIn`.
        ZoomOut => {
            st.alpha *= p;
            st.scale *= if is_out { 1.0 - q * 0.6 } else { 1.8 - 0.8 * p };
        }
        // A modest quarter-turn — distinct from `Spin360`'s full rotation.
        Spin => {
            st.alpha *= p;
            st.scale *= 0.75 + 0.25 * p;
            st.rotate += dir * q * std::f32::consts::PI * 0.5;
        }
        Spin360 => {
            st.alpha *= p;
            st.scale *= 0.6 + 0.4 * p;
            st.rotate += dir * q * std::f32::consts::PI * 2.0;
        }
        // Squishes the width toward zero and swings toward the rotating
        // edge, faking a card turning edge-on to the camera; a dark wash
        // peaks at the point of maximum squish, standing in for the shaded
        // face of a rotating cube.
        CubeRotation => {
            st.scale_x = (1.0 - q).max(0.02);
            st.offset_x += dir * q;
            st.wash = st.wash.max(q * 0.35);
        }
        // A wipe-style reveal (the "page" sliding off) plus a slight tilt and
        // a shadow that peaks mid-turn, near the fold — different in kind
        // from `CubeRotation`'s symmetric squish.
        PageTurn => {
            if is_out {
                st.reveal_r = st.reveal_r.min(p);
            } else {
                st.reveal_l = st.reveal_l.max(q);
            }
            st.rotate += dir * q * 0.12;
            st.wash = st.wash.max(q * (1.0 - q) * 0.8);
        }
        ClockWipe => {
            st.wipe_shape = MaskShape::Clock;
            st.wipe_progress = st.wipe_progress.max(p);
        }
        // Same growing-circle stencil as `Circle`, but heavily feathered and
        // paired with a faint vignette, for a lens-iris feel rather than a
        // hard mechanical reveal.
        Iris => {
            st.wipe_shape = MaskShape::Circle;
            st.wipe_progress = st.wipe_progress.max(p * WIPE_MAX);
            st.wipe_feather = 0.12;
            st.wash = st.wash.max((1.0 - (p - 0.5).abs() * 2.0).max(0.0) * 0.14);
        }
        Diamond => {
            st.wipe_shape = MaskShape::Rectangle;
            st.wipe_rotation = 45.0;
            st.wipe_progress = st.wipe_progress.max(p * WIPE_MAX);
            st.wipe_feather = 0.015;
        }
        Circle => {
            st.wipe_shape = MaskShape::Circle;
            st.wipe_progress = st.wipe_progress.max(p * WIPE_MAX);
            st.wipe_feather = 0.015;
        }
        Heart => {
            st.wipe_shape = MaskShape::Heart;
            st.wipe_progress = st.wipe_progress.max(p * WIPE_MAX);
        }
        Glitch => {
            st.alpha *= (0.85 + 0.15 * p).min(1.0);
            st.slices = st.slices.max(14);
            st.slice_vertical = false;
            st.slice_offset = st.slice_offset.max(q);
        }
        Pixelate => st.pixelate = st.pixelate.max(q),
        MotionBlur => {
            st.alpha *= p;
            st.streak = st.streak.max(q);
            st.streak_dx = dir * 0.5;
        }
        // A faster, more extreme version of `MotionBlur`: the clip overshoots
        // off-frame instead of settling at the edge, and the streak is longer.
        WhipPan => {
            st.alpha *= (0.7 + 0.3 * p).min(1.0);
            st.offset_x += dir * q * 1.4;
            st.streak = st.streak.max(q);
            st.streak_dx = dir * 1.1;
        }
        LightFlash => {
            st.alpha *= p;
            st.flash = st.flash.max((1.0 - p).powf(1.5));
        }
        None => {}
    }
}

/// Combined in/out transition state for `clip` at absolute timeline time `t`.
pub fn transition_at(clip: &Clip, t: f32) -> TransitionState {
    let mut st = TransitionState::default();
    let tin = clip.transition_in;
    if tin.is_active() {
        let p = ((t - clip.start) / tin.duration).clamp(0.0, 1.0);
        if p < 1.0 {
            apply(&mut st, tin.kind, p, false);
        }
    }
    let tout = clip.transition_out;
    if tout.is_active() {
        let p = ((clip.end() - t) / tout.duration).clamp(0.0, 1.0);
        if p < 1.0 {
            apply(&mut st, tout.kind, p, true);
        }
    }
    st
}
