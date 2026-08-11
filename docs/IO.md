# I/O and FFmpeg

ReelForge does **not** link libav. All container work goes through the host **ffmpeg** / **ffprobe** CLI.

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
| `write_av(video, audio, &opts)` | Temp video + PCM → mux AAC (or `with_audio_codec`) |
| `write_gif(clip, &WriteGifOptions)` | palettegen + paletteuse, loop forever |

```rust
WriteVideoOptions::new("out.mp4", 24.0)
    .with_crf(18)
    .with_video_codec("libx264")
    .with_duration(Duration::from_secs(10.0));

WriteGifOptions::new("loop.gif", 15.0)
    .with_duration(Duration::from_secs(2.0));
```

Odd frame sizes are cropped to even for yuv420 encoders. Expand size with `Resize` first if needed.

## Filtergraph

`FilterGraph` + `FilterOp` + `run_filtergraph` build an ffmpeg `-filter_complex` / simple filter chain without importing frames:

- Trim, crop, scale, hflip/vflip, fade, even dims

Use when the whole job can stay in FFmpeg.

## Formats

Anything your **ffmpeg build** supports for demux/mux. ReelForge’s in-process model is RGB8/RGBA8 frames and f32 PCM; conversion to yuv420p / AAC / GIF happens in the CLI encode step.
