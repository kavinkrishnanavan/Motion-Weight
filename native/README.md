# MotionWeight — native build

A CPU-rendered, single-process rewrite of the editor. Same features, no browser
engine, no GPU driver, no JavaScript.

## Why it is small

| Layer | Choice | Why |
|---|---|---|
| Window + input | `winit` | Thin wrapper over Win32; no toolkit. |
| Presentation | `softbuffer` | A plain DIB the app writes pixels into. No D3D/Vulkan driver is ever loaded. |
| Rasterizer | `tiny-skia` | Pure-Rust 2D. Anti-aliased paths, no GPU. |
| Text | `ab_glyph` over a memory-mapped face | Glyph outlines stay in the file-backed mapping and are rasterized on demand. Parsing faces eagerly cost 37 MB — more than every pixel buffer combined. |
| Widgets | `src/ui` (immediate mode) | ~1,000 lines. Focus is a field, so re-rendering can never steal it mid-word. |
| Media | ffmpeg over pipes | Already required for export; piping raw frames means the only resident frames are the ones on screen. |
| Audio | ffmpeg mix → `cpal` ring buffer | Playback runs the same filter graph the exporter does, so what you hear is what you get. Opened on first play. |

Preview composites at a 960 px proxy width; export always renders at the
project's real resolution.

## Measured (release, 1440x880 window at 1.5x DPI)

| Scenario | Working set | Private |
|---|---|---|
| Home screen | 39.9 MB | 27.1 MB |
| Editor, 4 clips | 40.3 MB | 28.9 MB |
| Playing video + audio | 41.5 MB | 31.7 MB |
| 40 clips, 4 lanes, scrubbed | 43.2 MB | 31.8 MB |

Idle CPU is ~1% of one core: redraws are driven by input, and the event loop
sleeps otherwise.

## Frame cost

The window is repainted whole, so frame cost scales with its area. At
1440x880 @1.5x (2160x1320 device pixels):

| | ms |
|---|---|
| Typical editing frame | 7-11 |
| 40 clips over 4 lanes | ~18 |
| Home screen | ~4 |

Two things dominate and are worth knowing about before optimising anything
else: the preview blit (proxy frame scaled to the stage) and the final copy of
the pixmap into the window, which is memory-bandwidth bound. Build with
`--features harness` and use `shot` to get a per-section breakdown.

## Build

```sh
cargo build --release          # ships: target/release/motionweight.exe (~1.9 MB)
cargo build --release --features harness
```

`ffmpeg` and `ffprobe` must be on `PATH` (or in `C:\ffmpeg\bin`).

Projects live in `%APPDATA%\MotionWeight`.

## Test harness

`--features harness` compiles a scripted-input driver. It exists so the UI can
be exercised without a human, and without taking screenshots: `shot` saves the
app's **own** pixmap, so a capture can only ever contain this window.

```sh
MW_SCRIPT=script.txt ./motionweight.exe
```

One command per redraw:

```
move <x> <y> | down | up | click | dclick | wheel <d>
ctrl 0|1 | shift 0|1 | key <name> | text <s>
wait <frames> | shot <path> | exit
```

`shot` writes `<path>` plus `<path>.txt` holding a one-line state dump (view,
playhead, selection, lanes and their clips) to assert against. With the harness
built, `MW_SAVE_PATH=<dir>` stands in for the export save dialog.
