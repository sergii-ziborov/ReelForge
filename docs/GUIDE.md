# ReelForge user guide

Programmatic video and audio editing for Rust: a lazy **clip graph** that samples frames and PCM on demand, then encodes with host **ffmpeg**.

## Concepts

| Term | Meaning |
|------|---------|
| `VideoClip` | Timed source: `frame_at(t)` → RGB/RGBA frame |
| `AudioClip` | Timed source: `samples_at(t, n)` → PCM f32 |
| `VideoEffect` / `AudioEffect` | Pure transforms: `apply(clip) → new clip` |
| `CompositeVideo` | Layered canvas with position, opacity, masks |
| Lazy graph | Nothing is rendered until `frame_at` / write |

Clips are cheap to wrap in `Arc`. Effects return new graphs; the original is unchanged.

## Install

```toml
[dependencies]
reelforge = "0.1"
```

Requires a Rust toolchain (see `rust-toolchain.toml`) and **ffmpeg** / **ffprobe** on `PATH`, or:

```text
REELFORGE_FFMPEG=C:\path\to\ffmpeg.exe
REELFORGE_FFPROBE=C:\path\to\ffprobe.exe
```

## Open media

```rust
use reelforge::prelude::*;

let video = open_video(&OpenVideoOptions::new("in.mp4"))?;
// Optional attached audio when the container has a track:
let audio = video.audio();

let only_video = open_video(&OpenVideoOptions::new("in.mp4").video_only())?;
let wav = open_audio(&OpenAudioOptions::new("music.wav"))?;
```

`VideoFileClip` uses a **sequential decode pipe** for ordered access (typical encode path) and falls back to single-frame seeks when needed.

## Build a graph

```rust
use std::sync::Arc;
use reelforge::prelude::*;

let base: Arc<dyn VideoClip> = Arc::new(open_video(&OpenVideoOptions::new("in.mp4"))?);
let cut = subclip_video(base, Time::from_secs(2.0), Duration::from_secs(5.0))?;
let sized = Resize::to_bicubic(Size::HD_1080).apply(cut)?;
let faded = FadeIn::new(Duration::from_secs(0.5)).apply(sized)?;
let titled = {
    let t = TextClip::new(&TextClipOptions::new("Hello", 48, faded.duration()))?;
    composite_video(
        faded.size(),
        vec![
            CompositeLayer::new(faded.clone()),
            CompositeLayer::new(Arc::new(t))
                .with_position(Position::center())
                .with_layer_index(1),
        ],
    )?
};
```

## Write outputs

```rust
// H.264 video
write_video(&*titled, &WriteVideoOptions::new("out.mp4", 24.0).with_crf(20))?;

// Video + audio mux (AAC by default)
if let Some(a) = audio {
    write_av(&*titled, a, &WriteVideoOptions::new("out_av.mp4", 24.0))?;
}

// Animated GIF (palettegen / paletteuse)
write_gif(&*titled, &WriteGifOptions::new("out.gif", 12.0)
    .with_duration(Duration::from_secs(3.0)))?;
```

For simple file-only transforms (trim/crop/scale/flip) without pulling frames into process memory, use the **filtergraph** path:

```rust
use reelforge::{FilterGraph, FilterOp, run_filtergraph};

run_filtergraph(
    "in.mp4",
    "cut.mp4",
    &FilterGraph::new()
        .then(FilterOp::Trim { start: 10.0, duration: Some(5.0) })
        .then(FilterOp::EvenDims),
)?;
```

## Size presets

```rust
Size::HD_720    // 1280×720
Size::HD_1080   // 1920×1080
Size::QHD       // 2560×1440
Size::UHD_4K    // 3840×2160
Size::DCI_4K    // 4096×2160
Size::UHD_8K    // 7680×4320
```

RGB8 memory roughly: 1080p ≈ 6 MiB/frame, 4K ≈ 25 MiB, 8K ≈ 95 MiB.

## CLI

```bash
cargo run -p reelforge-cli -- version
cargo run -p reelforge-cli -- probe input.mp4
cargo run -p reelforge-cli -- cut --start 10 --duration 5 in.mp4 out.mp4
cargo run -p reelforge-cli -- filter --hflip in.mp4 out.mp4
```

## Performance notes

- Prefer **filtergraph** for pure container transforms.
- Prefer **sequential** file access when encoding (default on `VideoFileClip`).
- Resize: `Nearest` (fast) · `Bilinear` (default) · `Bicubic` (sharpest).
- Solid `ColorClip` frames are Arc-shared (~ns to sample).

## See also

- [EFFECTS.md](EFFECTS.md) — effect catalog
- [IO.md](IO.md) — open/write options and FFmpeg behavior
