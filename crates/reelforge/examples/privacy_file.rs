//! Apply `TrackedPrivacy` to a real file (ellipse track, no detector).
//!
//! ```bash
//! cargo run -p reelforge --example privacy_file --release -- --input clip.mp4 --style pixelate
//! cargo run -p reelforge --example privacy_file --release -- --input clip.mp4 --cx 980 --cy 640 --radius 120 --style gaussian --sigma 80
//! ```
//!
//! Default ellipse is a talking-head box (center, ~18% of the short side).
//! Gaussian at demo σ=22 is **not** identity-safe; prefer `pixelate` / `solid`
//! or σ ≳ 0.2 × face diameter.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use reelforge::fx::{RegionSample, RegionTrack, TrackSet, TrackedPrivacy};
use reelforge::io::{WriteVideoOptions, ffmpeg_available, open_video, write_video};
use reelforge::prelude::*;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    if !ffmpeg_available() {
        return Err("host ffmpeg/ffprobe not on PATH".into());
    }
    let args: Vec<String> = env::args().skip(1).collect();
    let input = flag_value(&args, "--input").ok_or("need --input <file>")?;
    let input = PathBuf::from(input);
    if !input.is_file() {
        return Err(format!("not a file: {}", input.display()).into());
    }
    let style = flag_value(&args, "--style").unwrap_or("pixelate");
    let out = flag_value(&args, "--out").map_or_else(
        || PathBuf::from("target/demo/privacy-real.mp4"),
        PathBuf::from,
    );
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let src = open_video(&OpenVideoOptions::new(input.to_string_lossy()).video_only())?;
    let size = src.size();
    let fps = src
        .fps()
        .filter(|f| f.is_finite() && *f > 0.0)
        .unwrap_or(24.0);
    let write_fps = fps.min(30.0);
    let dur = src.duration().as_secs();
    let (cx, cy, radius) = resolve_region(&args, size);
    println!("=== ReelForge privacy on file ===");
    println!(
        "in    : {}  {}x{}  {:.2}s  {:.2} fps",
        input.display(),
        size.width,
        size.height,
        dur,
        fps
    );
    println!("track : cx={cx:.0} cy={cy:.0} r={radius:.0}  style={style}");
    println!("out   : {}", out.display());

    let mut track = RegionTrack::new("subject").with_kind("face");
    track.push(RegionSample::new(0.0, cx, cy, radius));
    track.push(RegionSample::new(dur, cx, cy, radius));
    let mut tracks = TrackSet::new();
    tracks.push(track);
    let privacy = match style {
        "gaussian" => {
            let sigma = flag_value(&args, "--sigma")
                .and_then(|s| s.parse().ok())
                .unwrap_or(80.0_f32);
            println!("sigma : {sigma}");
            TrackedPrivacy::gaussian(tracks, sigma)
        }
        "solid" => TrackedPrivacy::solid(tracks, Rgba8::new(16, 16, 18, 255)),
        _ => TrackedPrivacy::pixelate(tracks, 32),
    };
    let clip: Arc<dyn VideoClip> = privacy.apply(Arc::new(src))?;

    let t0 = Instant::now();
    write_video(
        clip.as_ref(),
        &WriteVideoOptions::new(out.to_string_lossy(), write_fps).with_crf(18),
    )?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let bytes = std::fs::metadata(&out).map_or(0, |m| m.len());
    println!("wrote : {bytes} bytes  encode={ms:.0} ms  ({write_fps:.0} fps sample)");
    println!("open  : {}", out.canonicalize().unwrap_or(out).display());
    Ok(())
}

#[allow(clippy::cast_precision_loss, clippy::similar_names)]
fn resolve_region(args: &[String], size: Size) -> (f32, f32, f32) {
    let short = size.width.min(size.height) as f32;
    let def_cx = size.width as f32 * 0.50;
    let def_cy = size.height as f32 * 0.42;
    let def_r = short * 0.18;
    (
        flag_value(args, "--cx")
            .and_then(|s| s.parse().ok())
            .unwrap_or(def_cx),
        flag_value(args, "--cy")
            .and_then(|s| s.parse().ok())
            .unwrap_or(def_cy),
        flag_value(args, "--radius")
            .and_then(|s| s.parse().ok())
            .unwrap_or(def_r),
    )
}

fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2).find_map(|w| {
        if w[0] == name {
            Some(w[1].as_str())
        } else {
            None
        }
    })
}
