# ReelForge

**Programmatic video editing for Rust.**

Cut, concatenate, overlay, title, composite, and render media from code — a fluent clip graph with native performance. Built for automation, batch pipelines, services, and agent tooling.

## Features

- **Clip graph** — timed video and audio sources, subclips, concat
- **Effects** — crop, resize (bilinear / nearest), rotate, mirror, fades, cross-fades, color, gamma, lum/contrast, blink, painting, loop, freeze / freeze-region, head-blur, margin, scroll, slide, speed, accel/decel, reverse, time-symmetrize, super-sample, chroma mask, mask and/or
- **Audio FX** — volume gain, stereo L/R gain, fade in/out, peak normalize, delay
- **Resolutions** — any size via `Size`; presets `HD_720`, `HD_1080`, `UHD_4K` (3840×2160), `UHD_8K` (7680×4320)
- **Compositing** — layers, position anchors, opacity, masks, cross-fades
- **Text & subtitles** — bitmap or TrueType titles; SRT parse and burn-in layers
- **I/O** — open/write via host **ffmpeg** / **ffprobe**; `write_video` / `write_av` / `write_gif`; filtergraph path for simple file transforms
- **CLI** — `version`, `probe`, `cut`, `filter`

## Install

```toml
[dependencies]
reelforge = "0.1"
```

Rust toolchain: see `rust-toolchain.toml`. Media encode/decode needs **ffmpeg** and **ffprobe** on `PATH` (or `REELFORGE_FFMPEG` / `REELFORGE_FFPROBE`).

## Quick start

```rust
use reelforge::prelude::*;
use std::sync::Arc;

let clip = ColorClip::new(Size::new(640, 360), Rgb8::BLACK, Duration::from_secs(2.0))
    .with_fps(24.0);
write_video(&clip, &WriteVideoOptions::new("out.mp4", 24.0))?;
```

```rust
let base = Arc::new(ColorClip::new(Size::new(640, 360), Rgb8::BLACK, Duration::from_secs(3.0)));
let title = TextClip::new(&TextClipOptions::new("Hello", 28, Duration::from_secs(3.0)))?;
let out = composite_video(
    Size::new(640, 360),
    vec![
        CompositeLayer::new(base),
        CompositeLayer::new(Arc::new(title))
            .with_position(Position::center())
            .with_layer_index(1),
    ],
)?;
```

```rust
// Free-angle rotate (canvas size unchanged)
let spun = Rotate::degrees(33.0).apply(clip)?;

// Resize: bilinear by default; nearest for speed
let hq = Resize::to(Size::HD_1080).apply(clip.clone())?;
let fast = Resize::to_nearest(Size::HD_720).apply(clip)?;

// Video + audio mux / GIF
write_av(&video, &audio, &WriteVideoOptions::new("out.mp4", 24.0))?;
write_gif(&clip, &WriteGifOptions::new("out.gif", 12.0))?;
```

```bash
cargo run -p reelforge-cli -- version
cargo run -p reelforge-cli -- probe input.mp4
cargo run -p reelforge-cli -- cut --start 10 --duration 5 in.mp4 out.mp4
```

## Crates

| Crate | Role |
|-------|------|
| `reelforge` | Umbrella API |
| `reelforge-core` | Time, frames, audio, clip traits |
| `reelforge-io` | Decode / encode / filtergraph |
| `reelforge-fx` | Video and audio effects |
| `reelforge-compose` | Layers, masks, compositing |
| `reelforge-text` | Titles and subtitles |
| `reelforge-cli` | Command-line tool |

## Performance

Frame-graph microbenchmarks on a development host (sample a transformed frame; not full encode). Parallel row paths + fixed-point color ops:

| Workload | Typical time |
|----------|----------------|
| 720p crop→resize→fade→B&W `frame_at` | ~2–8 ms |
| Two-layer composite (640×360) | ~6–8 ms |
| 1080p / 4K / 8K solid `frame_at` | ~30–60 ns (shared buffer) |
| 4K crop→1080 chain | ~25–40 ms |
| 8K crop→1080 chain | ~40–60 ms |

UHD is first-class (`Size::UHD_4K` / `Size::UHD_8K`): RGB8 4K ≈ 25 MiB/frame, 8K ≈ 95 MiB/frame — limited by RAM and host FFmpeg, not by a hard API cap.

```bash
cargo bench -p reelforge-fx --bench frame_ops
```

Simple file transforms can stay in FFmpeg via `run_filtergraph` / CLI `cut` without importing every frame into the process.

## License

MIT. See [LICENSE](LICENSE).
