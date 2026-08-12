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
        if p.stage == WriteStage::Video {
            frames2.store(p.index, Ordering::Relaxed);
        }
    });

write_video_with(&clip, &WriteVideoOptions::new("out.mp4", 24.0), &control)?;
// cancel.cancel(); // cooperative stop → IoError::Cancelled
```

| Control | Meaning |
|---------|---------|
| `CancelToken` | Cooperative cancel mid-encode / audio stream |
| `WriteProgress` | `stage` (Video / Audio / Mux / Done), index, total, fraction |
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

## Formats

Anything your **ffmpeg build** supports for demux/mux. ReelForge’s in-process model is RGB8/RGBA8 frames and f32 PCM; conversion to yuv420p / AAC / GIF happens in the CLI encode step.
