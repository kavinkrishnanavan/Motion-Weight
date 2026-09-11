//! Export pipeline: builds one `filter_complex` graph for the whole timeline and
//! hands it to ffmpeg. Layers are overlaid bottom-first, so the caller must send
//! tracks in composite order.

use crate::model::{ClipKind, ColorGrade, Crop, Transition, TransitionType};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn command(bin: &str) -> Command {
    let mut cmd = Command::new(bin);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn find_binary(name: &str) -> Option<String> {
    let probe = command(name).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status();
    if matches!(probe, Ok(s) if s.success()) {
        return Some(name.to_string());
    }
    let candidate = format!("C:\\ffmpeg\\bin\\{}.exe", name);
    if PathBuf::from(&candidate).exists() {
        return Some(candidate);
    }
    None
}

static FFMPEG: OnceLock<Option<String>> = OnceLock::new();
static FFPROBE: OnceLock<Option<String>> = OnceLock::new();

pub fn ffmpeg_bin() -> Result<&'static str, String> {
    FFMPEG
        .get_or_init(|| find_binary("ffmpeg"))
        .as_deref()
        .ok_or_else(|| "ffmpeg was not found. Install ffmpeg and put it on PATH.".to_string())
}

pub fn ffprobe_bin() -> Result<&'static str, String> {
    FFPROBE
        .get_or_init(|| find_binary("ffprobe"))
        .as_deref()
        .ok_or_else(|| "ffprobe was not found. Install ffmpeg (it includes ffprobe) and put it on PATH.".to_string())
}

pub fn available() -> bool {
    ffmpeg_bin().is_ok() && ffprobe_bin().is_ok()
}

// ------------------------------------------------------------------- probing

#[derive(Clone, Debug)]
pub struct MediaProbe {
    pub duration: f32,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
    pub is_image: bool,
}

pub fn probe(path: &str) -> Result<MediaProbe, String> {
    let bin = ffprobe_bin()?;
    let output = command(bin)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams", path])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let empty = Vec::new();
    let streams = json["streams"].as_array().unwrap_or(&empty);
    let video = streams.iter().find(|s| s["codec_type"] == "video");
    let has_audio = streams.iter().any(|s| s["codec_type"] == "audio");

    let duration = json["format"]["duration"].as_str().and_then(|s| s.parse::<f32>().ok());
    let single_frame = video.and_then(|s| s["nb_frames"].as_str()).map(|s| s == "1").unwrap_or(false);

    Ok(MediaProbe {
        duration: duration.unwrap_or(5.0),
        width: video.and_then(|s| s["width"].as_u64()).unwrap_or(0) as u32,
        height: video.and_then(|s| s["height"].as_u64()).unwrap_or(0) as u32,
        has_audio,
        is_image: video.is_some() && (duration.is_none() || single_frame),
    })
}

// ---------------------------------------------------------------- graph input

/// A clip flattened for export: geometry already resolved, mask already
/// rasterized to a raw gray plane on disk.
#[derive(Clone, Debug)]
pub struct ExportClip {
    pub kind: ClipKind,
    pub path: Option<String>,
    pub text: String,
    pub start: f32,
    pub duration: f32,
    pub trim_in: f32,
    pub has_audio: bool,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub opacity: f32,
    pub volume: f32,
    pub font_size: f32,
    pub color: String,
    pub font_family: String,
    pub bold: bool,
    pub italic: bool,
    pub bg_color: Option<String>,
    pub crop: Crop,
    /// `None` when the clip has no grading applied — `is_active()` was
    /// already checked building this, so its mere presence here means work.
    pub color_grade: Option<ColorGrade>,
    /// Path plus dimensions of a raw 8-bit gray stencil sized to the clip box.
    pub mask: Option<(String, u32, u32)>,
    pub trans_in: Transition,
    pub trans_out: Transition,
    pub fade_in: f32,
    pub fade_out: f32,
}

fn esc_drawtext(s: &str) -> String {
    s.replace('\\', "\\\\\\\\")
        .replace(':', "\\:")
        .replace('\'', "\u{2019}")
        .replace('%', "\\%")
        .replace('\n', " ")
}

fn font_file(family: &str, bold: bool, italic: bool) -> String {
    let file = match (family, bold, italic) {
        ("Georgia", true, _) => "georgiab.ttf",
        ("Georgia", false, _) => "georgia.ttf",
        ("Impact", _, _) => "impact.ttf",
        ("Times New Roman", true, _) => "timesbd.ttf",
        ("Times New Roman", false, _) => "times.ttf",
        ("Courier New", true, _) => "courbd.ttf",
        ("Courier New", false, _) => "cour.ttf",
        (_, true, true) => "arialbi.ttf",
        (_, true, false) => "arialbd.ttf",
        (_, false, true) => "ariali.ttf",
        _ => "arial.ttf",
    };
    format!("C:/Windows/Fonts/{}", file)
}

/// The scalar side of a color grade (temperature/tint/exposure/
/// highlights/shadows/curve) as ffmpeg filters, comma-joined with a
/// trailing comma so it drops straight into another filter chain. `colorbalance`
/// alone carries temperature+tint (as a midtone shift) and highlights+shadows
/// (its own named ranges); `exposure` and `curves` are ffmpeg's own filters of
/// the same name. Preview applies the identical fields with its own Rust math
/// (see `color::apply`) — the two are independent implementations of the same
/// numbers, not a shared code path, so they read the same but aren't pixel-
/// identical.
fn color_grade_filters(grade: &ColorGrade) -> String {
    let mut out = String::new();
    let (t, ti) = (grade.temperature, grade.tint);
    let (hi, sh) = (grade.highlights.clamp(-1.0, 1.0), grade.shadows.clamp(-1.0, 1.0));
    if t.abs() > 1e-4 || ti.abs() > 1e-4 || hi.abs() > 1e-4 || sh.abs() > 1e-4 {
        let rm = (t - ti * 0.5).clamp(-1.0, 1.0);
        let gm = ti.clamp(-1.0, 1.0);
        let bm = (-t - ti * 0.5).clamp(-1.0, 1.0);
        out.push_str(&format!(
            "colorbalance=rs={sh:.3}:gs={sh:.3}:bs={sh:.3}:rm={rm:.3}:gm={gm:.3}:bm={bm:.3}:rh={hi:.3}:gh={hi:.3}:bh={hi:.3},",
        ));
    }
    if grade.exposure.abs() > 1e-4 {
        out.push_str(&format!("exposure=exposure={:.3},", grade.exposure * 2.0));
    }
    if grade.curve.len() > 2 || grade.curve != [(0.0, 0.0), (1.0, 1.0)] {
        let mut pts = grade.curve.clone();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let points: Vec<String> = pts.iter().map(|(x, y)| format!("{:.3}/{:.3}", x, y)).collect();
        out.push_str(&format!("curves=master='{}',", points.join(" ")));
    }
    out
}

fn hex_to_ffcolor(hex: &str, opacity: f32) -> String {
    format!("0x{}@{:.3}", hex.trim_start_matches('#'), opacity.clamp(0.0, 1.0))
}

struct Ctx {
    args: Vec<String>,
    next_input: usize,
}

impl Ctx {
    fn push(&mut self, parts: &[&str]) {
        self.args.extend(parts.iter().map(|s| s.to_string()));
    }

    fn add_media_input(&mut self, clip: &ExportClip) -> usize {
        let idx = self.next_input;
        let path = clip.path.clone().unwrap_or_default();
        if clip.kind == ClipKind::Image {
            self.push(&["-loop", "1", "-t", &format!("{:.3}", clip.duration), "-i", &path]);
        } else {
            self.push(&[
                "-ss",
                &format!("{:.3}", clip.trim_in),
                "-t",
                &format!("{:.3}", clip.duration),
                "-i",
                &path,
            ]);
        }
        self.next_input += 1;
        idx
    }

    /// A grayscale stencil still, looped for the clip's lifetime.
    fn add_mask_input(&mut self, path: &str, duration: f32) -> usize {
        let idx = self.next_input;
        self.push(&["-loop", "1", "-t", &format!("{:.3}", duration), "-i", path]);
        self.next_input += 1;
        idx
    }
}

/// The animated pieces of a clip's in/out transitions, as ffmpeg expressions.
#[derive(Default)]
struct TransitionFx {
    /// Uniform scale, applied to both width and height unless `scale_x`
    /// overrides the width half.
    scale: Option<String>,
    /// Width-only override on top of `scale` — `CubeRotation`'s squish.
    scale_x: Option<String>,
    rotate: Option<String>,
    /// Edge-flush boxes that punch a transparent hole in the not-yet-revealed
    /// part of a wipe. `drawbox` cannot do this animated (see `WipeBox`).
    wipes: Vec<WipeBox>,
    /// `fade` clauses operating on the alpha channel.
    fades: Vec<String>,
    /// Offsets added to the overlay position, in global timeline time.
    offset_x: Vec<String>,
    offset_y: Vec<String>,
    /// Discrete (block size, `enable` window) steps for `Pixelate`. A real
    /// per-frame-animated block size (via `scale`'s `eval=frame` with
    /// `flags=neighbor`, the same trick the wipes use) hits a genuine ffmpeg
    /// bug: swscale's nearest-neighbor context caches the previous frame's
    /// stride, and a downscale target that changes size every frame corrupts
    /// it ("Input picture width is greater than stride"), silently truncating
    /// the whole output to one frame. A handful of fixed-size steps, each
    /// active only in its own time slice, sidesteps the bug entirely.
    pixel_steps: Vec<(u32, String)>,
}

/// One edge-flush rectangle contributing to a wipe's coverage mask, built
/// from `scale` (animatable via `eval=frame`) rather than `drawbox` — see the
/// comment on `symmetric_wipe_boxes` for why `drawbox` cannot be used here.
struct WipeBox {
    /// A full-height strip whose width shrinks (true), vs a full-width strip
    /// whose height shrinks (false).
    horizontal: bool,
    /// Stays flush against the left/top edge (true) or the right/bottom edge
    /// (false) as it shrinks.
    from_start: bool,
    /// 0..1 expression (in `t`) for the strip's thickness, as a fraction of
    /// the clip's own box.
    frac: String,
    /// Clip-local `enable` clause, active only while this wipe is animating.
    enable: String,
}

/// A `fade` through a solid color, timed at the start or end of the clip's
/// duration `dur` depending on `is_out`. Layered on top of the normal alpha
/// fade, a short `fd` reads as a flash/glow pop; the full transition duration
/// reads as a genuine dip through that color (`DipToBlack`/`DipToWhite`).
fn color_fade(is_out: bool, dur: f32, fd: f32, color: &str) -> String {
    if is_out {
        format!("fade=t=out:st={:.3}:d={:.3}:color={}", (dur - fd).max(0.0), fd, color)
    } else {
        format!("fade=t=in:st=0:d={:.3}:color={}", fd, color)
    }
}

/// The clip-local time window (as an `enable` clause) during which a wipe is
/// actively animating, so the extra filter stages it costs are skipped
/// entirely outside it.
fn wipe_enable(is_out: bool, dur: f32, d: f32) -> String {
    if is_out {
        format!("gt(t,{:.3})", (dur - d).max(0.0))
    } else {
        format!("lt(t,{:.3})", d)
    }
}

/// Five fixed block sizes stepping from coarse to sharp (or the reverse for
/// an out-transition), each confined to its own slice of the transition
/// window via `enable` — see the comment on `TransitionFx::pixel_steps` for
/// why the size can't just animate continuously.
fn pixelate_steps(is_out: bool, dur: f32, d: f32) -> Vec<(u32, String)> {
    const BLOCKS: [u32; 5] = [48, 28, 16, 8, 3];
    let n = BLOCKS.len();
    let win0 = if is_out { dur - d } else { 0.0 };
    (0..n)
        .map(|i| {
            let lo = win0 + d * (i as f32) / (n as f32);
            let hi = win0 + d * ((i + 1) as f32) / (n as f32);
            let block = if is_out { BLOCKS[n - 1 - i] } else { BLOCKS[i] };
            (block, format!("between(t,{:.3},{:.3})", lo, hi))
        })
        .collect()
}

/// Two (or four) edge-flush boxes closing in on the clip from opposite sides
/// at once. `p` is the 0..1 on-screen progress expression; `horizontal_only`
/// gives a left/right curtain instead of all four sides.
fn symmetric_wipe_boxes(p: &str, enable: &str, horizontal_only: bool) -> Vec<WipeBox> {
    let half = format!("(0.5*(1-{}))", p);
    let mut out = vec![
        WipeBox { horizontal: true, from_start: true, frac: half.clone(), enable: enable.to_string() },
        WipeBox { horizontal: true, from_start: false, frac: half.clone(), enable: enable.to_string() },
    ];
    if !horizontal_only {
        out.push(WipeBox { horizontal: false, from_start: true, frac: half.clone(), enable: enable.to_string() });
        out.push(WipeBox { horizontal: false, from_start: false, frac: half, enable: enable.to_string() });
    }
    out
}

fn transition_fx(clip: &ExportClip) -> TransitionFx {
    use TransitionType::*;
    let mut fx = TransitionFx::default();
    let dur = clip.duration.max(0.01);
    let end = clip.end();

    let mut scales: Vec<String> = Vec::new();
    let mut scales_x: Vec<String> = Vec::new();
    let mut rotates: Vec<String> = Vec::new();

    for is_out in [false, true] {
        let t = if is_out { clip.trans_out } else { clip.trans_in };
        if !t.is_active() {
            continue;
        }
        let d = t.duration.min(dur);
        // `p` runs 0 (transitioned away) -> 1 (fully on screen), in local clip time.
        let p = if is_out {
            format!("min(max(({:.3}-t)/{:.3},0),1)", dur, d)
        } else {
            format!("min(max(t/{:.3},0),1)", d)
        };
        // The same ramp against the global timeline, for overlay position expressions.
        let gp = if is_out {
            format!("min(max(({:.3}-t)/{:.3},0),1)", end, d)
        } else {
            format!("min(max((t-{:.3})/{:.3},0),1)", clip.start, d)
        };
        let sign: f32 = if is_out { -1.0 } else { 1.0 };
        let fade = if is_out {
            format!("fade=t=out:st={:.3}:d={:.3}:alpha=1", (dur - d).max(0.0), d)
        } else {
            format!("fade=t=in:st=0:d={:.3}:alpha=1", d)
        };

        match t.kind {
            Fade | CrossDissolve => fx.fades.push(fade),
            // The real dip: a fade through a solid color spanning the whole
            // transition window, not just a short flash.
            DipToBlack => fx.fades.push(color_fade(is_out, dur, d, "black")),
            DipToWhite => fx.fades.push(color_fade(is_out, dur, d, "white")),
            WipeLeft | WipeRight | WipeUp | WipeDown => {
                let horizontal = matches!(t.kind, WipeLeft | WipeRight);
                // Which side stays hidden flips between the in and out halves.
                let hide_from_start = matches!(
                    (t.kind, is_out),
                    (WipeLeft, false) | (WipeRight, true) | (WipeUp, false) | (WipeDown, true)
                );
                let enable = wipe_enable(is_out, dur, d);
                fx.wipes.push(WipeBox { horizontal, from_start: hide_from_start, frac: format!("(1-{})", p), enable });
            }
            // Constant-speed, edge-to-edge — the mechanical "push" feel.
            PushLeft | PushRight | PushUp | PushDown => {
                let flip = matches!(t.kind, PushRight | PushDown);
                let s = if flip { -sign } else { sign };
                let horizontal = matches!(t.kind, PushLeft | PushRight);
                let dim = if horizontal { "w" } else { "h" };
                let expr = format!("({:.1})*{}*(1-{})", s, dim, gp);
                if horizontal {
                    fx.offset_x.push(expr);
                } else {
                    fx.offset_y.push(expr);
                }
            }
            // Same motion as `Push`, eased with a smoothstep so it glides
            // rather than moving at constant speed.
            SlideLeft | SlideRight => {
                let flip = matches!(t.kind, SlideRight);
                let s = if flip { -sign } else { sign };
                let e = format!("({0}*{0}*(3-2*{0}))", gp);
                fx.offset_x.push(format!("({:.1})*w*(1-{})", s, e));
            }
            ZoomIn => {
                fx.fades.push(fade);
                scales.push(if is_out {
                    format!("(1+0.4*(1-{}))", p)
                } else {
                    format!("(0.6+0.4*{})", p)
                });
            }
            ZoomOut => {
                fx.fades.push(fade);
                scales.push(if is_out {
                    format!("(1-0.6*(1-{}))", p)
                } else {
                    format!("(1.8-0.8*{})", p)
                });
            }
            // A modest quarter-turn, distinct from `Spin360`'s full rotation.
            Spin => {
                fx.fades.push(fade);
                scales.push(format!("(0.75+0.25*{})", p));
                rotates.push(format!("({:.1})*(1-{})*1*PI", sign, p));
            }
            Spin360 => {
                fx.fades.push(fade);
                scales.push(format!("(0.6+0.4*{})", p));
                rotates.push(format!("({:.1})*(1-{})*2*PI", sign, p));
            }
            // A true non-uniform squish, distinct from the uniform zooms —
            // width only, via `scale_x`, rather than `scale`.
            CubeRotation => {
                scales_x.push(format!("max(0.02,{})", p));
                fx.offset_x.push(format!("({:.1})*w*(1-{})", sign, p));
            }
            // A wipe-style reveal plus a slight rotation, rather than
            // `CubeRotation`'s symmetric squish.
            // Rotate changes the layer's own output box size (it pads to the
            // diagonal), while a wipe's mask is built at the original box
            // size — combined, `alphamerge` gets mismatched dimensions. No
            // other transition needs both at once, so `PageTurn` keeps just
            // the wipe-style reveal in export; the preview still adds its
            // subtle tilt.
            PageTurn => {
                let horizontal = true;
                let hide_from_start = !is_out;
                let enable = wipe_enable(is_out, dur, d);
                fx.wipes.push(WipeBox { horizontal, from_start: hide_from_start, frac: format!("(1-{})", p), enable });
            }
            Pixelate => fx.pixel_steps.extend(pixelate_steps(is_out, dur, d)),
            // The preview renders true circle/heart/diamond/clock stencils via
            // its own mask rasterizer; there is no equally safe way to
            // hand-compose that as a raw ffmpeg filter expression, so export
            // approximates every shape wipe the same way: a box closing in
            // from all four sides at once, reusing the exact drawbox
            // mechanism already proven by the plain wipe transitions above.
            ClockWipe | Iris | Diamond | Circle | Heart => {
                let enable = wipe_enable(is_out, dur, d);
                fx.wipes.extend(symmetric_wipe_boxes(&p, &enable, false));
            }
            // A fast, decaying positional jitter stands in for the strip
            // splitting the preview does; ffmpeg can already animate an
            // overlay's x position per frame (used by the pushes above), so
            // this reuses that instead of building N crop/overlay chains.
            Glitch => {
                fx.offset_x.push(format!("(0.03)*sin(t*55)*(1-{})*w", gp));
            }
            // No per-pixel directional blur filter is wired up for export;
            // the overshoot and fade carry the "whip" feel on their own.
            MotionBlur => {
                fx.fades.push(fade);
                fx.offset_x.push(format!("({:.1})*w*0.15*(1-{})", sign, p));
            }
            WhipPan => {
                fx.offset_x.push(format!("({:.1})*w*1.4*(1-{})", sign, p));
            }
            // A short color fade through white, layered on top of the normal
            // alpha fade, reads as a flash/pop without needing a filter that
            // can animate a drawbox's own alpha (drawbox's `color@a` is a
            // fixed value, not a per-frame expression).
            LightFlash => {
                fx.fades.push(fade);
                fx.fades.push(color_fade(is_out, dur, (d * 0.5).max(0.05), "white"));
            }
            None => {}
        }
    }

    if !scales.is_empty() {
        fx.scale = Some(scales.join("*"));
    }
    if !scales_x.is_empty() {
        fx.scale_x = Some(scales_x.join("*"));
    }
    if !rotates.is_empty() {
        fx.rotate = Some(rotates.join("+"));
    }
    fx
}

impl ExportClip {
    fn end(&self) -> f32 {
        self.start + self.duration
    }
}

/// Level, fades, then the delay that places the clip on the timeline. The fades
/// run in clip-local time, before the delay shifts it.
fn audio_chain(clip: &ExportClip, idx: usize, label: &str) -> String {
    let ms = (clip.start * 1000.0).round().max(0.0) as i64;
    let dur = clip.duration.max(0.01);
    let mut chain = format!(
        "[{}:a]aformat=channel_layouts=stereo,asetpts=PTS-STARTPTS,volume={:.3}",
        idx, clip.volume
    );
    let fade_in = clip.fade_in.clamp(0.0, dur);
    if fade_in > 0.001 {
        chain.push_str(&format!(",afade=t=in:st=0:d={:.3}", fade_in));
    }
    let fade_out = clip.fade_out.clamp(0.0, dur);
    if fade_out > 0.001 {
        chain.push_str(&format!(",afade=t=out:st={:.3}:d={:.3}", dur - fade_out, fade_out));
    }
    chain.push_str(&format!(",adelay={}|{}[{}];", ms, ms, label));
    chain
}

pub fn total_duration(tracks: &[Vec<ExportClip>]) -> f32 {
    tracks
        .iter()
        .flat_map(|t| t.iter())
        .map(|c| c.end())
        .fold(0.0_f32, f32::max)
        .max(0.1)
}

fn build_graph(
    w: u32,
    h: u32,
    fps: Option<u32>,
    tracks: &[Vec<ExportClip>],
    total: f32,
    need_audio: bool,
    need_video: bool,
) -> (Ctx, String, Option<String>, Option<String>) {
    let mut ctx = Ctx { args: Vec::new(), next_input: 0 };
    let mut fc = String::new();

    // An audio-only mix (playback scrubbing audio) has no use for the video
    // chain, and a named filter output that nothing maps or consumes is a
    // hard error on modern ffmpeg ("has an unconnected output") — so the
    // whole video branch, including the base canvas, is skipped outright.
    let mut running = String::new();
    if need_video {
        let fps_part = fps.map(|f| format!(":r={}", f)).unwrap_or_default();
        fc.push_str(&format!("color=c=black:s={}x{}:d={:.3}{}[base];", w, h, total, fps_part));
        running = "base".to_string();
    }
    let mut stage = 0usize;
    let mut audio_labels: Vec<String> = Vec::new();

    for track in tracks {
        for clip in track {
            let end = clip.end();
            match clip.kind {
                ClipKind::Video | ClipKind::Image => {
                    let idx = ctx.add_media_input(clip);
                    if need_video {
                    // Even dimensions keep libx264 and the scaler happy.
                    let ow = ((clip.width * w as f32 / 2.0).round() * 2.0).max(2.0) as i64;
                    let oh = ((clip.height * h as f32 / 2.0).round() * 2.0).max(2.0) as i64;
                    let cx = (clip.x * w as f32).round() as i64;
                    let cy = (clip.y * h as f32).round() as i64;
                    let fx = transition_fx(clip);

                    // Source crop, then fit to the clip's on-screen box.
                    let (cl, ct) = (clip.crop.left.clamp(0.0, 0.95), clip.crop.top.clamp(0.0, 0.95));
                    let (cr, cb) = (clip.crop.right.clamp(0.0, 0.95), clip.crop.bottom.clamp(0.0, 0.95));
                    let mut pre = String::new();
                    if cl + cr > 1e-4 || ct + cb > 1e-4 {
                        pre.push_str(&format!(
                            "crop=w=iw*{:.4}:h=ih*{:.4}:x=iw*{:.4}:y=ih*{:.4},",
                            (1.0 - cl - cr).max(0.05),
                            (1.0 - ct - cb).max(0.05),
                            cl,
                            ct
                        ));
                    }
                    pre.push_str(&format!("scale={}:{},format=rgba,setsar=1,", ow, oh));
                    if let Some(grade) = &clip.color_grade {
                        pre.push_str(&color_grade_filters(grade));
                    }
                    pre.pop(); // drop the trailing comma every branch above leaves
                    let mut cur = format!("pre{}", stage);
                    fc.push_str(&format!("[{}:v]{}[{}];", idx, pre, cur));

                    if let Some((path, _, _)) = &clip.mask {
                        let midx = ctx.add_mask_input(path, clip.duration);
                        let mk = format!("mk{}", stage);
                        fc.push_str(&format!("[{}:v]scale={}:{},format=gray[{}];", midx, ow, oh, mk));
                        let merged = format!("pm{}", stage);
                        fc.push_str(&format!("[{}][{}]alphamerge[{}];", cur, mk, merged));
                        cur = merged;
                    }

                    // A LUT is layered on top of the scalar grade rather than
                    // replacing it, and `lut3d` has no strength knob of its
                    // own — so it's applied on a split branch and blended
                    // back in at `lut_strength`, exactly like `alphamerge`
                    // above swaps `cur` to the merged result.
                    if let Some(grade) = clip.color_grade.as_ref().filter(|g| !g.lut_path.is_empty()) {
                        let lut_path = grade.lut_path.replace('\\', "/");
                        let base = format!("lb{}", stage);
                        let branch = format!("li{}", stage);
                        let graded = format!("lo{}", stage);
                        fc.push_str(&format!("[{}]split=2[{}][{}];", cur, base, branch));
                        fc.push_str(&format!("[{}]lut3d=file='{}'[{}];", branch, lut_path, graded));
                        let blended = format!("lm{}", stage);
                        fc.push_str(&format!(
                            "[{}][{}]blend=all_mode=normal:all_opacity={:.3}[{}];",
                            graded,
                            base,
                            grade.lut_strength.clamp(0.0, 1.0),
                            blended
                        ));
                        cur = blended;
                    }

                    let mut post = String::new();
                    for (block, enable) in &fx.pixel_steps {
                        post.push_str(&format!("pixelize=w={0}:h={0}:enable='{1}',", block, enable));
                    }
                    if fx.scale.is_some() || fx.scale_x.is_some() {
                        let sx = fx.scale_x.as_deref().or(fx.scale.as_deref()).unwrap_or("1");
                        let sy = fx.scale.as_deref().unwrap_or("1");
                        post.push_str(&format!(
                            "scale=w='trunc({}*({})/2)*2':h='trunc({}*({})/2)*2':eval=frame,",
                            ow, sx, oh, sy
                        ));
                    }
                    if let Some(a) = &fx.rotate {
                        // A fixed output box keeps the corners at any input size.
                        let side = (((ow * ow + oh * oh) as f64).sqrt().ceil() as i64 / 2) * 2 + 2;
                        post.push_str(&format!("rotate=a='{}':c=black@0:ow={}:oh={},", a, side, side));
                    }
                    for fade in &fx.fades {
                        post.push_str(fade);
                        post.push(',');
                    }
                    post.push_str(&format!(
                        "colorchannelmixer=aa={:.3},setpts=PTS+{:.3}/TB",
                        clip.opacity, clip.start
                    ));
                    let layer = format!("layer{}", stage);
                    fc.push_str(&format!("[{}]{}[{}];", cur, post, layer));

                    // A wipe punches a transparent hole in the not-yet-revealed
                    // edge(s) of the layer. `drawbox` cannot animate its own
                    // size or position per frame in this ffmpeg build (only
                    // `enable` is per-frame there), so instead each edge is
                    // built as a black rectangle on an otherwise-white canvas
                    // — sized via `scale`, which *does* support `eval=frame` —
                    // and the accumulated white/black plane is used exactly
                    // like the static clip.mask stencil above: one `alphamerge`
                    // at the end punches every edge's hole at once.
                    let mut wiped = layer.clone();
                    if !fx.wipes.is_empty() {
                        let fps_n = fps.unwrap_or(30);
                        let dur = clip.duration.max(0.05);
                        let mut mask_run = format!("wmbase{}", stage);
                        fc.push_str(&format!(
                            "color=c=white:s={}x{}:d={:.3}:r={}[{}];",
                            ow, oh, dur, fps_n, mask_run
                        ));
                        for (wi, wb) in fx.wipes.iter().enumerate() {
                            let box_src = format!("wbsrc{}_{}", stage, wi);
                            let box_scaled = format!("wbox{}_{}", stage, wi);
                            fc.push_str(&format!(
                                "color=c=black:s={}x{}:d={:.3}:r={}[{}];",
                                ow, oh, dur, fps_n, box_src
                            ));
                            let (dim_w, dim_h) = if wb.horizontal {
                                (format!("'max(trunc(iw*({})/2)*2,2)'", wb.frac), "ih".to_string())
                            } else {
                                ("iw".to_string(), format!("'max(trunc(ih*({})/2)*2,2)'", wb.frac))
                            };
                            fc.push_str(&format!(
                                "[{}]scale=w={}:h={}:eval=frame[{}];",
                                box_src, dim_w, dim_h, box_scaled
                            ));
                            let (x_e, y_e) = if wb.horizontal {
                                (if wb.from_start { "0" } else { "main_w-overlay_w" }, "0")
                            } else {
                                ("0", if wb.from_start { "0" } else { "main_h-overlay_h" })
                            };
                            let next_mask = format!("wmask{}_{}", stage, wi);
                            fc.push_str(&format!(
                                "[{}][{}]overlay=x='{}':y='{}':eval=frame:enable='{}'[{}];",
                                mask_run, box_scaled, x_e, y_e, wb.enable, next_mask
                            ));
                            mask_run = next_mask;
                        }
                        let mask_gray = format!("wmaskg{}", stage);
                        fc.push_str(&format!("[{}]format=gray[{}];", mask_run, mask_gray));
                        let after_wipe = format!("wiped{}", stage);
                        fc.push_str(&format!("[{}][{}]alphamerge[{}];", wiped, mask_gray, after_wipe));
                        wiped = after_wipe;
                    }

                    // Position by centre so animated layer sizes stay centred.
                    let mut x_expr = format!("{}-w/2", cx);
                    for off in &fx.offset_x {
                        x_expr.push('+');
                        x_expr.push_str(off);
                    }
                    let mut y_expr = format!("{}-h/2", cy);
                    for off in &fx.offset_y {
                        y_expr.push('+');
                        y_expr.push_str(off);
                    }
                    let next = format!("v{}", stage);
                    fc.push_str(&format!(
                        "[{}][{}]overlay=x='{}':y='{}':eval=frame:enable='between(t,{:.3},{:.3})'[{}];",
                        running, wiped, x_expr, y_expr, clip.start, end, next
                    ));
                    running = next;
                    stage += 1;
                    }

                    if need_audio && clip.kind == ClipKind::Video && clip.has_audio && clip.volume > 1e-4 {
                        let label = format!("a{}", stage);
                        fc.push_str(&audio_chain(clip, idx, &label));
                        audio_labels.push(label);
                        stage += 1;
                    }
                }
                ClipKind::Audio => {
                    // A muted clip contributes nothing; skip its input entirely.
                    if !need_audio || clip.volume <= 1e-4 {
                        continue;
                    }
                    let idx = ctx.add_media_input(clip);
                    let label = format!("a{}", stage);
                    fc.push_str(&audio_chain(clip, idx, &label));
                    audio_labels.push(label);
                    stage += 1;
                }
                ClipKind::Text => {
                    if !need_video {
                        continue;
                    }
                    let ff = font_file(&clip.font_family, clip.bold, clip.italic);
                    let color = hex_to_ffcolor(&clip.color, clip.opacity);
                    let x_px = (clip.x * w as f32).round() as i64;
                    let y_px = (clip.y * h as f32).round() as i64;
                    let box_part = match &clip.bg_color {
                        Some(bg) => format!(":box=1:boxcolor={}:boxborderw=10", hex_to_ffcolor(bg, 0.6)),
                        None => String::new(),
                    };
                    let next = format!("v{}", stage);
                    fc.push_str(&format!(
                        "[{}]drawtext=fontfile='{}':text='{}':fontsize={}:fontcolor={}:x={}-text_w/2:y={}-text_h/2:enable='between(t,{:.3},{:.3})'{}[{}];",
                        running,
                        ff,
                        esc_drawtext(&clip.text),
                        clip.font_size as i64,
                        color,
                        x_px,
                        y_px,
                        clip.start,
                        end,
                        box_part,
                        next
                    ));
                    running = next;
                    stage += 1;
                }
            }
        }
    }

    let final_video = need_video.then(|| {
        let label = format!("outv{}", stage);
        fc.push_str(&format!("[{}]format=yuv420p[{}];", running, label));
        label
    });

    let final_audio = need_audio.then(|| {
        let label = "outa".to_string();
        if audio_labels.is_empty() {
            fc.push_str(&format!("anullsrc=r=44100:cl=stereo:d={:.3}[{}];", total, label));
        } else {
            let refs: String = audio_labels.iter().map(|l| format!("[{}]", l)).collect();
            let n = audio_labels.len();
            fc.push_str(&format!(
                "{}amix=inputs={}:duration=longest:dropout_transition=0,volume={}[{}];",
                refs, n, n, label
            ));
        }
        label
    });

    (ctx, fc, final_video, final_audio)
}

// -------------------------------------------------------------------- exports

pub fn export_video(
    tracks: &[Vec<ExportClip>],
    w: u32,
    h: u32,
    fps: u32,
    out_path: &str,
    progress: impl Fn(f32),
) -> Result<(), String> {
    let bin = ffmpeg_bin()?;
    let total = total_duration(tracks);
    let (mut ctx, fc, video, audio) = build_graph(w, h, Some(fps), tracks, total, true, true);
    let video = video.expect("video branch always built for video export");

    ctx.push(&["-filter_complex", &fc]);
    ctx.push(&["-map", &format!("[{}]", video)]);
    ctx.push(&["-map", &format!("[{}]", audio.expect("audio branch always built for video export"))]);
    ctx.push(&["-r", &fps.to_string()]);
    ctx.push(&["-c:v", "libx264", "-preset", "veryfast", "-crf", "18"]);
    ctx.push(&["-c:a", "aac", "-b:a", "192k"]);
    ctx.push(&["-movflags", "+faststart", "-y", out_path]);

    run_with_progress(bin, &ctx.args, total, progress)
}

pub fn export_frame(tracks: &[Vec<ExportClip>], w: u32, h: u32, out_path: &str) -> Result<(), String> {
    let bin = ffmpeg_bin()?;
    let (mut ctx, fc, video, _) = build_graph(w, h, None, tracks, 1.0, false, true);
    let video = video.expect("video branch always built for frame export");
    ctx.push(&["-filter_complex", &fc]);
    ctx.push(&["-map", &format!("[{}]", video)]);
    ctx.push(&["-frames:v", "1", "-y", out_path]);

    let output = command(bin)
        .args(&ctx.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    match output {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) if o.stderr.is_empty() => Err("ffmpeg crashed while rendering the frame. Try a different frame.".into()),
        Ok(o) => Err(tail_error(&o.stderr)),
        Err(e) => Err(e.to_string()),
    }
}

/// Renders the whole timeline's audio to stereo f32 PCM on stdout, starting at
/// `from`. Playback streams this rather than decoding clips separately, which
/// keeps mixing, volume and fades identical to what the export produces.
pub fn spawn_audio_mix(tracks: &[Vec<ExportClip>], from: f32, rate: u32) -> Result<std::process::Child, String> {
    let bin = ffmpeg_bin()?;
    let total = total_duration(tracks);
    let (mut ctx, fc, _video, audio) = build_graph(1, 1, None, tracks, total, true, false);
    let audio = audio.ok_or("no audio branch")?;

    ctx.push(&["-filter_complex", &fc]);
    ctx.push(&["-map", &format!("[{}]", audio)]);
    ctx.push(&["-vn", "-ss", &format!("{:.3}", from.max(0.0))]);
    ctx.push(&["-f", "f32le", "-acodec", "pcm_f32le", "-ac", "2", "-ar", &rate.to_string(), "-"]);

    command(bin)
        .args(&ctx.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())
}

fn tail_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let tail: Vec<&str> = text.lines().rev().take(6).collect();
    tail.into_iter().rev().collect::<Vec<_>>().join("\n")
}

fn run_with_progress(bin: &str, args: &[String], total: f32, progress: impl Fn(f32)) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Read};

    let mut full = args.to_vec();
    full.extend(["-progress".to_string(), "pipe:1".to_string(), "-nostats".to_string()]);

    let mut child = command(bin)
        .args(&full)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("no stdout")?;

    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(val) = line.strip_prefix("out_time_ms=") {
            if let Ok(us) = val.trim().parse::<f32>() {
                progress(if total > 0.0 { (us / 1e6 / total).clamp(0.0, 1.0) } else { 0.0 });
            }
        }
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        progress(1.0);
        return Ok(());
    }
    let mut buf = Vec::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_end(&mut buf);
    }
    Err(format!("ffmpeg exited with an error:\n{}", tail_error(&buf)))
}
