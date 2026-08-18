//! Short 0.2 product reel: live test pattern, titles, privacy, slide, concat.
//!
//! ```bash
//! cargo run -p reelforge --example demo_reel --release
//! cargo run -p reelforge --example demo_reel --release -- target/demo/reelforge-0.2.mp4
//! ```
#![allow(clippy::print_stdout, clippy::print_stderr)]

use reelforge::fx::{
    FadeIn, FadeOut, RegionSample, RegionTrack, SlideIn, SlideSide, TrackSet, TrackedPrivacy,
};
use reelforge::io::{FfmpegTools, WriteVideoOptions, ffmpeg_available, write_video};
use reelforge::prelude::*;
use reelforge::text::{TextClip, TextClipOptions};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

const W: u32 = 1280;
const H: u32 = 720;
const FPS: f64 = 24.0;
const SCENE_SECS: f64 = 3.0;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    if !ffmpeg_available() {
        return Err("host ffmpeg/ffprobe not on PATH (or REELFORGE_FFMPEG)".into());
    }

    let out = env::args().nth(1).map_or_else(
        || PathBuf::from("target/demo/reelforge-0.2.mp4"),
        PathBuf::from,
    );
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let work = out.parent().unwrap_or_else(|| Path::new("target/demo"));
    std::fs::create_dir_all(work)?;

    println!("=== ReelForge 0.2 demo reel ===");
    let tools = FfmpegTools::discover()?;
    let src_a = write_lavfi(
        &tools,
        &format!("testsrc2=size={W}x{H}:rate={FPS}:duration={SCENE_SECS}"),
        &work.join("src_testsrc2.mp4"),
    )?;
    let src_b = write_lavfi(
        &tools,
        &format!("smptebars=size={W}x{H}:rate={FPS}:duration={SCENE_SECS}"),
        &work.join("src_smpte.mp4"),
    )?;

    let scene_a = titled(
        fade_privacy(open_scaled(&src_a)?)?,
        "ReelForge 0.2",
        "contracts  ·  timeline  ·  privacy",
        -80,
    )?;
    let scene_b = titled(
        SlideIn::new(Duration::from_secs(0.45), SlideSide::Right).apply(open_scaled(&src_b)?)?,
        "Capture compile",
        "ticks  ·  wipe → slides  ·  resume",
        -80,
    )?;

    let reel =
        FadeOut::new(Duration::from_secs(0.4)).apply(concatenate_video(vec![scene_a, scene_b])?)?;

    println!(
        "graph : {}x{}  {:.2}s  → {}",
        reel.size().width,
        reel.size().height,
        reel.duration().as_secs(),
        out.display()
    );
    let t0 = Instant::now();
    write_video(
        reel.as_ref(),
        &WriteVideoOptions::new(out.to_string_lossy(), FPS).with_crf(20),
    )?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let bytes = std::fs::metadata(&out).map_or(0, |m| m.len());
    println!("wrote : {bytes} bytes  encode={ms:.0} ms");
    println!("open  : {}", out.canonicalize().unwrap_or(out).display());
    Ok(())
}

fn open_scaled(path: &Path) -> Result<Arc<dyn VideoClip>, Box<dyn std::error::Error>> {
    let opened = open_video(&OpenVideoOptions::new(path.to_string_lossy()).video_only())?;
    if opened.size().width == W && opened.size().height == H {
        Ok(Arc::new(opened))
    } else {
        Ok(Resize::to(Size::new(W, H))
            .with_filter(ResizeFilter::Bilinear)
            .apply(Arc::new(opened))?)
    }
}

fn fade_privacy(src: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>, Box<dyn std::error::Error>> {
    let mut track = RegionTrack::new("subject").with_kind("face");
    // testsrc2 keeps a color circle in the upper-right — blur that.
    track.push(RegionSample::new(0.0, 980.0, 180.0, 110.0));
    track.push(RegionSample::new(SCENE_SECS, 1000.0, 200.0, 110.0));
    let mut tracks = TrackSet::new();
    tracks.push(track);
    let redacted = TrackedPrivacy::gaussian(tracks, 14.0).apply(src)?;
    Ok(FadeIn::new(Duration::from_secs(0.4)).apply(redacted)?)
}

fn titled(
    base: Arc<dyn VideoClip>,
    title: &str,
    kicker: &str,
    title_dy: i32,
) -> Result<Arc<dyn VideoClip>, Box<dyn std::error::Error>> {
    let dur = base.duration();
    let font = title_font();
    let head = TextClip::new(
        &TextClipOptions::new(title, 52, dur)
            .with_font_path(font)
            .with_color(Rgba8::new(255, 255, 255, 255))
            .with_padding(10),
    )?;
    let sub = TextClip::new(
        &TextClipOptions::new(kicker, 22, dur)
            .with_font_path(font)
            .with_color(Rgba8::new(220, 230, 240, 230))
            .with_padding(8),
    )?;
    Ok(composite_video(
        Size::new(W, H),
        vec![
            CompositeLayer::new(base),
            CompositeLayer::new(Arc::new(head))
                .with_position(Position::anchored(Anchor::Center, 0, title_dy))
                .with_layer_index(1),
            CompositeLayer::new(Arc::new(sub))
                .with_position(Position::anchored(Anchor::Center, 0, title_dy + 56))
                .with_layer_index(2),
        ],
    )?)
}

fn title_font() -> &'static str {
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
    ];
    CANDIDATES
        .iter()
        .copied()
        .find(|p| Path::new(p).is_file())
        .unwrap_or(reelforge::text::BITMAP_FONT)
}

fn write_lavfi(
    tools: &FfmpegTools,
    spec: &str,
    path: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    let status = Command::new(&tools.ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            spec,
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-crf",
            "18",
        ])
        .arg(path)
        .status()?;
    if !status.success() {
        return Err(format!("lavfi source failed: {status}").into());
    }
    Ok(path.to_path_buf())
}
