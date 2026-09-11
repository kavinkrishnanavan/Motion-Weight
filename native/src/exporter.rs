//! Turns the edit state into the flat clip lists ffmpeg wants, rasterizing each
//! clip's mask into a temp file on the way.

use crate::ffmpeg::ExportClip;
use crate::mask;
use crate::model::{Clip, ClipKind};
use crate::store::Store;
use crate::transitions::transition_at;

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("motionweight");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Rasterizes a clip's mask into a raw 8-bit gray plane sized to the clip's
/// on-screen box. The mask is authored in frame coordinates, so it is rebased
/// into the clip's own box first.
fn mask_file(clip: &Clip, pw: u32, ph: u32) -> Option<(String, u32, u32)> {
    if clip.kind == ClipKind::Text || !clip.mask.is_active() {
        return None;
    }
    let box_w = (clip.width * pw as f32).round().max(2.0) as u32;
    let box_h = (clip.height * ph as f32).round().max(2.0) as u32;
    let local = mask::localize(
        &clip.mask,
        clip.x,
        clip.y,
        clip.width,
        clip.height,
        pw.min(ph) as f32,
        box_w.min(box_h) as f32,
    );
    let plane = mask::render(&local, box_w, box_h);
    // Written as a PNG rather than a raw plane: ffmpeg's image demuxer can loop
    // a still for the clip's lifetime, which the rawvideo demuxer cannot.
    let mut pixmap = tiny_skia::Pixmap::new(box_w, box_h)?;
    for (px, cov) in pixmap.pixels_mut().iter_mut().zip(plane.data.iter()) {
        *px = tiny_skia::PremultipliedColorU8::from_rgba(*cov, *cov, *cov, 255)?;
    }
    let path = temp_dir().join(format!("mask-{}.png", clip.id));
    pixmap.save_png(&path).ok()?;
    Some((path.to_string_lossy().to_string(), box_w, box_h))
}

fn to_export(store: &Store, clip: &Clip, pw: u32, ph: u32) -> ExportClip {
    let asset = store.asset(clip.asset_id);
    ExportClip {
        kind: clip.kind,
        path: asset.map(|a| a.path.clone()),
        text: clip.text.clone(),
        start: clip.start,
        duration: clip.duration,
        trim_in: if clip.kind == ClipKind::Text { 0.0 } else { clip.trim_in },
        has_audio: asset.map(|a| a.has_audio).unwrap_or(false),
        x: clip.x,
        y: clip.y,
        width: clip.width,
        height: clip.height,
        opacity: clip.opacity,
        volume: if clip.kind == ClipKind::Text { 0.0 } else { clip.volume },
        font_size: clip.font_size,
        color: clip.color.clone(),
        font_family: clip.font_family.clone(),
        bold: clip.bold,
        italic: clip.italic,
        bg_color: (clip.kind == ClipKind::Text && clip.bg_enabled).then(|| clip.bg_color.clone()),
        crop: clip.crop,
        color_grade: (clip.kind != ClipKind::Text).then(|| clip.color_grade.clone()).filter(|g| g.is_active()),
        mask: mask_file(clip, pw, ph),
        trans_in: clip.transition_in,
        trans_out: clip.transition_out,
        fade_in: clip.fade_in,
        fade_out: clip.fade_out,
    }
}

/// ffmpeg paints tracks in the order given, so send them bottom layer first.
pub fn build_tracks(store: &Store) -> Vec<Vec<ExportClip>> {
    let (pw, ph) = (store.project.settings.width, store.project.settings.height);
    store
        .tracks_bottom_first()
        .map(|t| t.clips.iter().map(|c| to_export(store, c, pw, ph)).collect())
        .collect()
}

/// Same shape, but with masks skipped — used for the audio playback stream,
/// where the video branch is never rendered.
pub fn build_audio_tracks(store: &Store) -> Vec<Vec<ExportClip>> {
    let (pw, ph) = (store.project.settings.width, store.project.settings.height);
    store
        .tracks_bottom_first()
        .map(|t| {
            t.clips
                .iter()
                .filter(|c| matches!(c.kind, ClipKind::Audio | ClipKind::Video))
                .map(|c| {
                    let mut spec = to_export(store, c, pw, ph);
                    spec.mask = None;
                    spec
                })
                .collect()
        })
        .collect()
}

/// The clips visible at `t`, with each transition's state at that instant baked
/// into geometry and opacity — a still frame has no timeline for ffmpeg to
/// animate against.
pub fn build_frame_tracks(store: &Store, t: f32) -> Vec<Vec<ExportClip>> {
    let (pw, ph) = (store.project.settings.width, store.project.settings.height);
    store
        .tracks_bottom_first()
        .map(|track| {
            track
                .clips
                .iter()
                .filter(|c| c.covers(t))
                .map(|c| {
                    let mut shifted = c.clone();
                    let local = t - c.start;
                    shifted.start = 0.0;
                    if c.kind != ClipKind::Text {
                        let st = transition_at(c, t);
                        shifted.trim_in = c.trim_in + local;
                        shifted.x = c.x + st.offset_x * c.width;
                        shifted.y = c.y + st.offset_y * c.height;
                        shifted.width = c.width * st.scale;
                        shifted.height = c.height * st.scale;
                        shifted.opacity = c.opacity * st.alpha;
                    }
                    let mut spec = to_export(store, &shifted, pw, ph);
                    spec.trans_in = Default::default();
                    spec.trans_out = Default::default();
                    spec
                })
                .collect()
        })
        .collect()
}

/// Removes the mask stencils an export left behind.
pub fn cleanup_temp() {
    let Ok(entries) = std::fs::read_dir(temp_dir()) else { return };
    for entry in entries.flatten() {
        let is_stencil = entry.file_name().to_string_lossy().starts_with("mask-");
        if is_stencil {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
