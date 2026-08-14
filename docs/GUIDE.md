# ReelForge user guide

Programmatic video and audio editing for Rust: a lazy **clip graph** that samples frames and PCM on demand, then encodes with host **ffmpeg**.

See also: [EFFECTS.md](EFFECTS.md) · [IO.md](IO.md) · [README](../README.md)

---

## Concepts

| Term | Meaning |
|------|---------|
| `VideoClip` | Timed source: `frame_at(t)` → RGB/RGBA `Frame`; `surface_at(t)` → timed `VideoSurface` |
| `VideoSurface` | Native planes (YUV/NV12/BGRA from files, packed RGB from the clip graph), PTS, color, or an `ExternalSurface` GPU handle. File encode stays on YUV when no pixel effect ran. |
| `AlphaMode` | `Opaque` / `Straight` / `Premultiplied` on RGBA `Frame` (mask is coverage, not color alpha) |
| `AudioClip` | Timed source: `samples_at` / `samples_at_media` → interleaved f32 PCM |
| `AudioTimeline` | Sample-accurate `MediaTime` ↔ PCM frame index |
| `resample_linear` | In-process linear resample to another sample rate |
| `VideoEffect` / `AudioEffect` | Pure transforms: `apply(clip) → Arc<dyn …>` |
| `CompositeVideo` | Layered canvas: position, opacity, masks, z-order |
| Lazy graph | Nothing is rendered until `frame_at` / write |
| `CachedVideo` / `cache_video` | LRU frame cache for warm / realtime `frame_at` |
| `FrameStream` / `stream_video` | Sequential stream + optional prefetch |
| `RenderPlan` | Linear one-shot: one file → ops → one file (CLI / agents) |
| `RenderGraph` | Typed DAG: assets, ops, redaction, outputs → compile → run |

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
let surround = open_audio(&OpenAudioOptions::new("mix.wav").with_native_layout())?;
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
            duration: 5.0,
        })
        .then(FilterOp::EvenDims),
)?;
```

Source audio is copied (`-c:a copy`). Trim also cuts audio (`atrim`). Use `FiltergraphRunOptions::drop_audio()` to strip it.

---

## RenderGraph

Use the **clip graph** (`Arc<dyn VideoClip>` + effects) for scripted MoviePy-style work.  
Use **RenderPlan** for a single input chain (JSON / CLI).  
Use **RenderGraph** when the job is a DAG: several assets, fused privacy, hybrid FFmpeg + Rust, inspectable stages.

```text
RenderGraph.validate
    → compile_graph      typed ops + numeric indexes + media contracts
    → schedule_graph     fuse consecutive backends → ExecutionPlan
    → run_render_graph   walk stages, write GraphOutput.uri
    → artifact_manifest  planned products (stage ports, contracts, output URIs)
```

These types live on the `reelforge` crate root (not all of them are in `prelude`).

### CaptureProject → RenderGraph

`CaptureProject` is an OTIO-like **user timeline** (sequences, tracks, clips, gaps, markers, semantic refs). It compiles to `RenderGraph`. It is **not** the editor, not screen capture, and not a replacement for `RenderPlan` v1.

```rust
use reelforge::{
    CaptureProject, Gap, MediaRef, MediaRefId, ProjectId, Sequence, SequenceId, TimelineItem,
    TimelineTrack, TimelineTrackId, TrackKind, compile_project,
};

let mut project = CaptureProject::new(ProjectId::new("demo"), "demo");
project.media.push(MediaRef {
    id: MediaRefId::new("a"),
    uri: "in.mp4".into(),
    duration: None,
    role: Some("video".into()),
});
// … push a video track with clips / gaps …
let compiled = compile_project(&project)?;
// compiled.graph → compile_graph / run_render_graph
```

`version: 0` migrates to v1. Compile v1: trim, `speed`, fade / dissolve, audio-track `mix`. Wipe is stored, not compiled. Markers stay editorial.

### Build, inspect, run

```rust
use reelforge::{
    GraphOutput, MaskSample, MaskTimeline, MediaAsset, MediaAssetId, MediaTime, NodeId,
    OperationId, OperationRegistry, RENDER_GRAPH_VERSION, RegionRedaction, RenderGraph,
    GraphRunOptions, RenderNode, RenderNodeKind, WriteControl, artifact_manifest, compile_graph,
    explain_render_graph, run_render_graph_with_manifest, schedule_graph,
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
                params: serde_json::json!({ "start": 1.0, "duration": 4.0 }),
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

graph.validate()?;
let compiled = compile_graph(&graph, &OperationRegistry::with_builtins())?;
assert!(compiled.lookup(&NodeId("trim".into()))?.as_usize() < compiled.nodes.len());
// Each node has an inferred output contract:
assert!(compiled.nodes.iter().any(|n| n.output.video));

let plan = schedule_graph(&graph, &OperationRegistry::with_builtins())?;
let manifest = artifact_manifest(&compiled, &plan);
assert_eq!(manifest.outputs[0].uri.as_deref(), Some("out.mp4"));
for (i, io) in plan.io.iter().enumerate() {
    println!(
        "stage {i} ({}) in={} out={}",
        plan.stages[i].backend_tag(),
        io.inputs.len(),
        io.outputs.len()
    );
}

println!("{}", explain_render_graph(&graph)?);
let sealed = run_render_graph_with_manifest(&graph, &WriteControl::default(), &GraphRunOptions::default())?;
assert!(sealed.outputs[0].file_fingerprint.is_some());
```

`run_render_graph` / `run_render_graph_with` still just write files.  
`run_render_graph_with_manifest` returns the planned `ArtifactManifest` with `file_fingerprint` filled for outputs that exist on disk.

`run_render_graph_with` accepts `WriteControl` (progress / cancel). Walking `ExecutionPlan` stages reports `WriteStage::Plan`; encode still uses `Video` / `Audio` / `Mux` / `Done`.

### What compile checks

| Check | Error |
|-------|--------|
| Duplicate node / asset / output names | `RFGRAPH_DUPLICATE_*` |
| Cycles | `RFGRAPH_CYCLE` |
| Unknown op or bad params | `RFGRAPH_UNKNOWN_OPERATION` / `RFGRAPH_INVALID_PARAMS` |
| Missing stream on an edge (e.g. gain after `rf.audio.drop`) | `RFGRAPH_MEDIA_CONTRACT` |

Asset `role`: omit or `"video"` → video + companion audio; `"audio"` → audio only.

### Builtin ops (ids)

| Id | Typical use |
|----|-------------|
| `rf.transform.trim` / `hflip` / `vflip` / `scale` / `crop` / `even_dims` / `rotate` / `fade_in` / `fade_out` | Geometry / time |
| `rf.color.black_and_white` / `invert` / `painting` | Look |
| `rf.adapter.sightloom` | Adapter stage: JSON tracks / `AdapterHost` → `MaskTimeline` |
| `rf.redaction.region` or `RenderNodeKind::Redaction` | Fused privacy (`MaskTimeline` + optional `MaskAsset`) |
| `rf.compose.layers` | Multi-input composite |
| `rf.audio.gain` / `drop` / `preserve` / `mix` | Audio |
| `rf.encode.h264` | Encode hints (`crf`, `path`, `preserve_audio`) |
| `rf.gpu.passthrough` / `rf.encode.hw` | GPU stage: passthrough or NVENC/QSV/AMF encode hint |

Authoring ids (`NodeId`) are aliases. After compile, execution identity is a dense `NodeIndex` (canonical topo). Permuting the JSON `nodes` array compiles to the same program.

### Masks / tracks

`TrackTimeline` is the identity source (`TrackId` + optional `SubjectId` / `AppearanceId` / `ObservationId` / `Geometry` / `OcclusionState` / `MaskRef`).  
`MaskTimeline` is a **materialized ROI view** of one or more tracks — not a vision index. ReelForge does not query subjects.

Pixel silhouettes travel as `MaskAsset` (`Dense` / `Cropped` / `Rle` / `Polygon` / `External`) on `MaskSample.asset` / `MaskFrame`. The privacy pass stamps a **union coverage ROI** and blurs only that crop — not the whole frame × N subjects.

`rf.adapter.sightloom` is a real **adapter executor**. `AdapterRegistry` ships a JSON SightLoom executor. `SightloomPackageHost` opens a folder (`manifest.json` + `masks/*.bin`) and resolves `MaskAsset::External` to dense/cropped coverage — no SightLoom crate. A custom `AdapterHost` can still wrap a live vision process. Empty-mask `Redaction` nodes consume the adapter's timeline.

GPU stages (`rf.gpu.passthrough`, `rf.encode.hw`) execute instead of failing closed. `GpuRegistry` passthrough keeps the clip; `rf.encode.hw` probes host `ffmpeg` for NVENC/QSV/AMF (or uses `backend` / `codec`). A `GpuHost` can replace the clip with an `ExternalSurface` device path. ReelForge does not ship CUDA kernels.

Durable jobs: `JobStore` persists `{id}/job.json`. `submit_render_job` fingerprints the graph+plan. `run_render_job` / `resume_render_job` write `Running` → `Done`, cancel → `Paused`, other errors → `Failed`. A `Done` job with the same fingerprint and an output file is a no-op. In-process stages re-evaluate on resume; a matching `StageCache` hit still skips encode. Capture owns the queue.

Preview contract: `PreviewRequest` + `PreviewQuality` (`Draft` / `Proxy` / `Full`). `preview_clip` / `preview_graph` sample one RGB frame at `MediaTime` (downscale, no encode). `write_proxy` / thumbnails remain the file-side hooks. Capture owns which spec to cache.

`rf.transform.trim` / `fade_in` / `fade_out` compile to `MediaTime` ticks (`MediaRange` for trim). Float seconds in JSON become 1 MHz ticks; `{ticks, timescale}` is preserved. Conversion to `Time`/`Duration` happens only at the effect / FFmpeg boundary.

```rust
use reelforge::{
    MediaTime, RegionRedaction, SubjectId, TrackId, TrackSample, TrackTimeline,
    mask_timeline_from_tracks, parse_track_timelines,
};

let mut track = TrackTimeline::new(TrackId::new("tr_1")).with_subject(SubjectId::new("person_a"));
track.push(TrackSample::ellipse(TrackId::new("tr_1"), MediaTime::new(0, 30)?, 320.0, 180.0, 40.0));
let masks = mask_timeline_from_tracks([&track]);
let redaction = RegionRedaction::gaussian_tracks([&track], 12.0);
```

Or ingest a SightLoom-shaped JSON export (**no SightLoom crate**):

```rust
let tracks = parse_track_timelines(&json)?;
let redaction = RegionRedaction::gaussian_tracks(&tracks, 12.0);
```

One fused `RegionRedaction` — not one blur node per face. `reelforge-sightloom-adapter` only maps JSON → `TrackTimeline`.

More I/O detail: [IO.md](IO.md#rendergraph).

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
reelforge plan job.json --explain
reelforge plan job.json --run
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

Imports the common clip, effect, compose, I/O, and text types used in the clip-graph examples.  
`RenderGraph`, `compile_graph`, `schedule_graph`, and `run_render_graph` are on the crate root (`use reelforge::…`).
