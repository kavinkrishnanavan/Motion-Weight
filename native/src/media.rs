//! Importing media: probing, thumbnailing and waveform extraction, all off the
//! UI thread so a large file never stalls a frame.

use crate::decode;
use crate::ffmpeg;
use crate::model::{next_id, AssetKind, MediaAsset};

const VIDEO_EXT: [&str; 6] = ["mp4", "mov", "mkv", "avi", "webm", "m4v"];
const IMAGE_EXT: [&str; 5] = ["png", "jpg", "jpeg", "webp", "bmp"];
const AUDIO_EXT: [&str; 6] = ["mp3", "wav", "m4a", "aac", "ogg", "flac"];

pub fn all_extensions() -> Vec<&'static str> {
    VIDEO_EXT.iter().chain(IMAGE_EXT.iter()).chain(AUDIO_EXT.iter()).copied().collect()
}

fn ext_of(path: &str) -> String {
    path.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

fn name_of(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

/// Thumbnail width in the media bin. Small on purpose: forty assets at this
/// size cost about two megabytes in total.
pub const THUMB_W: u32 = 160;

pub struct Imported {
    pub asset: MediaAsset,
    pub thumbnail: Option<decode::Frame>,
}

/// Probes a file and builds its asset record. Blocking; call from a worker.
pub fn build_asset(path: &str) -> Result<Imported, String> {
    let ext = ext_of(path);
    let name = name_of(path);

    if IMAGE_EXT.contains(&ext.as_str()) {
        let probe = ffmpeg::probe(path)?;
        let (w, h) = (probe.width.max(1), probe.height.max(1));
        let thumbnail = decode::thumbnail(path, 0.0, true, THUMB_W, w as f32 / h as f32);
        return Ok(Imported {
            asset: MediaAsset {
                id: next_id(),
                path: path.to_string(),
                name,
                kind: AssetKind::Image,
                duration: 3.0,
                width: w,
                height: h,
                has_audio: false,
                waveform: Vec::new(),
            },
            thumbnail,
        });
    }

    if AUDIO_EXT.contains(&ext.as_str()) {
        let probe = ffmpeg::probe(path)?;
        let waveform = decode::waveform(path, probe.duration).unwrap_or_default();
        return Ok(Imported {
            asset: MediaAsset {
                id: next_id(),
                path: path.to_string(),
                name,
                kind: AssetKind::Audio,
                duration: probe.duration,
                width: 0,
                height: 0,
                has_audio: true,
                waveform,
            },
            thumbnail: None,
        });
    }

    let probe = ffmpeg::probe(path)?;
    let (w, h) = (probe.width.max(1), probe.height.max(1));
    let kind = if probe.is_image { AssetKind::Image } else { AssetKind::Video };
    let thumbnail = decode::thumbnail(path, probe.duration, probe.is_image, THUMB_W, w as f32 / h as f32);
    let waveform = if probe.has_audio {
        decode::waveform(path, probe.duration).unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(Imported {
        asset: MediaAsset {
            id: next_id(),
            path: path.to_string(),
            name,
            kind,
            duration: if kind == AssetKind::Image { 3.0 } else { probe.duration },
            width: w,
            height: h,
            has_audio: probe.has_audio,
            waveform,
        },
        thumbnail,
    })
}

pub fn format_duration(seconds: f32) -> String {
    let m = (seconds / 60.0).floor() as i64;
    let s = (seconds % 60.0).floor() as i64;
    format!("{}:{:02}", m, s)
}

/// CapCut-style HH:MM:SS:FF timecode.
pub fn timecode(seconds: f32, fps: u32) -> String {
    let s = seconds.max(0.0);
    let h = (s / 3600.0).floor() as i64;
    let m = ((s % 3600.0) / 60.0).floor() as i64;
    let sec = (s % 60.0).floor() as i64;
    let f = ((s % 1.0) * fps as f32).floor() as i64;
    format!("{:02}:{:02}:{:02}:{:02}", h, m, sec, f)
}
