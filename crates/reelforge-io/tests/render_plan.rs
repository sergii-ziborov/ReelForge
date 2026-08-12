//! `RenderPlan` JSON, optimize, extract, hybrid, and (optional) `FFmpeg` execution.

use reelforge_io::{
    PlanOp, PlanOutput, RenderPlan, extract_ffmpeg, ffmpeg_available, optimize_plan,
    require_full_ffmpeg, run_render_plan,
};
use std::path::PathBuf;
use std::process::Command;

fn skip_without_ffmpeg() -> bool {
    if ffmpeg_available() {
        false
    } else {
        eprintln!("skipping: ffmpeg/ffprobe not available");
        true
    }
}

#[test]
fn json_file_roundtrip_and_optimize() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("job.json");
    let plan = RenderPlan::from_file("clip.mp4")
        .then(PlanOp::Identity)
        .then(PlanOp::Crop {
            x: 0,
            y: 0,
            w: 1280,
            h: 720,
        })
        .then(PlanOp::Crop {
            x: 100,
            y: 50,
            w: 640,
            h: 360,
        })
        .then(PlanOp::Scale { w: 1280, h: 720 })
        .then(PlanOp::Scale { w: 640, h: 360 })
        .then(PlanOp::HFlip)
        .then(PlanOp::HFlip)
        .then(PlanOp::EvenDims)
        .with_output(PlanOutput::new("out.mp4"));
    plan.save(&path).expect("save");
    let loaded = RenderPlan::load(&path).expect("load");
    assert_eq!(loaded.ops.len(), plan.ops.len());

    let opt = optimize_plan(&loaded);
    assert_eq!(opt.stats.identities_removed, 1);
    assert_eq!(opt.stats.crops_merged, 1);
    assert_eq!(opt.stats.scales_merged, 1);
    assert_eq!(opt.stats.flips_cancelled, 1);
    // crop + scale + even_dims
    assert_eq!(opt.plan.ops.len(), 3);

    let extracted = extract_ffmpeg(&loaded);
    assert!(extracted.fully_ffmpeg);
    assert_eq!(extracted.ffmpeg_op_count, 3);
    let vf = extracted.ffmpeg_vf.expect("vf");
    assert!(vf.contains("crop="));
    assert!(vf.contains("scale=640:360"));
    assert!(vf.contains("floor(iw/2)*2") || vf.contains("crop=floor"));
}

#[test]
fn custom_breaks_prefix_only() {
    let plan = RenderPlan::from_file("in.mp4")
        .then(PlanOp::Trim {
            start: 1.0,
            duration: 3.0,
        })
        .then(PlanOp::Custom {
            name: "head_blur".into(),
            params: Some(serde_json::json!({"radius": 24})),
        })
        .then(PlanOp::Scale { w: 320, h: 180 });
    let e = extract_ffmpeg(&plan);
    assert!(!e.fully_ffmpeg);
    assert_eq!(e.ffmpeg_op_count, 1);
    assert_eq!(e.remainder_op_count, 2);
    assert!(require_full_ffmpeg(&plan).is_err());
}

#[test]
fn run_fully_ffmpeg_plan_end_to_end() {
    if skip_without_ffmpeg() {
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let input: PathBuf = dir.path().join("src.mp4");
    let output: PathBuf = dir.path().join("dst.mp4");

    // Generate a short source with ffmpeg (color source).
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=320x240:d=0.5",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-crf",
            "28",
        ])
        .arg(&input)
        .status();
    let status = match status {
        Ok(s) if s.success() => s,
        Ok(s) => {
            eprintln!("skipping: ffmpeg lavfi failed with {s}");
            return;
        }
        Err(e) => {
            eprintln!("skipping: ffmpeg spawn failed: {e}");
            return;
        }
    };
    assert!(status.success());

    let plan = RenderPlan::from_file(input.to_string_lossy())
        .then(PlanOp::HFlip)
        .then(PlanOp::Scale { w: 160, h: 120 })
        .then(PlanOp::EvenDims)
        .with_output(PlanOutput {
            path: output.to_string_lossy().into_owned(),
            fps: None,
            video_codec: Some("libx264".into()),
            crf: Some(28),
        });

    run_render_plan(&plan).expect("run_render_plan");
    assert!(output.is_file());
    let meta = std::fs::metadata(&output).expect("meta");
    assert!(meta.len() > 0);
}

#[test]
fn run_hybrid_ffmpeg_prefix_then_rust_custom() {
    if skip_without_ffmpeg() {
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let input: PathBuf = dir.path().join("src.mp4");
    let output: PathBuf = dir.path().join("hybrid.mp4");

    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=160x120:d=0.4",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-crf",
            "28",
        ])
        .arg(&input)
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("skipping: ffmpeg lavfi failed with {s}");
            return;
        }
        Err(e) => {
            eprintln!("skipping: ffmpeg spawn failed: {e}");
            return;
        }
    }

    let plan = RenderPlan::from_file(input.to_string_lossy())
        .then(PlanOp::HFlip)
        .then(PlanOp::Custom {
            name: "black_and_white".into(),
            params: None,
        })
        .then(PlanOp::Scale { w: 80, h: 60 })
        .then(PlanOp::EvenDims)
        .with_output(PlanOutput {
            path: output.to_string_lossy().into_owned(),
            fps: Some(10.0),
            video_codec: Some("libx264".into()),
            crf: Some(28),
        });

    let extracted = extract_ffmpeg(&plan);
    assert!(!extracted.fully_ffmpeg);
    assert!(extracted.has_ffmpeg_segment());
    assert!(extracted.remainder_op_count >= 2);

    run_render_plan(&plan).expect("hybrid run_render_plan");
    assert!(output.is_file());
    assert!(std::fs::metadata(&output).unwrap().len() > 0);
}

#[test]
fn hybrid_rejects_unknown_custom_before_work() {
    let plan = RenderPlan::from_file("nope.mp4")
        .then(PlanOp::Custom {
            name: "head_blur".into(),
            params: None,
        })
        .with_output(PlanOutput::new("out.mp4"));
    let err = run_render_plan(&plan);
    assert!(err.is_err());
}
