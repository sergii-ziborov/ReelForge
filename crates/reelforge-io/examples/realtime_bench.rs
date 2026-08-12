//! Realtime / GPU export bench (filtergraph path — no Rust RGB).
//!
//! ```bash
//! cargo run -p reelforge-io --example realtime_bench --release -- path/to/in.mp4
//! ```
#![allow(
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::similar_names
)]

use reelforge_core::{Duration, VideoClip};
use reelforge_io::{
    FilterOp, OpenVideoOptions, RealtimeExport, detect_hw_encoders, ffmpeg_available,
    nvenc_available, open_video,
};
use std::env;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    if !ffmpeg_available() {
        return Err("ffmpeg not available".into());
    }
    let input = env::args()
        .nth(1)
        .unwrap_or_else(|| "target/demo/sample-5s.mp4".into());
    let out_dir = PathBuf::from("target/demo");
    std::fs::create_dir_all(&out_dir)?;

    let probe = open_video(&OpenVideoOptions::new(&input).video_only())?;
    let fps = probe.fps().unwrap_or(30.0);
    let dur = probe.duration();
    let size = probe.size();
    let crop_w = (size.width * 9 / 10).max(2) & !1;
    let crop_h = (size.height * 9 / 10).max(2) & !1;
    let x = (size.width - crop_w) / 2;
    let y = (size.height - crop_h) / 2;
    let tw = ((crop_w / 2).max(2)) & !1;
    let th = ((crop_h / 2).max(2)) & !1;
    let fade = (dur.as_secs() * 0.05).clamp(0.05, 0.5);

    println!("=== ReelForge realtime export bench ===");
    println!("input : {input}");
    println!(
        "probe : {}x{}  {:.2}s @ {:.1}fps",
        size.width,
        size.height,
        dur.as_secs(),
        fps
    );
    let hw = detect_hw_encoders()?;
    println!(
        "hw    : nvenc_h264={} qsv={} amf={} (nvenc_available={})",
        hw.nvenc_h264,
        hw.qsv_h264,
        hw.amf_h264,
        nvenc_available()
    );

    let graph = |g: reelforge_io::FilterGraph| {
        g.then(FilterOp::Crop {
            w: crop_w,
            h: crop_h,
            x,
            y,
        })
        .then(FilterOp::Scale { w: tw, h: th })
        .then(FilterOp::FadeIn { duration: fade })
        .then(FilterOp::BlackAndWhite)
    };

    // CPU ultrafast filtergraph path
    let out_cpu = out_dir.join("realtime_cpu_ultra.mp4");
    let exp_cpu = RealtimeExport::new(&input, out_cpu.to_string_lossy(), fps)?
        .with_cpu_ultrafast()
        .with_graph(graph(reelforge_io::FilterGraph::new()))
        .with_duration(Duration::from_secs(dur.as_secs().min(5.7)));
    println!("vf    : {}", exp_cpu.vf()?);
    let t0 = Instant::now();
    exp_cpu.run()?;
    let cpu_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let n = (dur.as_secs().min(5.7) * fps).round().max(1.0);
    println!(
        "cpu_ultrafast : {:.1} ms  {:.2} fps  bytes={}",
        cpu_ms,
        n / (cpu_ms / 1000.0),
        std::fs::metadata(&out_cpu)?.len()
    );

    // GPU path if present
    if hw.nvenc_h264 {
        let out_gpu = out_dir.join("realtime_nvenc.mp4");
        let exp_gpu = RealtimeExport::new(&input, out_gpu.to_string_lossy(), fps)?
            .with_nvenc(23)
            .with_graph(graph(reelforge_io::FilterGraph::new()))
            .with_duration(Duration::from_secs(dur.as_secs().min(5.7)));
        let t1 = Instant::now();
        match exp_gpu.run() {
            Ok(()) => {
                let gpu_ms = t1.elapsed().as_secs_f64() * 1000.0;
                println!(
                    "nvenc_realtime: {:.1} ms  {:.2} fps  bytes={}  speedup_vs_cpu={:.2}x",
                    gpu_ms,
                    n / (gpu_ms / 1000.0),
                    std::fs::metadata(&out_gpu)?.len(),
                    cpu_ms / gpu_ms.max(1e-9)
                );
            }
            Err(e) => println!("nvenc_realtime: FAILED ({e}) — driver/GPU may be missing"),
        }
    } else {
        println!("nvenc_realtime: skipped (no h264_nvenc in this ffmpeg build)");
    }

    println!(
        "note  : zero Rust RGB frames — decode+filters+encode stay in ffmpeg (MoviePy cannot)"
    );
    Ok(())
}
