# ReelForge user guide

Programmatic video and audio editing for Rust: a lazy **clip graph** that samples frames and PCM on demand, then encodes with host **ffmpeg**.

See also: [EFFECTS.md](EFFECTS.md) · [IO.md](IO.md) · [README](../README.md)

---

## Concepts

| Term | Meaning |
|------|---------|
| `VideoClip` | Timed source: `frame_at(t)` → RGB/RGBA frame |
| `AudioClip` | Timed source: `samples_at(t, n)` → interleaved f32 PCM |
| `VideoEffect` / `AudioEffect` | Pure transforms: `apply(clip) → Arc<dyn …>` |
| `CompositeVideo` | Layered canvas: position, opacity, masks, z-order |
| Lazy graph | Nothing is rendered until `frame_at` / write |
| `CachedVideo` / `cache_video` | LRU frame cache for warm / realtime `frame_at` |
| `FrameStream` / `stream_video` | Sequential stream + optional prefetch |

### Cache & streams (realtime)

```rust
use reelforge::prelude::*;
use std::sync::Arc;

let clip: Arc<dyn VideoClip> = Arc::new(open_video(&OpenVideoOptions::new("in.mp4"))?);
let fx = BlackAndWhite.apply(clip)?;
// ~2 seconds of frames at clip fps (LRU)
let hot = cache_video_realtime(fx, 2.0);

// Warm path: second call is a cache hit (Arc frame clone)
let _ = hot.frame_at(Time::from_secs(1.0))?;
let _ = hot.frame_at(Time::from_secs(1.0))?;

// Sequential stream with prefetch window
let mut stream = stream_video(Arc::clone(&hot), 1.0)?;
while let Some((idx, t, frame)) = stream.next_frame()? {
    // realtime consumer…
    let _ = (idx, t, frame);
}
```

Clips are cheap to wrap in `Arc`. Effects return **new** graphs; the original is unchanged.

### Mental model

```text
sources ──► effects / subclips ──► composite ──► write_* / frame_at
                ▲
         Arc-shared nodes
```

---

## Install

```toml
[dependencies]
reelforge = "0.1"
```

```bash
cargo add reelforge
cargo install reelforge-cli
```

| Need | Detail |
|------|--------|
| Rust | **1.97+** (see `rust-toolchain.toml`) |
| ffmpeg / ffprobe | On `PATH`, or set env vars below |

```text
REELFORGE_FFMPEG=/path/to/ffmpeg
REELFORGE_FFPROBE=/path/to/ffprobe
```

```rust
use reelforge::ffmpeg_available;
assert!(ffmpeg_available());
```

---

## Open media

```rust
use reelforge::prelude::*;

let video = open_video(&OpenVideoOptions::new("in.mp4"))?;
// Optional attached audio when the container has a track:
let audio = video.audio();

let only_video = open_video(&OpenVideoOptions::new("in.mp4").video_only())?;
let wav = open_audio(&OpenAudioOptions::new("music.wav"))?;
let still = ImageClip::from_path("poster.png", Duration::from_secs(3.0))?;
```

### Sequential decode

`VideoFileClip` keeps a long-lived raw RGB pipe for **forward** frame indices (encode loops). Seeking backward restarts the pipe; failures fall back to single-frame extract.

---

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

### Compose tips

- Higher `layer_index` draws on top.
- `Position::center()`, `Position::absolute(x, y)`, or `Position::anchored(Anchor::…, ox, oy)`.
- Opacity in `0.0..=1.0`; optional clip `mask_at` for soft edges / chroma keys.

---

## Write outputs

```rust
// Software H.264
write_video(&*titled, &WriteVideoOptions::new("out.mp4", 24.0).with_crf(20))?;

// Hardware encode (ffmpeg build + GPU required)
write_video(&*titled, &WriteVideoOptions::new("out_nv.mp4", 24.0).with_nvenc(23))?;
// also: .with_nvenc_hevc(23)  .with_qsv(23)  .with_amf(23)
// or:  .with_extra_args(["-preset", "p5", "-cq", "20"]).without_crf()

// Video + audio mux (default AAC)
if let Some(a) = audio {
    write_av(&*titled, a, &WriteVideoOptions::new("out_av.mp4", 24.0))?;
}

// Animated GIF
write_gif(
    &*titled,
    &WriteGifOptions::new("out.gif", 12.0).with_duration(Duration::from_secs(3.0)),
)?;
```

Odd frame sizes are floored to even for yuv420 encoders. Expand with `Resize` first if needed.

### Filtergraph (no pixel import)

```rust
use reelforge::{FilterGraph, FilterOp, run_filtergraph};

run_filtergraph(
    "in.mp4",
    "cut.mp4",
    &FilterGraph::new()
        .then(FilterOp::Trim {
            start: 10.0,
            duration: Some(5.0),
        })
        .then(FilterOp::EvenDims),
)?;
```

---

## Subtitles (SRT / WebVTT / ASS)

```rust
let cues = parse_subtitles_path("talk.vtt")?; // .srt / .vtt / .ass / .ssa
// or: parse_subtitles(&string)  // auto-detect
// or: parse_srt / parse_vtt / parse_ass

let layers = burn_in_layers(&cues, &BurnInOptions::default())?;
// composite layers over base video…
```

Markup is stripped for burn-in (VTT tags, ASS `{\…}` overrides). Hard breaks (`\N`) become newlines.

---

## Size presets

```text
Size::HD_720     1280×720
Size::HD_1080    1920×1080
Size::QHD        2560×1440
Size::UHD_4K     3840×2160
Size::DCI_4K     4096×2160
Size::UHD_8K     7680×4320
```

RGB8 memory (approx.): 1080p ≈ 6 MiB/frame · 4K ≈ 25 MiB · 8K ≈ 95 MiB.

---

## Quality metrics

```rust
use reelforge::{psnr_rgb, ssim_rgb};

let p = psnr_rgb(&frame_a, &frame_b)?; // dB; ∞ if identical
let s = ssim_rgb(&frame_a, &frame_b)?; // ~0..=1 (global, per-channel mean)
```

Useful for regression tests and A/B of effect settings.

---

## CLI

```bash
reelforge version
reelforge probe input.mp4
reelforge cut --start 10 --duration 5 in.mp4 out.mp4
reelforge filter --hflip in.mp4 out.mp4
```

From a workspace checkout:

```bash
cargo run -p reelforge-cli -- probe input.mp4
```

---

## Performance notes

1. Prefer **filtergraph** for pure container transforms.
2. Prefer sequential file access when encoding (default on `VideoFileClip`).
3. Resize: `Nearest` (fast) · `Bilinear` (default) · `Bicubic` (sharpest).
4. Solid `ColorClip` frames are Arc-shared (~ns to sample).
5. Benches: `cargo bench -p reelforge-fx --bench frame_ops`.

---

## Prelude

```rust
use reelforge::prelude::*;
```

Imports the common clip, effect, compose, I/O, and text types used in the examples above.
