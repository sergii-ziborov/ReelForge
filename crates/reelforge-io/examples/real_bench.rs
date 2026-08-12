//! Real-file pipeline benchmark (not Criterion microbench).
//!
//! ```bash
//! cargo run -p reelforge-io --example real_bench --release -- path/to/in.mp4 [out.mp4]
//! ```
#![allow(
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use reelforge_core::{
    CachedVideo, Duration, FrameStream, Size, Time, VideoClip, VideoEffect, psnr_rgb, ssim_rgb,
};
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

struct Probe {
    size: Size,
    dur: Duration,
    fps: f64,
    open_ms: f64,
}

struct GraphOut {
    chain: Arc<dyn VideoClip>,
    crop_w: u32,
    crop_h: u32,
    x: u32,
    y: u32,
    target: Size,
    graph_ms: f64,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (input, output) = parse_args()?;
    ensure_ffmpeg()?;

    println!("=== ReelForge real-file bench ===");
    println!("input : {input}");
    println!("output: {}", output.display());

    let probe = open_and_probe(&input)?;
    println!(
        "probe : {}x{}  duration={:.3}s  fps={:.3}  open_ms={:.1}",
        probe.size.width,
        probe.size.height,
        probe.dur.as_secs(),
        probe.fps,
        probe.open_ms
    );

    let graph = build_chain(&input, &probe)?;
    println!(
        "graph : crop {}x{}@({},{}) → {}x{} + fade + bw  build_ms={:.2}",
        graph.crop_w,
        graph.crop_h,
        graph.x,
        graph.y,
        graph.target.width,
        graph.target.height,
        graph.graph_ms
    );

    let sample = sample_frames(Arc::clone(&graph.chain), &probe)?;
    println!(
        "sample: {} frames cold_ms/frame={:.2} warm_ms/frame={:.2} stream_ms/frame={:.2} cache_hit={:.1}%",
        sample.count,
        sample.per_frame_ms,
        sample.warm_ms,
        sample.stream_ms_per_frame,
        sample.cache_hit_rate * 100.0
    );
    println!(
        "quality(self): psnr={:?}  ssim={:.6}",
        sample.psnr, sample.ssim
    );

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let stem = output
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("bench");
    let enc_base = encode_chain(
        graph.chain.as_ref(),
        &probe,
        &output,
        4,
        EncodeMode::Default,
    )?;
    let out_fast = output.with_file_name(format!("{stem}_fast.mp4"));
    let enc_fast = encode_chain(
        graph.chain.as_ref(),
        &probe,
        &out_fast,
        8,
        EncodeMode::VeryFast,
    )?;
    let out_uf = output.with_file_name(format!("{stem}_ultra.mp4"));
    let enc_uf = encode_chain(
        graph.chain.as_ref(),
        &probe,
        &out_uf,
        1,
        EncodeMode::UltraFast,
    )?;

    println!("--- results ---");
    println!("open_ms          {:.1}", probe.open_ms);
    println!("graph_build_ms   {:.2}", graph.graph_ms);
    println!("sample_ms/frame  {:.2}", sample.per_frame_ms);
    println!("warm_ms/frame    {:.2}", sample.warm_ms);
    println!("stream_ms/frame  {:.2}", sample.stream_ms_per_frame);
    println!("cache_hit_rate   {:.1}%", sample.cache_hit_rate * 100.0);
    println!(
        "encode_base      {:.1} ms  {:.2} fps  (in_flight=4, default)",
        enc_base.enc_ms, enc_base.enc_fps
    );
    println!(
        "encode_fast      {:.1} ms  {:.2} fps  (in_flight=8, veryfast)",
        enc_fast.enc_ms, enc_fast.enc_fps
    );
    println!(
        "encode_ultra     {:.1} ms  {:.2} fps  (in_flight=1, ultrafast)",
        enc_uf.enc_ms, enc_uf.enc_fps
    );
    println!(
        "speedup_vs_base  fast={:.2}x  ultra={:.2}x",
        enc_base.enc_ms / enc_fast.enc_ms.max(1e-9),
        enc_base.enc_ms / enc_uf.enc_ms.max(1e-9)
    );
    println!("progress_frames  {}", enc_uf.progress_frames);
    println!("output_bytes     {}", enc_uf.out_bytes);
    println!("output_path      {}", out_uf.display());
    println!("self_psnr        {:?}", sample.psnr);
    println!("self_ssim        {:.6}", sample.ssim);
    Ok(())
}

#[derive(Clone, Copy)]
enum EncodeMode {
    Default,
    VeryFast,
    UltraFast,
}

fn parse_args() -> Result<(String, PathBuf), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: real_bench <input.mp4> [output.mp4]")?;
    let output = args.next().map_or_else(
        || PathBuf::from("target/demo/real_bench_out.mp4"),
        PathBuf::from,
    );
    Ok((input, output))
}

fn ensure_ffmpeg() -> Result<(), Box<dyn std::error::Error>> {
    if ffmpeg_available() {
        Ok(())
    } else {
        Err("ffmpeg/ffprobe not found on PATH (or REELFORGE_FFMPEG)".into())
    }
}

fn open_and_probe(input: &str) -> Result<Probe, Box<dyn std::error::Error>> {
    let t_open = Instant::now();
    let video = open_video(&OpenVideoOptions::new(input))?;
    let open_ms = t_open.elapsed().as_secs_f64() * 1000.0;
    Ok(Probe {
        size: video.size(),
        dur: video.duration(),
        fps: video.fps().unwrap_or(24.0),
        open_ms,
    })
}

fn build_chain(input: &str, probe: &Probe) -> Result<GraphOut, Box<dyn std::error::Error>> {
    let video = open_video(&OpenVideoOptions::new(input))?;
    let clip: Arc<dyn VideoClip> = Arc::new(video);
    let size = probe.size;
    let crop_w = (size.width * 9 / 10).max(2) & !1;
    let crop_h = (size.height * 9 / 10).max(2) & !1;
    let x = (size.width - crop_w) / 2;
    let y = (size.height - crop_h) / 2;
    let target = Size::new(((crop_w / 2).max(2)) & !1, ((crop_h / 2).max(2)) & !1);

    let t_graph = Instant::now();
    let cropped = Crop::new(x, y, crop_w, crop_h).apply(clip)?;
    let resized = Resize::to(target)
        .with_filter(ResizeFilter::Bilinear)
        .apply(cropped)?;
    let fade_secs = (probe.dur.as_secs() * 0.05).clamp(0.05, 0.5);
    let faded = FadeIn::new(Duration::from_secs(fade_secs)).apply(resized)?;
    let chain = BlackAndWhite.apply(faded)?;
    Ok(GraphOut {
        chain,
        crop_w,
        crop_h,
        x,
        y,
        target,
        graph_ms: t_graph.elapsed().as_secs_f64() * 1000.0,
    })
}

struct SampleStats {
    count: usize,
    per_frame_ms: f64,
    warm_ms: f64,
    psnr: f64,
    ssim: f64,
    cache_hit_rate: f64,
    stream_ms_per_frame: f64,
}

fn sample_frames(
    chain: Arc<dyn VideoClip>,
    probe: &Probe,
) -> Result<SampleStats, Box<dyn std::error::Error>> {
    let cached = Arc::new(CachedVideo::realtime(chain, 2.0));

    let fracs = [0.05_f64, 0.25, 0.5, 0.75];
    let sample_times: Vec<Time> = fracs
        .iter()
        .map(|f| {
            Time::from_secs(
                (probe.dur.as_secs() * f).min((probe.dur.as_secs() - 1.0 / probe.fps).max(0.0)),
            )
        })
        .collect();

    let t_sample = Instant::now();
    for t in &sample_times {
        let _ = cached.frame_at(*t)?;
    }
    let total_ms = t_sample.elapsed().as_secs_f64() * 1000.0;
    let count = sample_times.len();
    let per_frame_ms = total_ms / count as f64;

    let mid = sample_times[1];
    let t_warm = Instant::now();
    for _ in 0..16 {
        let _ = cached.frame_at(mid)?;
    }
    let warm_ms = t_warm.elapsed().as_secs_f64() * 1000.0 / 16.0;

    let a = cached.frame_at(mid)?;
    let b = cached.frame_at(mid)?;
    let stats = cached.stats();

    let mut stream = FrameStream::new(Arc::clone(&cached) as Arc<dyn VideoClip>, Some(probe.fps))?;
    stream.set_prefetch_capacity(8);
    stream.fill_prefetch();
    let t_stream = Instant::now();
    let mut n_stream = 0_u64;
    let stream_limit = 30_u64.min(stream.len());
    while n_stream < stream_limit {
        if stream.next_frame()?.is_none() {
            break;
        }
        n_stream += 1;
    }
    let stream_ms = t_stream.elapsed().as_secs_f64() * 1000.0;
    let stream_ms_per_frame = stream_ms / n_stream.max(1) as f64;

    Ok(SampleStats {
        count,
        per_frame_ms,
        warm_ms,
        psnr: psnr_rgb(&a, &b)?,
        ssim: ssim_rgb(&a, &b)?,
        cache_hit_rate: stats.hit_rate(),
        stream_ms_per_frame,
    })
}

struct EncodeStats {
    enc_ms: f64,
    enc_fps: f64,
    progress_frames: u64,
    out_bytes: u64,
}

fn encode_chain(
    chain: &dyn VideoClip,
    probe: &Probe,
    output: &PathBuf,
    in_flight: usize,
    mode: EncodeMode,
) -> Result<EncodeStats, Box<dyn std::error::Error>> {
    let frames_done = Arc::new(AtomicU64::new(0));
    let frames_done2 = Arc::clone(&frames_done);
    let control = WriteControl::new()
        .with_max_in_flight(in_flight)
        .with_progress(move |p: WriteProgress| {
            if p.stage == WriteStage::Video {
                frames_done2.store(p.index, Ordering::Relaxed);
            }
        });

    let write_fps = probe.fps.clamp(12.0, 30.0);
    let write_secs = probe.dur.as_secs().clamp(0.5, 8.0);
    let write_dur = Duration::from_secs(write_secs);
    let mut opts = WriteVideoOptions::new(output.to_string_lossy(), write_fps)
        .with_crf(23)
        .with_duration(write_dur);
    opts = match mode {
        EncodeMode::Default => opts,
        EncodeMode::VeryFast => opts.with_fast_encode(),
        EncodeMode::UltraFast => opts.with_ultrafast_encode(),
    };

    let t_enc = Instant::now();
    write_video_with(chain, &opts, &control)?;
    let enc_ms = t_enc.elapsed().as_secs_f64() * 1000.0;
    let n_frames = (write_secs * write_fps).round().max(1.0);
    let enc_fps = n_frames / (enc_ms / 1000.0).max(1e-9);
    let _ = (write_secs, write_fps);
    Ok(EncodeStats {
        enc_ms,
        enc_fps,
        progress_frames: frames_done.load(Ordering::Relaxed),
        out_bytes: std::fs::metadata(output)?.len(),
    })
}
