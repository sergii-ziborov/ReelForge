# Correctness gates

Public quality bar for ReelForge pipelines. These are **not** Criterion microbenchmarks; they sit **next to** them and fail CI when quality regresses.

## What is gated

| Check | Where | Requires ffmpeg |
|-------|--------|-----------------|
| Synthetic frame graph determinism + mild PSNR/SSIM | `reelforge-io` test `correctness_gate` | no |
| MP4 decode + transform | same | yes |
| Transform + encode + output size | same | yes |
| Full decode → transform → encode | same | yes |
| Wall time / output bytes / peak RSS (Linux VmHWM) | logged on success | optional |

### Numeric floors (lossy encode)

| Metric | Gate |
|--------|------|
| Mild solid / nearest identity SSIM | ≥ 0.90 |
| Mild PSNR when finite | ≥ 28 dB |
| After H.264 roundtrip SSIM | ≥ 0.75 |
| After H.264 roundtrip PSNR | ≥ 20 dB |

Identical pure-Rust samples must report **infinite PSNR** and **SSIM = 1**.

## Run locally

```bash
# Pure + skip ffmpeg cases if tools missing
cargo test -p reelforge-io --test correctness_gate -- --nocapture

# With host ffmpeg/ffprobe on PATH
cargo test -p reelforge-io --test correctness_gate -- --nocapture
cargo test -p reelforge-io --test ffmpeg_roundtrip
```

## CI

Workflow [`.github/workflows/ci.yml`](../.github/workflows/ci.yml):

1. **quality** — `ubuntu` / `windows` / `macos`: fmt, clippy `-D warnings`, test, docs, package dry-run, bench compile  
2. **ffmpeg-correctness** — Ubuntu + apt `ffmpeg`: correctness gate + ffmpeg integration tests  

Publish workflow runs the same preflight (fmt/clippy/test/package) **before** crates.io upload.

## Microbenchmarks (separate)

```bash
cargo bench -p reelforge-fx --bench frame_ops
cargo bench -p reelforge-io --bench render_plan
```

Use benches for **speed**; use correctness_gate for **trust**.
