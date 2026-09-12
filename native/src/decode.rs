//! Media decoding. Everything goes through ffmpeg pipes rather than an
//! in-process codec library: the binary is already a hard dependency for export,
//! and piping raw frames keeps our own resident memory to the few frames we
//! actually hold.

use crate::ffmpeg::command;
use std::io::Read;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Mutex,
};

/// A decoded RGBA frame at proxy resolution.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    pub fn blank(width: u32, height: u32) -> Frame {
        Frame { width, height, rgba: vec![0; (width * height * 4) as usize] }
    }
}

fn read_exact_frame(stdout: &mut impl Read, len: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        match stdout.read(&mut buf[filled..]) {
            Ok(0) => return None,
            Ok(n) => filled += n,
            Err(_) => return None,
        }
    }
    Some(buf)
}

/// Decodes exactly one frame. Used for still images, thumbnails, and any time a
/// streaming decoder has not caught up yet.
pub fn decode_one(path: &str, at: f32, w: u32, h: u32) -> Option<Frame> {
    let bin = crate::ffmpeg::ffmpeg_bin().ok()?;
    let mut args: Vec<String> = vec!["-v".into(), "error".into()];
    // Input-side seeking to exactly 0 is a no-op for a real video, but some
    // still images (certain Exif/JFIF JPEGs, seen from Pexels downloads) have
    // an `image2`/mjpeg demuxer that yields zero frames when *any* `-ss` is
    // given before `-i`, even 0.000 — decoding from the start works fine, so
    // only seek when actually skipping ahead.
    if at > 0.0 {
        args.push("-ss".into());
        args.push(format!("{:.3}", at));
    }
    args.push("-i".into());
    args.push(path.into());
    args.extend([
        "-frames:v".into(),
        "1".into(),
        "-vf".into(),
        format!("scale={}:{}:flags=bilinear", w, h),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "-".into(),
    ]);
    let mut child = command(bin)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let mut stdout = child.stdout.take()?;
    let frame = read_exact_frame(&mut stdout, (w * h * 4) as usize);
    let _ = child.wait();
    frame.map(|rgba| Frame { width: w, height: h, rgba })
}

/// A streaming decoder for one clip, backed by a worker thread and a two-frame
/// hand-off. Seeking restarts the underlying ffmpeg process; the caller keeps
/// showing the previous frame until a new one lands, so scrubbing never flashes
/// black.
pub struct ClipDecoder {
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// Wall-clock generation, bumped on every seek so stale frames are dropped.
    generation: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    rx: Receiver<(u32, f32, Frame)>,
    /// Newest frame received, with the source time it represents.
    pub current: Option<(f32, Frame)>,
    /// Generation `current` was delivered under, so a stale frame left over
    /// from before the latest seek can be told apart from a fresh one.
    current_gen: u32,
    stream_start: f32,
    stream_gen: u32,
    /// When the current seek was issued, so a seek still waiting on its first
    /// frame is judged by real elapsed time rather than by how far the live
    /// playhead has since moved — see the comment in `frame_at`.
    seek_issued_at: std::time::Instant,
    fps: f32,
    worker: Option<std::thread::JoinHandle<()>>,
    pending: Arc<Mutex<Option<f32>>>,
    /// The time asked for last call, so a playhead that has stopped moving can
    /// be told apart from one that is running.
    last_request: f32,
    /// True once ffmpeg has been stopped because nothing is asking for new
    /// frames. See `park` for why this matters so much.
    parked: bool,
    park: Arc<AtomicBool>,
    pub last_touched: std::time::Instant,
    /// When the requested time first stopped moving, so parking waits out
    /// `PARK_GRACE_S` instead of killing ffmpeg the instant playback pauses.
    /// A pause-glance-resume inside that window costs nothing; only a pause
    /// that actually sticks around pays for the UI-starvation guard.
    idle_since: Option<std::time::Instant>,
    /// One frame pulled from `rx` but not yet consumed, because it landed
    /// further ahead of the request than we want to display yet. Holding it
    /// here (instead of either discarding it or forcing it into `current`)
    /// is what throttles the decoder now that ffmpeg itself runs at full
    /// speed: see the comment on `frame_at`'s drain loop.
    lookahead: Option<(u32, f32, Frame)>,
}

/// How long the requested time has to sit still before the decoder actually
/// parks (kills ffmpeg). Resuming a parked decoder always re-seeks — cheap
/// most of the time, but occasionally a real, visible catch-up delay (see
/// `motionweight-seek-timeout-retry`) — so a pause shorter than this, the
/// common "glance at a frame, hit play again" case, skips paying that cost
/// at all instead of paying it on every single pause.
const PARK_GRACE_S: f32 = 0.6;

/// How long a seek is allowed to run before producing even one frame, in real
/// wall-clock seconds, before it is judged stuck and retried. Generous: a slow
/// decode legitimately reaching a distant, sparsely-keyframed target should
/// finish inside this window rather than being cancelled mid-flight.
const SEEK_TIMEOUT_S: f32 = 6.0;

/// How large a jump in the requested time has to be, between one call and
/// the next, to be treated as an explicit seek rather than ordinary playback
/// advancing frame by frame (which, even under load, rarely steps `t` by
/// more than a fraction of this in one call).
const JUMP_THRESHOLD_S: f32 = 0.4;

impl ClipDecoder {
    pub fn new(path: &str, width: u32, height: u32, fps: f32, start_at: f32) -> ClipDecoder {
        let (tx, rx) = std::sync::mpsc::sync_channel::<(u32, f32, Frame)>(2);
        let generation = Arc::new(AtomicU32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let park = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(Mutex::new(Some(start_at)));

        let worker = {
            let (path, gen, stop, pending, park) =
                (path.to_string(), generation.clone(), stop.clone(), pending.clone(), park.clone());
            std::thread::Builder::new()
                .name("mw-decode".into())
                .spawn(move || decode_loop(path, width, height, fps, gen, stop, park, pending, tx))
                .ok()
        };

        ClipDecoder {
            path: path.to_string(),
            width,
            height,
            generation,
            stop,
            rx,
            current: None,
            current_gen: 0,
            stream_start: start_at,
            stream_gen: 0,
            seek_issued_at: std::time::Instant::now(),
            fps,
            worker,
            pending,
            last_request: f32::NEG_INFINITY,
            parked: false,
            park,
            last_touched: std::time::Instant::now(),
            idle_since: None,
            lookahead: None,
        }
    }

    /// Asks for the frame at source time `t`, returning whatever is best
    /// available right now. Never blocks.
    pub fn frame_at(&mut self, t: f32) -> Option<&Frame> {
        self.last_touched = std::time::Instant::now();

        // A parked decoder has no ffmpeg behind it, so the moment the playhead
        // moves again it has to be restarted from the new position rather than
        // waited on.
        let delta = t - self.last_request;
        let moved = delta.abs() > 1e-4;
        // A real scrub — dragging or clicking the ruler to a new spot — moves
        // `t` by far more between calls than ordinary playback advancing
        // frame by frame ever does. That's the signal an explicit seek was
        // requested, in *either* direction, and it always deserves an
        // immediate restart at the new target: without this, a forward scrub
        // was indistinguishable from decode simply falling behind (which,
        // below, is deliberately left to catch up on its own rather than
        // re-seeking) and so was never actually honored — the preview kept
        // crawling forward from wherever it already was instead of jumping
        // to the new position, reading as "not playing from there".
        let jumped = delta.abs() > JUMP_THRESHOLD_S;
        self.last_request = t;
        if moved {
            self.idle_since = None;
        }
        if moved && self.parked {
            self.parked = false;
            self.park.store(false, Ordering::SeqCst);
            self.seek(t);
        } else if jumped {
            self.seek(t);
        }

        // Drain what the worker has produced, keeping the newest frame that is
        // not ahead of the request. `self.current` is never cleared on seek
        // (the old frame stays on screen until a new one lands), so its
        // timestamp can belong to a completely different stream generation —
        // comparing a fresh stream's timestamps against it is only valid once
        // that fresh stream is the one `current` was last set from. Otherwise
        // a backward seek (or any seek landing before wherever the stale
        // frame's timestamp was) made every incoming frame lose the `ts >
        // *cur` check forever, since `current` never updates to tell them
        // apart from a genuinely-later frame within the *same* stream: the
        // preview would freeze on the pre-seek frame permanently while decode
        // silently produced and discarded correct frames behind the scenes.
        //
        // Unlike a plain `while let Ok(..) = try_recv()`, this stops the
        // instant it meets a frame further ahead of `t` than we want to show
        // yet, and holds that one in `lookahead` instead of consuming it.
        // ffmpeg runs at full decode speed now (no `-re`), so without this the
        // decoder would race through the file discarding everything beyond
        // the display window as fast as it could decode it. Leaving the
        // surplus frame sitting here means it also sits in the bounded
        // channel behind it, which backpressures the worker's `try_send` loop
        // (decode_loop) into sleeping until playback catches up — that
        // channel-fill is the only throttle left, and it paces the decoder to
        // actual consumption instead of to wall-clock time. The old wall-clock
        // pacing (`-re`) was the reason a random seek into a long clip could
        // "stick": ffmpeg paces its own internal keyframe-to-target catch-up
        // the same as normal playback, so landing far from a keyframe meant
        // waiting out that whole gap in real time before the first frame
        // arrived at all. Full-speed decode does that catch-up in a blink.
        loop {
            let item = self.lookahead.take().map(Ok).unwrap_or_else(|| self.rx.try_recv());
            let Ok((gen, ts, frame)) = item else { break };
            if gen != self.stream_gen {
                continue;
            }
            let ahead = match &self.current {
                Some((cur, _)) if self.current_gen == gen => ts > *cur,
                _ => true,
            };
            if !ahead {
                continue;
            }
            if ts <= t + 1.0 / self.fps {
                self.current = Some((ts, frame));
                self.current_gen = gen;
            } else {
                self.lookahead = Some((gen, ts, frame));
                break;
            }
        }

        // Once frames are flowing, a re-seek is only for a genuine jump — a
        // backward scrub, caught here by the live time landing behind the
        // newest decoded frame. Falling *forward* behind schedule is
        // deliberately NOT a re-seek trigger any more: this used to re-seek
        // once the live playhead drifted more than ~1 second ahead of the
        // newest frame, on the assumption that a fresh seek is cheap. It
        // isn't, for a source that decodes slower than the project's frame
        // rate (large frames, sparse keyframes forcing a long
        // decode-from-keyframe before the first output) — reaching even the
        // *first* frame of a catch-up seek can itself take longer than that.
        // Every such re-seek then gets judged "behind" before it produces
        // anything, is abandoned for a still-later (so even more expensive)
        // target, and the gap only grows: a spiral that never converges and
        // looks, from the UI, like a permanent freeze the moment playback
        // continues past a manual seek. Left alone instead, a slow decoder
        // just keeps decoding forward and the preview lags rather than
        // dying — worse sync, but never stuck.
        let fresh = self.current.is_some() && self.current_gen == self.stream_gen;
        // A timed-out seek retries at its *own* original target, not at `t`.
        // While playing, `t` is the live, ever-advancing playhead; if the
        // seek simply hasn't produced a frame yet because that position is
        // slow to reach (a deep GOP, a cold read), retrying against `t`
        // instead chases a moving target that has only gotten harder to
        // reach in the meantime — the exact divergent spiral described
        // below ("the gap only grows"), just entered here (on resuming
        // playback after the decoder parked, which always re-seeks) rather
        // than through the "falling behind" path that was removed for that
        // reason. Retrying the same, fixed target converges instead, or at
        // worst keeps failing no worse than before.
        let seek_target = if fresh {
            let (cur, _) = self.current.as_ref().unwrap();
            (t < *cur - 0.05).then_some(t)
        } else if t < self.stream_start - 0.05 {
            // Nothing has landed yet for this seek, and a jump backward past
            // its own target is still a real, unambiguous re-seek signal.
            Some(t)
        } else if self.seek_issued_at.elapsed().as_secs_f32() > SEEK_TIMEOUT_S {
            Some(self.stream_start)
        } else {
            None
        };
        if let Some(target) = seek_target {
            if !self.parked {
                self.seek(target);
            }
        }

        // Nothing is asking for new frames and we already have the one being
        // shown, so stop ffmpeg. Draining the pipe every frame the way this
        // function does means the decoder never blocks on backpressure: left
        // running it will race through the whole rest of the file at its own
        // pace, holding two cores and hundreds of megabytes away from the
        // interface for as long as the project stays open. That contention is
        // felt everywhere — sliders, typing, scrolling — because it slows down
        // every frame the UI draws, not just the preview.
        if !moved && !self.parked && self.current_gen == self.stream_gen {
            if let Some((ts, _)) = &self.current {
                if *ts >= t - 1.0 / self.fps.max(1.0) {
                    let idle_since = *self.idle_since.get_or_insert_with(std::time::Instant::now);
                    if idle_since.elapsed().as_secs_f32() > PARK_GRACE_S {
                        self.parked = true;
                        self.park.store(true, Ordering::SeqCst);
                    }
                }
            }
        }

        self.current.as_ref().map(|(_, f)| f)
    }

    pub fn seek(&mut self, t: f32) {
        self.parked = false;
        self.park.store(false, Ordering::SeqCst);
        self.stream_gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.stream_start = t;
        self.seek_issued_at = std::time::Instant::now();
        self.lookahead = None;
        *self.pending.lock().unwrap() = Some(t);
    }
}

impl Drop for ClipDecoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.park.store(false, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.pending.lock().unwrap() = Some(0.0);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_loop(
    path: String,
    w: u32,
    h: u32,
    fps: f32,
    generation: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    park: Arc<AtomicBool>,
    pending: Arc<Mutex<Option<f32>>>,
    tx: SyncSender<(u32, f32, Frame)>,
) {
    let Ok(bin) = crate::ffmpeg::ffmpeg_bin() else { return };
    let frame_len = (w * h * 4) as usize;

    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let Some(start) = pending.lock().unwrap().take() else {
            std::thread::sleep(std::time::Duration::from_millis(4));
            continue;
        };
        let my_gen = generation.load(Ordering::SeqCst);

        let child = command(bin)
            .args([
                "-v",
                "error",
                // `-hwaccel auto` was tried here to speed up slow-to-decode
                // sources (10-bit HEVC at 1080p+ can undercut a 60fps
                // project's real-time needs in software). It measured faster
                // in isolation, but this decoder tears down and restarts a
                // brand-new ffmpeg process — and so a brand-new hardware
                // decode session — on every seek, and repeatedly
                // creating/destroying hardware sessions like that is exactly
                // the pattern that trips up flaky GPU drivers: it came back
                // as the video simply never starting after a scrub, worse
                // than the slowness it was meant to fix. Software decode is
                // slower but reliable; that trade wins.
                // No `-re`: ffmpeg decodes as fast as the CPU allows rather
                // than pacing itself to the source's own clock. That used to
                // mean it sprinted the decoder far past the playhead's accept
                // window within milliseconds of every seek, discarding almost
                // every frame as "too far ahead" — but `frame_at`'s drain loop
                // now stops pulling from the channel once a frame lands ahead
                // of the display window and holds it in `lookahead`, which
                // backpressures this process through the channel-full sleep
                // below instead. That keeps steady playback paced to actual
                // consumption, while a seek's internal keyframe-to-target
                // catch-up (which real-time pacing used to throttle right
                // along with everything else) now runs at full speed.
                "-ss",
                &format!("{:.3}", start.max(0.0)),
                "-i",
                &path,
                "-vf",
                &format!("scale={}:{}:flags=bilinear,fps={}", w, h, fps.max(1.0)),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn();
        let Ok(mut child) = child else { return };
        let Some(mut stdout) = child.stdout.take() else { return };

        let mut index = 0u32;
        loop {
            if stop.load(Ordering::SeqCst)
                || park.load(Ordering::SeqCst)
                || generation.load(Ordering::SeqCst) != my_gen
            {
                break;
            }
            let Some(rgba) = read_exact_frame(&mut stdout, frame_len) else { break };
            let ts = start + index as f32 / fps.max(1.0);
            index += 1;
            let mut payload = (my_gen, ts, Frame { width: w, height: h, rgba });
            // Block only in short slices, so a seek during playback is picked up
            // promptly instead of waiting on a full consumer cycle.
            loop {
                match tx.try_send(payload) {
                    Ok(()) => break,
                    Err(TrySendError::Full(p)) => {
                        payload = p;
                        if stop.load(Ordering::SeqCst)
                            || park.load(Ordering::SeqCst)
                            || generation.load(Ordering::SeqCst) != my_gen
                        {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(3));
                    }
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
        }

        let _ = child.kill();
        let _ = child.wait();
    }
}

// ---------------------------------------------------------------- thumbnails

/// Small RGBA thumbnail for the media bin. Images and video share the path;
/// video grabs a frame slightly in so black leaders do not win.
pub fn thumbnail(path: &str, duration: f32, is_image: bool, target_w: u32, aspect: f32) -> Option<Frame> {
    let h = ((target_w as f32 / aspect.max(0.05)).round() as u32).clamp(1, 512);
    let at = if is_image { 0.0 } else { (duration / 2.0).min(0.2) };
    decode_one(path, at, target_w, h)
}

/// Number of peak buckets stored per asset: enough detail for a timeline strip.
/// Enough peaks that a clip stays detailed when the timeline is zoomed in,
/// without bloating what has to be cloned into the undo stack and serialized
/// by every autosave — peaks live in the project file, so this number is paid
/// for on every save and every history snapshot, not just once.
pub const WAVEFORM_BUCKETS: usize = 1500;

/// Decodes an asset's audio and reduces it to normalized peak buckets. Streams
/// through the samples so a long file never lands in memory.
pub fn waveform(path: &str, duration: f32) -> Option<Vec<f32>> {
    let bin = crate::ffmpeg::ffmpeg_bin().ok()?;
    const RATE: u32 = 8000;
    let mut child = command(bin)
        .args([
            "-v", "error", "-i", path, "-vn", "-ac", "1", "-ar", "8000", "-f", "f32le", "-",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;

    let total_samples = (duration.max(0.1) * RATE as f32) as usize;
    let per_bucket = (total_samples / WAVEFORM_BUCKETS).max(1);
    let mut peaks = vec![0.0f32; WAVEFORM_BUCKETS];
    let mut buf = vec![0u8; 16384];
    let mut sample_index = 0usize;
    // ffmpeg's pipe writes do not respect sample boundaries, so a partial
    // f32 at the end of one read is carried into the next.
    let mut carry: Vec<u8> = Vec::with_capacity(16388);

    loop {
        let Ok(n) = stdout.read(&mut buf) else { break };
        if n == 0 {
            break;
        }
        carry.extend_from_slice(&buf[..n]);
        let usable = carry.len() - carry.len() % 4;
        accumulate(&carry[..usable], &mut peaks, &mut sample_index, per_bucket);
        carry.drain(..usable);
    }

    let _ = child.kill();
    let _ = child.wait();

    let max = peaks.iter().cloned().fold(0.0f32, f32::max);
    if max <= 0.0 {
        return None;
    }
    Some(peaks.into_iter().map(|p| (p / max * 100.0).round() / 100.0).collect())
}

fn accumulate(bytes: &[u8], peaks: &mut [f32], sample_index: &mut usize, per_bucket: usize) {
    for s in bytes.chunks_exact(4) {
        let v = f32::from_le_bytes([s[0], s[1], s[2], s[3]]).abs();
        let bucket = (*sample_index / per_bucket).min(peaks.len() - 1);
        if v > peaks[bucket] {
            peaks[bucket] = v;
        }
        *sample_index += 1;
    }
}
