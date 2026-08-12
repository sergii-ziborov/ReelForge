//! Real-file pipeline benchmark (not Criterion microbench).
//!
//! ```bash
//! cargo run -p reelforge-io --example real_bench --release -- path/to/in.mp4 [out.mp4]
//! ```

use reelforge_core::{Duration, Size, Time, VideoClip, VideoEffect, psnr_rgb, ssim_rgb};
use reelforge_fx::{BlackAndWhite, Crop, FadeIn, Resize, ResizeFilter};
use reelforge_io::{
    OpenVideoOptions, WriteControl, WriteProgress, WriteStage, WriteVideoOptions, ffmpeg_available,
    open_video, write_video_with,
};
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: real_bench <input.mp4> [output.mp4]")?;
    let output = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/demo/real_bench_out.mp4"));

    if !ffmpeg_available() {
        return Err("ffmpeg/ffprobe not found on PATH (or REELFORGE_FFMPEG)".into());
    }

    println!("=== ReelForge real-file bench ===");
    println!("input : {input}");
    println!("output: {}", output.display());

    let t_open = Instant::now();
    let video = open_video(&OpenVideoOptions::new(&input))?;
    let open_ms = t_open.elapsed().as_secs_f64() * 1000.0;
    let size = video.size();
    let dur = video.duration();
    let fps = video.fps().unwrap_or(24.0);
    println!(
        "probe : {}x{}  duration={:.3}s  fps={fps:.3}  open_ms={open_ms:.1}",
        size.width,
        size.height,
        dur.as_secs()
    );

    let clip: Arc<dyn VideoClip> = Arc::new(video);

    // Representative MoviePy-like chain: crop → resize → fade → B&W
    let crop_w = (size.width * 9 / 10).max(2) & !1;
    let crop_h = (size.height * 9 / 10).max(2) & !1;
    let x = (size.width - crop_w) / 2;
    let y = (size.height - crop_h) / 2;
    let target = Size::new(((crop_w / 2).max(2)) & !1, ((crop_h / 2).max(2)) & !1);

    let t_graph = Instant::now();
    let cropped = Crop::new(x, y, crop_w, crop_h).apply(Arc::clone(&clip))?;
    let resized = Resize::to(target)
        .with_filter(ResizeFilter::Bilinear)
        .apply(cropped)?;
    let faded =
        FadeIn::new(Duration::from_secs((dur.as_secs() * 0.05).clamp(0.05, 0.5))).apply(resized)?;
    let chain = BlackAndWhite.apply(faded)?;
    let graph_ms = t_graph.elapsed().as_secs_f64() * 1000.0;
    println!(
        "graph : crop {crop_w}x{crop_h}@({x},{y}) → {}x{} + fade + bw  build_ms={graph_ms:.2}",
        target.width, target.height
    );

    // Sample a few frames (transform-only throughput, no encode)
    let sample_times = [0.05, 0.25, 0.5, 0.75]
        .map(|f| Time::from_secs((dur.as_secs() * f).min((dur.as_secs() - 1.0 / fps).max(0.0))));
    let t_sample = Instant::now();
    let mut frames = Vec::new();
    for t in sample_times {
        frames.push(chain.frame_at(t)?);
    }
    let sample_ms = t_sample.elapsed().as_secs_f64() * 1000.0;
    let per_frame = sample_ms / frames.len() as f64;
    println!(
        "sample: {} frames  total_ms={sample_ms:.1}  ms/frame={per_frame:.2}  fps_eq={:.1}",
        frames.len(),
        1000.0 / per_frame.max(1e-9)
    );

    // Self-consistency on same timestamp
    let a = chain.frame_at(sample_times[1])?;
    let b = chain.frame_at(sample_times[1])?;
    let psnr = psnr_rgb(&a, &b)?;
    let ssim = ssim_rgb(&a, &b)?;
    println!("quality(self): psnr={psnr:?}  ssim={ssim:.6}");

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let frames_done = Arc::new(AtomicU64::new(0));
    let frames_done2 = Arc::clone(&frames_done);
    let control =
        WriteControl::new()
            .with_max_in_flight(4)
            .with_progress(move |p: WriteProgress| {
                if p.stage == WriteStage::Video {
                    frames_done2.store(p.index, Ordering::Relaxed);
                }
            });

    let write_fps = fps.clamp(12.0, 30.0);
    // Cap long demos so the bench finishes in reasonable time.
    let write_dur = Duration::from_secs(dur.as_secs().min(8.0).max(0.5));
    let opts = WriteVideoOptions::new(output.to_string_lossy(), write_fps)
        .with_crf(23)
        .with_duration(write_dur);

    println!(
        "encode: duration={:.2}s @ {write_fps:.2} fps  max_in_flight=4  crf=23",
        write_dur.as_secs()
    );
    let t_enc = Instant::now();
    write_video_with(chain.as_ref(), &opts, &control)?;
    let enc_ms = t_enc.elapsed().as_secs_f64() * 1000.0;
    let n_prog = frames_done.load(Ordering::Relaxed);
    let out_bytes = std::fs::metadata(&output)?.len();
    let n_frames = (write_dur.as_secs() * write_fps).round().max(1.0);
    let enc_fps = n_frames / (enc_ms / 1000.0).max(1e-9);

    println!("--- results ---");
    println!("open_ms          {open_ms:.1}");
    println!("graph_build_ms   {graph_ms:.2}");
    println!("sample_ms/frame  {per_frame:.2}");
    println!("encode_ms        {enc_ms:.1}");
    println!("encode_fps       {enc_fps:.2}");
    println!("progress_frames  {n_prog}");
    println!("output_bytes     {out_bytes}");
    println!("output_path      {}", output.display());
    println!("self_psnr        {psnr:?}");
    println!("self_ssim        {ssim:.6}");
    Ok(())
}
