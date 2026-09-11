//! Playback audio. Rather than decoding and mixing clips ourselves, we hand the
//! whole timeline to the same ffmpeg graph the exporter uses and play its PCM
//! output. Volume, fades and mixing are then identical to the exported file, and
//! our resident cost is one small ring buffer.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Ring capacity in seconds. Big enough to ride out a scheduling hiccup, small
/// enough that a seek is heard immediately.
const BUFFER_SECS: f32 = 0.4;

struct Shared {
    ring: Mutex<VecDeque<f32>>,
    channels: usize,
}

pub struct AudioEngine {
    stream: Option<cpal::Stream>,
    shared: Arc<Shared>,
    pub sample_rate: u32,
    child: Option<std::process::Child>,
    reader_stop: Arc<AtomicBool>,
    reader: Option<std::thread::JoinHandle<()>>,
    capacity: usize,
}

impl AudioEngine {
    pub fn new() -> AudioEngine {
        let host = cpal::default_host();
        let device = host.default_output_device();
        let config = device.as_ref().and_then(|d| d.default_output_config().ok());

        let (sample_rate, channels) = config
            .as_ref()
            .map(|c| (c.sample_rate().0, c.channels() as usize))
            .unwrap_or((48_000, 2));
        let shared = Arc::new(Shared { ring: Mutex::new(VecDeque::new()), channels });
        let capacity = (sample_rate as f32 * BUFFER_SECS) as usize * channels;

        let stream = match (device, config) {
            (Some(device), Some(config)) if config.sample_format() == cpal::SampleFormat::F32 => {
                let s = shared.clone();
                device
                    .build_output_stream(
                        &config.into(),
                        move |out: &mut [f32], _| {
                            let mut ring = s.ring.lock().unwrap();
                            for sample in out.iter_mut() {
                                *sample = ring.pop_front().unwrap_or(0.0);
                            }
                        },
                        |err| eprintln!("audio stream error: {err}"),
                        None,
                    )
                    .ok()
            }
            _ => None,
        };
        if let Some(s) = &stream {
            let _ = s.play();
        }

        AudioEngine {
            stream,
            shared,
            sample_rate,
            child: None,
            reader_stop: Arc::new(AtomicBool::new(false)),
            reader: None,
            capacity,
        }
    }

    pub fn is_available(&self) -> bool {
        self.stream.is_some()
    }

    /// Starts (or restarts) playback of the timeline's audio from `from`.
    pub fn start(&mut self, tracks: &[Vec<crate::ffmpeg::ExportClip>], from: f32) {
        self.stop();
        if self.stream.is_none() {
            return;
        }
        let Ok(mut child) = crate::ffmpeg::spawn_audio_mix(tracks, from, self.sample_rate) else { return };
        let Some(mut stdout) = child.stdout.take() else { return };

        let stop = Arc::new(AtomicBool::new(false));
        self.reader_stop = stop.clone();
        let shared = self.shared.clone();
        let capacity = self.capacity;

        self.reader = std::thread::Builder::new()
            .name("mw-audio".into())
            .spawn(move || {
                // ffmpeg emits stereo f32; fan it out to whatever the device wants.
                let mut buf = vec![0u8; 8192];
                let mut carry: Vec<u8> = Vec::with_capacity(8196);
                while !stop.load(Ordering::Relaxed) {
                    let Ok(n) = stdout.read(&mut buf) else { break };
                    if n == 0 {
                        break;
                    }
                    carry.extend_from_slice(&buf[..n]);
                    let usable = carry.len() - carry.len() % 8;
                    {
                        let mut ring = shared.ring.lock().unwrap();
                        for pair in carry[..usable].chunks_exact(8) {
                            let l = f32::from_le_bytes([pair[0], pair[1], pair[2], pair[3]]);
                            let r = f32::from_le_bytes([pair[4], pair[5], pair[6], pair[7]]);
                            match shared.channels {
                                1 => ring.push_back((l + r) * 0.5),
                                2 => {
                                    ring.push_back(l);
                                    ring.push_back(r);
                                }
                                n => {
                                    ring.push_back(l);
                                    ring.push_back(r);
                                    for _ in 2..n {
                                        ring.push_back(0.0);
                                    }
                                }
                            }
                        }
                    }
                    carry.drain(..usable);

                    // Back off while the ring is full so decoding stays just
                    // ahead of the speaker instead of racing to the end.
                    while !stop.load(Ordering::Relaxed) && shared.ring.lock().unwrap().len() >= capacity {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                }
            })
            .ok();

        self.child = Some(child);
    }

    pub fn stop(&mut self) {
        self.reader_stop.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
        if let Ok(mut ring) = self.shared.ring.lock() {
            ring.clear();
            ring.shrink_to_fit();
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop();
    }
}
