# ReelForge

**Programmatic video editing for Rust.**

A fluent **clip graph** for cutting, compositing, titling, effects, and render — built for automation, batch pipelines, services, and agent tooling. Native performance, no GUI required.

[![crates.io](https://img.shields.io/crates/v/reelforge.svg)](https://crates.io/crates/reelforge)
[![docs.rs](https://docs.rs/reelforge/badge.svg)](https://docs.rs/reelforge)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

```toml
[dependencies]
reelforge = "0.1"
```

```bash
cargo add reelforge
cargo install reelforge-cli    # installs the `reelforge` binary
```

**Requirements:** Rust **1.97+** (`rust-toolchain.toml`), and host **ffmpeg** + **ffprobe** on `PATH` (or `REELFORGE_FFMPEG` / `REELFORGE_FFPROBE`). ReelForge does **not** link libav.

---

## Why ReelForge

| | |
|--|--|
| **Lazy graphs** | Wrap clips in `Arc`, apply effects; nothing renders until `frame_at` / write |
| **Frame cache + streams** | LRU `CachedVideo` for warm/realtime hits; `FrameStream` sequential + prefetch |
| **In-process pixels** | Parallel raster paths (rayon), fixed-point color, bicubic resize; timed `VideoSurface` (PTS / YUV planes) |
| **Host FFmpeg** | Decode/encode/mux via CLI — any format your ffmpeg build supports |
| **UHD-ready** | First-class `Size` presets through 8K; memory is the only hard limit |
| **Scripted NLE surface** | Geometry, time, color, audio FX, text, subtitles, compose, GIF |

---

## Features

### Clip graph & compose
- Video / audio sources: files, solid color, silence, still images
- Subclip, concatenate, layer composite (position, opacity, masks, z-order)
- Cross-fades, start offsets, multi-layer titles

### Effects (`reelforge-fx`)
- **Geometry:** crop, resize (**nearest / bilinear / bicubic**), rotate (90° / free angle), mirror, margin, even size, scroll, slide in/out
- **Time:** speed, accel/decel, reverse, time-symmetrize, loop, freeze, freeze-region, super-sample, blink
- **Color / look:** fades, B&W, invert, multiply, gamma, lum/contrast, **painting** (edge-enhance + ink), chroma mask, mask and/or
- **Blur:** **HeadBlur** + **TrackedBlur** (multi-region tracks JSON / SightLoom-compatible samples)
- **Audio:** gain, stereo L/R, fade in/out, peak normalize, delay

### Text & captions
- Bitmap face (portable) or TrueType via `fontdue`
- Subtitles: **SRT**, **WebVTT**, **ASS/SSA** → `burn_in_layers`

### I/O
- `open_video` / `open_audio` / `ImageClip`
- Sequential rawvideo pipe for ordered file access (fast encode path)
- `write_video` / `write_av` / `write_gif` (+ `_with` for progress / cancel / pipeline)
- Streaming PCM for `write_av` (chunked, no full-timeline buffer)
- Bounded frame pipeline (`WriteControl::max_in_flight`) + RGB buffer pool
- Hardware encode helpers: **NVENC / QSV / AMF** + free-form `extra_ffmpeg_args`
- Filtergraph fast path for pure file transforms (trim/crop/scale/flip); source audio is copied
- **RenderPlan** JSON: optimize (fuse/DCE) + FFmpeg prefix extract + **hybrid** run (prefix → Rust remainder → encode)
- **RenderGraph** DAG: typed ops, hybrid `ExecutionPlan`, **TrackTimeline** → `MaskTimeline` view + fused **RegionRedaction**
- **CaptureProject**: OTIO-like sequences/tracks/clips/gaps → `compile_project` → `RenderGraph`
- Optional SightLoom-shaped JSON adapter (`parse_track_timelines`) — no SightLoom crate

### Quality & tooling
- `psnr_rgb` / `ssim_rgb` frame metrics
- **Correctness gate** (synthetic + MP4 decode/transform/encode, CI-failing floors) — see [docs/CORRECTNESS.md](docs/CORRECTNESS.md)
- CLI: `version`, `probe`, `cut`, `filter`, `plan`
- Criterion benches: `cargo bench -p reelforge-fx`, `cargo bench -p reelforge-io --bench render_plan`
- CI: fmt / clippy / test / docs / package dry-run / bench compile (Linux, Windows, macOS) + FFmpeg job

---

## Documentation

| Doc | Contents |
|-----|----------|
| **[docs/GUIDE.md](docs/GUIDE.md)** | Concepts, install, open/write, compose, **RenderGraph**, subtitles, metrics |
| **[docs/EFFECTS.md](docs/EFFECTS.md)** | Full effect catalog + quality tips |
| **[docs/IO.md](docs/IO.md)** | FFmpeg, sequential decode, HW encode, GIF, RenderPlan, RenderGraph I/O |
| **[docs/CORRECTNESS.md](docs/CORRECTNESS.md)** | Public PSNR/SSIM pipeline gates + CI |
| [docs.rs/reelforge](https://docs.rs/reelforge) | Generated API docs |
| [crates.io/reelforge](https://crates.io/crates/reelforge) | Package registry |

---

## Quick start

### Solid color → MP4

```rust
use reelforge::prelude::*;

let clip = ColorClip::new(Size::new(640, 360), Rgb8::BLACK, Duration::from_secs(2.0))
    .with_fps(24.0);
write_video(&clip, &WriteVideoOptions::new("out.mp4", 24.0))?;
```

### Title over base

```rust
use reelforge::prelude::*;
use std::sync::Arc;

let base = Arc::new(ColorClip::new(
    Size::new(640, 360),
    Rgb8::BLACK,
    Duration::from_secs(3.0),
));
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

### File open → effects → write / GIF / HW

```rust
use reelforge::prelude::*;
use std::sync::Arc;

let video = open_video(&OpenVideoOptions::new("in.mp4"))?;
let clip: Arc<dyn VideoClip> = Arc::new(video);
let hq = Resize::to_bicubic(Size::HD_1080).apply(clip)?;
let faded = FadeIn::new(Duration::from_secs(0.4)).apply(hq)?;

// Software x264
write_video(&*faded, &WriteVideoOptions::new("out.mp4", 24.0).with_crf(20))?;

// NVIDIA NVENC (needs ffmpeg + GPU)
write_video(&*faded, &WriteVideoOptions::new("out_nv.mp4", 24.0).with_nvenc(23))?;

// GIF
write_gif(&*faded, &WriteGifOptions::new("out.gif", 12.0))?;
```

### Subtitles burn-in

```rust
let cues = parse_subtitles_path("talk.vtt")?; // also .srt / .ass
let layers = burn_in_layers(&cues, &BurnInOptions::default())?;
// composite `layers` over your base video…
```

### RenderPlan (JSON + FFmpeg extract)

```rust
use reelforge::prelude::*;

let plan = RenderPlan::from_file("in.mp4")
    .then(PlanOp::Trim { start: 1.0, duration: 5.0 })
    .then(PlanOp::HFlip)
    .then(PlanOp::Scale { w: 640, h: 360 })
    .then(PlanOp::EvenDims)
    .with_output(PlanOutput::new("out.mp4"));

let extracted = extract_ffmpeg(&plan);
assert!(extracted.fully_ffmpeg);
// run_render_plan(&plan)?; // host ffmpeg, no Rust pixel import
```

### CLI

```bash
reelforge version
reelforge probe input.mp4
reelforge cut --start 10 --duration 5 in.mp4 out.mp4
reelforge filter --hflip in.mp4 out.mp4
reelforge plan job.json --explain
reelforge plan job.json --run
```

### RenderGraph (DAG → compile → run)

One-shot jobs stay on **RenderPlan**. Multi-node edits (trim + privacy + encode) use **RenderGraph**:

```rust
use reelforge::{
    GraphOutput, MaskSample, MaskTimeline, MediaAsset, MediaAssetId, MediaTime, NodeId,
    OperationId, OperationRegistry, RENDER_GRAPH_VERSION, RegionRedaction, RenderGraph,
    RenderNode, RenderNodeKind, compile_graph, explain_render_graph, run_render_graph,
    schedule_graph,
};

let mut masks = MaskTimeline::new();
masks.push(MaskSample::ellipse(
    MediaTime::new(0, 30)?,
    320.0,
    180.0,
    48.0,
));

let graph = RenderGraph {
    version: RENDER_GRAPH_VERSION,
    assets: vec![MediaAsset {
        id: MediaAssetId("in".into()),
        uri: "in.mp4".into(),
        duration: None,
        role: Some("video".into()),
    }],
    nodes: vec![
        RenderNode {
            id: NodeId("src".into()),
            body: RenderNodeKind::Source {
                asset: MediaAssetId("in".into()),
            },
            inputs: vec![],
        },
        RenderNode {
            id: NodeId("trim".into()),
            body: RenderNodeKind::Op {
                operation: OperationId::new("rf.transform.trim"),
                params: serde_json::json!({ "start": 0.0, "duration": 5.0 }),
            },
            inputs: vec![NodeId("src".into())],
        },
        RenderNode {
            id: NodeId("blur".into()),
            body: RenderNodeKind::Redaction {
                redaction: RegionRedaction::gaussian(masks, 12.0),
            },
            inputs: vec![NodeId("trim".into())],
        },
        RenderNode {
            id: NodeId("out".into()),
            body: RenderNodeKind::Output {
                name: "main".into(),
            },
            inputs: vec![NodeId("blur".into())],
        },
    ],
    outputs: vec![GraphOutput {
        name: "main".into(),
        node: NodeId("out".into()),
        uri: Some("out.mp4".into()),
    }],
};

let compiled = compile_graph(&graph, &OperationRegistry::with_builtins())?;
// compiled.nodes[i].output.{video, audio, masks} — contract after each node
let plan = schedule_graph(&graph, &OperationRegistry::with_builtins())?;
// plan.io[k].inputs / .outputs — numeric stage ports
println!("{}", explain_render_graph(&graph)?);
run_render_graph(&graph)?; // schedule + hybrid execute + write
```

`compile_graph` rejects broken edges (`RFGRAPH_MEDIA_CONTRACT`) — e.g. `rf.audio.gain` after `rf.audio.drop`. See [docs/GUIDE.md](docs/GUIDE.md#rendergraph).

---

## Crates

| Crate | Role |
|-------|------|
| [`reelforge`](https://crates.io/crates/reelforge) | Umbrella API + prelude |
| [`reelforge-core`](https://crates.io/crates/reelforge-core) | Time, frames, audio, traits, PSNR/SSIM |
| [`reelforge-io`](https://crates.io/crates/reelforge-io) | Decode / encode / filtergraph / graph runner |
| [`reelforge-fx`](https://crates.io/crates/reelforge-fx) | Video & audio effects |
| [`reelforge-compose`](https://crates.io/crates/reelforge-compose) | Layers, concat, composite |
| [`reelforge-text`](https://crates.io/crates/reelforge-text) | Titles & subtitles |
| [`reelforge-render-graph`](https://crates.io/crates/reelforge-render-graph) | RenderGraph, compile, schedule, masks |
| [`reelforge-cli`](https://crates.io/crates/reelforge-cli) | Command-line tool |

---

## Performance

Frame-graph microbenchmarks on a development host (**sample a transformed frame**, not full encode). Parallel row paths + fixed-point color ops.

| Workload | Typical time (this host) |
|----------|---------------------------|
| 720p crop→resize→fade→B&W (nearest) | **~0.91 ms** |
| 720p chain (bilinear) | **~1.34 ms** |
| Two-layer composite 640×360 | **~2.8 ms** |
| 4K → 1080 chain (nearest) | **~6.2 ms** |
| 4K → 1080 chain (bilinear) | **~10 ms** |
| 4K near-full chain | **~13 ms** |
| 8K → 1080 chain | **~12 ms** |
| 1080p / 4K / 8K solid `frame_at` | **~17 ns** (Arc-shared buffer) |

Re-run locally:

```bash
cargo bench -p reelforge-fx --bench frame_ops
```

**UHD:** `Size::UHD_4K` / `Size::UHD_8K`. RGB8 ≈ 25 MiB/frame (4K), ≈ 95 MiB (8K).  
**Tip:** pure file trim/scale/flip → `run_filtergraph` / CLI `cut` without importing every frame into the process.

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

---

## Dependencies

| Kind | What |
|------|------|
| In-process | `rayon`, `image`, `fontdue`, `serde` / `serde_json`, `thiserror`, `clap` (CLI) |
| Host tools | **ffmpeg**, **ffprobe** (CLI only — no libav link) |

---

## License

MIT. See [LICENSE](LICENSE).
