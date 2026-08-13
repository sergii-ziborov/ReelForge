# I/O and FFmpeg

ReelForge does **not** link libav. All container work goes through the host **ffmpeg** / **ffprobe** CLI.

See also: [GUIDE.md](GUIDE.md) · [EFFECTS.md](EFFECTS.md) · [README](../README.md)

## Discovery

1. `REELFORGE_FFMPEG` / `REELFORGE_FFPROBE` if set  
2. Else `ffmpeg` / `ffprobe` on `PATH`

```rust
use reelforge::ffmpeg_available;
assert!(ffmpeg_available());
```

## Open

| API | Role |
|-----|------|
| `open_video(&OpenVideoOptions)` | Probe + `VideoFileClip` |
| `OpenVideoOptions::new(path).video_only()` | Skip audio attach |
| `video.audio()` | Optional `AudioFileClip` if open found a track |
| `open_audio(&OpenAudioOptions)` | Decode full PCM (f32, default 48 kHz stereo) |
| `OpenAudioOptions::with_native_layout()` | Keep source 5.1 / 7.1 / N-ch instead of stereo downmix |
| `SampleLayout` | `Mono` / `Stereo` / `Quad` / `Surround51` / `Surround71` / `Discrete(n)` |
| `AudioTimeline` | Sample-accurate `MediaTime` ↔ sample-frame index |
| `AudioBuffer::resample` / `resample_linear` | In-process linear rate conversion (layout unchanged) |
| `ImageClip::from_path` / `from_frame` | Still as video |

### Sequential decode

`VideoFileClip` keeps a long-lived raw RGB pipe for **forward** frame indices (encode loops). Seeking backward restarts the pipe; failures fall back to single-frame `-ss` extract.

## Write

| API | Output |
|-----|--------|
| `write_video(clip, &WriteVideoOptions)` | Default libx264 + yuv420p (even sizes) |
| `write_video_with(..., &WriteControl)` | Same + progress / cancel / pipeline depth |
| `write_av(video, audio, &opts)` | Temp video + **streamed** PCM → mux AAC |
| `write_av_with(..., &WriteControl)` | Same with controls |
| `write_gif` / `write_gif_with` | palettegen + paletteuse, loop forever |

### Progress, cancel, bounded pipeline

```rust
use reelforge::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

let cancel = CancelToken::new();
let frames = Arc::new(AtomicU64::new(0));
let frames2 = Arc::clone(&frames);
let control = WriteControl::new()
    .with_cancel(cancel.clone())
    .with_max_in_flight(4) // bounded sample workers + ordered join
    .with_progress(move |p| {
        match p.stage {
            WriteStage::Plan => { /* ExecutionPlan stage i / total */ }
            WriteStage::Video => frames2.store(p.index, Ordering::Relaxed),
            _ => {}
        }
    });

write_video_with(&clip, &WriteVideoOptions::new("out.mp4", 24.0), &control)?;
// cancel.cancel(); // cooperative stop → IoError::Cancelled
```

| Control | Meaning |
|---------|---------|
| `CancelToken` | Cooperative cancel mid-encode / audio stream |
| `WriteProgress` | `stage` (`Plan` / `Video` / `Audio` / `Mux` / `Done`), index, total, fraction |
| `max_in_flight` | `1` sequential; `>1` worker pool with ordered join (cap 32) |

Audio PCM for `write_av` is written in ~1 s chunks (no full-timeline buffer).

```rust
WriteVideoOptions::new("out.mp4", 24.0)
    .with_crf(18)
    .with_video_codec("libx264")
    .with_duration(Duration::from_secs(10.0));

// Hardware encode passthrough (requires matching ffmpeg + GPU)
WriteVideoOptions::new("out.mp4", 24.0).with_nvenc(23);
WriteVideoOptions::new("out.mp4", 24.0).with_nvenc_hevc(23);
WriteVideoOptions::new("out.mp4", 24.0).with_qsv(23);
WriteVideoOptions::new("out.mp4", 24.0).with_amf(23);
// Or raw args:
WriteVideoOptions::new("out.mp4", 24.0)
    .with_video_codec("h264_nvenc")
    .without_crf()
    .with_extra_args(["-preset", "p5", "-cq", "20", "-b:v", "0"]);

WriteGifOptions::new("loop.gif", 15.0)
    .with_duration(Duration::from_secs(2.0));
```

Odd frame sizes are cropped to even for yuv420 encoders. Expand size with `Resize` first if needed.

## Filtergraph

`FilterGraph` + `FilterOp` + `run_filtergraph` build an ffmpeg simple filter chain without importing frames:

- Trim, crop, scale, hflip/vflip, fade, even dims
- **Audio is kept** by default (`-c:a copy`). Trim also applies `atrim` + AAC. Video-only sources stay silent.
- `run_filtergraph_with(..., &FiltergraphRunOptions::new().drop_audio())` restores the old `-an` path
- Pure `RenderPlan` FFmpeg jobs and `run_filtergraph_encode` use the same policy
- Hybrid remainder remuxes the prefix/source audio onto the Rust-encoded video (`mux_copy_audio`)

Use when the whole job can stay in FFmpeg.

## RenderPlan (JSON + optimizer)

Typed deterministic plan for agents, CLI, and batch jobs:

```json
{
  "version": 1,
  "source": { "type": "file", "path": "in.mp4" },
  "ops": [
    { "op": "trim", "start": 1.0, "duration": 5.0 },
    { "op": "crop", "x": 0, "y": 0, "w": 1280, "h": 720 },
    { "op": "scale", "w": 640, "h": 360 },
    { "op": "h_flip" },
    { "op": "even_dims" }
  ],
  "output": { "path": "out.mp4", "crf": 23 }
}
```

| API | Role |
|-----|------|
| `RenderPlan::load` / `save` / `from_json` | JSON document |
| `optimize_plan` | Drop identity, cancel double flips, merge crops/scales |
| `extract_ffmpeg` | Longest pure-FFmpeg **prefix** + Rust/custom remainder |
| `run_render_plan` / `run_render_plan_with` | Pure FFmpeg **or hybrid** (prefix → Rust remainder → encode) |
| `explain_plan` | Human-readable split + `mode: ffmpeg|hybrid|rust` |
| `apply_plan_ops` | Apply remainder ops on a clip graph |

### Hybrid runner

```text
input ──► [FFmpeg prefix ops] ──► temp.mp4 ──► open ──► Rust remainder ──► write
                (optional)                         custom + crop/scale/…
```

Known `custom` names on the Rust path:

| name | params |
|------|--------|
| `black_and_white` / `bw` | — |
| `invert` | — |
| `painting` | — |
| `multiply_color` | `factor` |
| `head_blur` / `tracked_blur` / `privacy_blur` | `tracks` or `tracks_path`, optional `radius`, `radius_scale`, `feather`, `intensity` |
| `identity` | — |

### Tracks JSON (SightLoom adapter boundary)

ReelForge does **not** link SightLoom. Vision exports this intermediate:

```json
{
  "version": 1,
  "tracks": [{
    "id": "face_12",
    "kind": "face",
    "samples": [
      {"t": 0.0, "cx": 320, "cy": 180, "radius": 40, "conf": 0.94},
      {"t": 1.0, "x": 300, "y": 160, "w": 80, "h": 90, "conf": 0.91}
    ]
  }]
}
```

Samples accept **center+radius**, **x/y/w/h**, or **left/top/right/bottom** (SightLoom `Rect`-style).

```rust
use reelforge::prelude::*;

let tracks = load_track_set("faces.json")?;
let clip = TrackedBlur::new(tracks).apply(Arc::new(open_video(&opts)?))?;
// or via plan: custom head_blur + params.tracks_path
```

CLI:

```bash
reelforge plan job.json              # explain (default)
reelforge plan job.json --optimize
reelforge plan job.json --extract
reelforge plan job.json --run        # full FFmpeg plans only
```

Benches (no encode): `cargo bench -p reelforge-io --bench render_plan`

## RenderGraph

`RenderPlan` stays **one input → linear ops → one output**.  
`RenderGraph` is the DAG (several assets, fused redaction, hybrid stages). Do not grow Plan v1 into a project file.

`CaptureProject` (`compile_project`) is the OTIO-like **user timeline**. It compiles to `RenderGraph` (trim, speed, fade/dissolve, audio mix). Editor / screen capture stay in ReelForge Capture.

See the walkthrough in [GUIDE.md](GUIDE.md#rendergraph).

### Pipeline

```text
RenderGraph
    validate / from_json
    compile_graph(graph, &OperationRegistry::with_builtins())
        → CompiledGraph          // NodeIndex, typed ops, MediaContract per node
    schedule_graph / schedule_compiled
        → ExecutionPlan
            stages[]             // FFmpeg / Rust / Adapter / GPU (NodeId adapter)
            io[]                 // StageIo: numeric inputs/outputs + contracts
    run_render_graph / run_render_graph_with
        → files at GraphOutput.uri
    run_render_graph_with_manifest
        → ArtifactManifest   // planned ports + output URIs + file_fingerprint
```

| API | Role |
|-----|------|
| `graph.validate()` / `RenderGraph::from_json` | Structure, unique ids, acyclicity |
| `compile_graph` | Typed params + dense indexes + **media contracts** |
| `schedule_graph` / `schedule_compiled` | Fuse consecutive backends |
| `explain_render_graph` / `_with` | Human stage list |
| `run_render_graph` / `_with` | Schedule + hybrid execute + write |
| `run_render_graph_with_manifest` / `run_execution_plan_with_manifest` | Same + sealed `ArtifactManifest` (`file_fingerprint`) |
| `materialize_graph` / `_with_seeds` / `materialize_execution_plan` | In-process clip, no encode (tests / preview) |
| `compile_op` + `TypedParams::executor_kind` | Unary vs n-ary gather; `execute` lives in the I/O runner |

### Contracts

Each `CompiledNode.output` is a `MediaContract { video, audio, masks }`.

- File source (no role or `role: "video"`) → video + companion audio  
- `role: "audio"` → audio only  
- Visual ops / redaction **keep companion audio**  
- `rf.audio.drop` clears audio; `rf.audio.gain` after that is `RFGRAPH_MEDIA_CONTRACT` at **compile**, not at encode  

### Stage ports

`ExecutionPlan.io` is parallel to `stages` (same length after `schedule_compiled`):

```text
StageIo {
  index,
  nodes: [NodeIndex],   // fused set
  inputs,               // edges from *outside* the stage
  outputs,              // consumed by a later stage or a GraphOutput
}
```

`WriteControl` on `run_render_graph_with` / `run_execution_plan_with` reports `WriteStage::Plan` while walking those stages (`index` / `total` = stage count), then `Video` / `Audio` / `Mux` / `Done` on encode.

### Redaction

`RenderNodeKind::Redaction { redaction: RegionRedaction { masks, style } }` is one fused ROI pass. Styles: `Gaussian { sigma }`, `Pixelate { block_size }`, `Solid { color }`.

Tracks JSON / `TrackedBlur` remain the **linear** adapter. Prefer `parse_track_timelines` (SightLoom-shaped JSON, no SightLoom crate) → `RegionRedaction::gaussian_tracks`. `MaskTimeline` is the ROI view, not the identity source.

## VideoSurface (P1)

`Frame` is still packed RGB/RGBA for effects. `VideoSurface` is the timed media object:

```text
VideoSurface {
  format,          // file surface_at: Yuv420p / Nv12 / packed RGB
  size,
  planes[],        // packed RGB = one SurfacePlane; Yuv420p = Y+U+V; Nv12 = Y+UV
  timestamp,       // MediaTime PTS
  duration,        // this sample (PTS delta or 1/fps)
  time_base,       // stream  num/den  from ffprobe
  location,        // CpuPacked (RGB) / CpuPlanar (YUV)
  color            // range, space, primaries, transfer
}
```

```rust
let clip = open_video(&OpenVideoOptions::new("in.mp4"))?;
let s = clip.surface_at(Time::from_secs(1.0))?;
let pts = s.timestamp();           // stream PTS when the file has a timing index
let tb = s.time_base();            // 1/90000 etc.
let range = s.color().range;       // Limited / Full from the file when tagged
assert_eq!(s.format(), PixelFormat::Yuv420p); // typical H.264/H.265 file
let y = s.plane(0).unwrap();       // luma, stride >= width
let frame = clip.frame_at(Time::from_secs(1.0))?; // effects still get packed RGB
```

File clips override `surface_at` with a **native** rawvideo decode (`-pix_fmt yuv420p` / `nv12`, not RGB). 4:2:2 / 4:4:4 / 10-bit sources are converted in the YUV domain to 8-bit 4:2:0. PTS comes from the VFR/CFR table (`FrameTimingIndex`); color + `time_base` from `ffprobe`.

`frame_at` is still packed RGB (effects / encode stdin). `to_frame()` on a YUV surface fails — use `frame_at`. Synthetic clips stay full-range RGB. GPU `MemoryLocation::External` is reserved.

## Alpha

`FrameFormat::Rgba8` is **straight** (unassociated) unless you call `frame.premultiply()`. `blit_over` reads `Frame::alpha_mode()` so premultiplied and straight sources composite the same. `Rgb8` / YUV surfaces are `AlphaMode::Opaque`.

[`Mask`](GUIDE.md) is per-pixel **coverage** for compose / redaction — it is not a color-alpha channel and has no premultiply tag.

## Audio timeline and resample

```rust
let wav = open_audio(&OpenAudioOptions::new("music.wav"))?;
let tl = wav.timeline();
let t = MediaTime::from_secs(0.25, wav.format().sample_rate)?;
let start = tl.index_at(t);
let chunk = wav.samples_at_media(t, 1024)?;
let at_44k = chunk.resample(44_100)?;
```

`AudioFileClip` indexes PCM with `AudioTimeline` (tick math, not `floor(seconds × rate)`).  
`resample` / `resample_linear` change **rate only** (layout unchanged). FFmpeg still does the decode-time rate when you pass `OpenAudioOptions` sample_rate. Default open is stereo; `with_native_layout()` / `with_layout(SampleLayout::Surround51)` keep or request more channels.

## Formats

Anything your **ffmpeg build** supports for demux/mux. The in-process clip graph (`frame_at` / effects) is still RGB8/RGBA8 + f32 PCM. File `surface_at` is native YUV/NV12 planes. Encode still converts RGB stdin to yuv420p / AAC / GIF in the CLI step.
