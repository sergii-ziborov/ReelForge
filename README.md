# ReelForge

**Programmatic video editing for Rust.**

Cut, concatenate, overlay, title, composite, and render media from code — a fluent clip graph with native performance. Built for automation, batch pipelines, services, and agent tooling.

## Features

- **Clip graph** — timed video and audio sources, subclips, concat
- **Effects** — crop, resize, rotate (90° steps or free angle), mirror, fades, color, loop, freeze, margin, speed, reverse
- **Compositing** — layers, position anchors, opacity, masks, cross-fades
- **Text & subtitles** — bitmap or TrueType titles; SRT parse and burn-in layers
- **I/O** — open/write media via host **ffmpeg** / **ffprobe**; `write_video` / `write_av` (audio mux); filtergraph fast path for simple transforms
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

Frame-graph microbenchmarks on the same host (sample a transformed frame; not full encode). Parallel row paths + fixed-point color ops:

| Workload | ReelForge | MoviePy 2.x | Speedup |
|----------|-----------|-------------|---------|
| 720p crop→resize→fade→B&W `frame_at` | **~2.1–2.4 ms** | ~8.3 ms | **~3.5–4×** |
| Two-layer composite (640×360) | **~5.7–6.3 ms** | ~30 ms | **~5×** |
| 1080p solid color `frame_at` | **~26 ns** (shared buffer) | ~10 µs | **~380×** |

ReelForge: `cargo bench -p reelforge-fx --bench frame_ops`. MoviePy: `python scripts/bench_moviepy_frames.py`. Numbers vary by machine; re-run locally.

Simple file transforms can stay in FFmpeg via `run_filtergraph` / `reelforge cut` without importing every frame into the process.

```rust
// Free-angle rotate (canvas size unchanged)
let spun = Rotate::degrees(33.0).apply(clip)?;

// Video + audio mux
write_av(&video, &audio, &WriteVideoOptions::new("out.mp4", 24.0))?;
```

## License

MIT. See [LICENSE](LICENSE).
