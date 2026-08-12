# ReelForge

**Programmatic video editing for Rust.**

Cut, concatenate, overlay, title, composite, and render media from code — a fluent clip graph with native performance. Built for automation, batch pipelines, services, and agent tooling.

## Features

- **Clip graph** — timed video and audio sources, subclips, concat
- **Effects** — full geometry / time / color suite; **bicubic** resize; **gaussian head-blur** with soft feather; **painting** (edge-enhance + ink)
- **Audio FX** — gain, stereo L/R, fades, normalize, delay
- **Resolutions** — `Size` presets through **8K** (`UHD_4K`, `UHD_8K`, …)
- **Compositing** — layers, anchors, opacity, masks, cross-fades
- **Text & subtitles** — bitmap or TrueType; **SRT / WebVTT / ASS** parse and burn-in
- **I/O** — host **ffmpeg** / **ffprobe** (no libav link); `write_video` / `write_av` / `write_gif`; **NVENC / QSV / AMF** helpers + extra ffmpeg args; sequential file decode; filtergraph
- **Quality metrics** — `psnr_rgb` / `ssim_rgb` for frame regression checks
- **CLI** — `version`, `probe`, `cut`, `filter`

## Install

```toml
[dependencies]
reelforge = "0.1"
```

```bash
cargo add reelforge
cargo install reelforge-cli   # binary: reelforge
```

Crates: [reelforge](https://crates.io/crates/reelforge) · [docs.rs](https://docs.rs/reelforge)  
Rust toolchain: see `rust-toolchain.toml` (MSRV **1.97**). Media tools: **ffmpeg** and **ffprobe** on `PATH` (or `REELFORGE_FFMPEG` / `REELFORGE_FFPROBE`).

## Documentation

| Doc | Contents |
|-----|----------|
| [docs/GUIDE.md](docs/GUIDE.md) | Concepts, open/write, composition |
| [docs/EFFECTS.md](docs/EFFECTS.md) | Effect catalog and quality tips |
| [docs/IO.md](docs/IO.md) | FFmpeg discovery, sequential decode, GIF/AV |

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
// Quality resize + stylized look + soft tracking blur
let hq = Resize::to_bicubic(Size::HD_1080).apply(clip)?;
let paint = Painting::new().inky().apply(hq)?;
let blur = HeadBlur::fixed(960.0, 400.0, 48.0)
    .with_feather(0.4)
    .apply(paint)?;

write_av(&video, &audio, &WriteVideoOptions::new("out.mp4", 24.0))?;
write_gif(&blur, &WriteGifOptions::new("out.gif", 12.0))?;
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

Frame-graph microbenchmarks on a development host (sample a transformed frame; not full encode):

| Workload | Typical time |
|----------|----------------|
| 720p crop→resize→fade→B&W `frame_at` | ~0.8–2 ms (nearest / bilinear) |
| Two-layer composite (640×360) | ~2–3 ms |
| 1080p / 4K / 8K solid `frame_at` | ~30–60 ns (shared buffer) |
| 4K crop→1080 chain | ~25–40 ms |
| 8K crop→1080 chain | ~40–60 ms |

UHD is first-class. RGB8 4K ≈ 25 MiB/frame, 8K ≈ 95 MiB/frame — limited by RAM and host FFmpeg.

```bash
cargo bench -p reelforge-fx --bench frame_ops
```

Simple file transforms can stay in FFmpeg via `run_filtergraph` / CLI `cut` without importing every frame into the process.

## Dependencies

In-process crates: `rayon`, `image`, `fontdue`, `serde`/`serde_json`, `thiserror`, `clap` (CLI).  
Encode/decode: **host ffmpeg** only (no libav link-time dependency).

## License

MIT. See [LICENSE](LICENSE).
