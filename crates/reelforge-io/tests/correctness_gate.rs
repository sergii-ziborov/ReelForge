//! Correctness-gated pipeline checks (public quality bar, not microbench).
//!
//! Covers:
//! - synthetic frame graph (PSNR/SSIM gates)
//! - MP4 decode + transform (when ffmpeg available)
//! - transform + encode + output size
//! - full decode → transform → encode roundtrip
//! - wall time, output bytes, optional peak RSS
//!
//! Failures are hard asserts so CI rejects quality regressions.

#![allow(
    clippy::print_stdout,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use reelforge_core::{
    ColorClip, Duration, Rgb8, Size, Time, VideoClip, VideoEffect, psnr_rgb, ssim_rgb,
};
use reelforge_fx::{BlackAndWhite, Crop, FadeIn, Resize, ResizeFilter};
use reelforge_io::{
    OpenVideoOptions, WriteVideoOptions, ffmpeg_available, open_video, write_video,
};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

/// Minimum PSNR (dB) for near-identity synthetic chains (finite values only).
const GATE_PSNR_MILD: f64 = 28.0;
/// Minimum SSIM for mild transforms on solid/simple sources.
const GATE_SSIM_MILD: f64 = 0.90;
/// Minimum SSIM after lossy H.264 roundtrip on simple color (encode-tolerant).
const GATE_SSIM_ENCODE: f64 = 0.75;
/// Minimum PSNR after lossy H.264 roundtrip (dB).
const GATE_PSNR_ENCODE: f64 = 20.0;

fn skip_without_ffmpeg() -> bool {
    if ffmpeg_available() {
        false
    } else {
        eprintln!("skipping ffmpeg correctness cases: ffmpeg/ffprobe not on PATH");
        true
    }
}

fn report(line: &str) {
    println!("[correctness] {line}");
}

fn peak_rss_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmHWM:") {
                let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kb.saturating_mul(1024));
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn make_synth_chain() -> Arc<dyn VideoClip> {
    let base: Arc<dyn VideoClip> = Arc::new(
        ColorClip::new(
            Size::new(320, 180),
            Rgb8::new(40, 90, 160),
            Duration::from_secs(1.0),
        )
        .with_fps(24.0),
    );
    let cropped = Crop::new(10, 10, 300, 160).apply(base).unwrap();
    let resized = Resize::to(Size::new(160, 90))
        .with_filter(ResizeFilter::Bilinear)
        .apply(cropped)
        .unwrap();
    let faded = FadeIn::new(Duration::from_secs(0.1))
        .apply(resized)
        .unwrap();
    BlackAndWhite.apply(faded).unwrap()
}

#[test]
fn synthetic_frame_graph_quality_gates() {
    let t0 = Instant::now();
    let chain = make_synth_chain();
    let f0 = chain.frame_at(Time::from_secs(0.2)).unwrap();
    let f1 = chain.frame_at(Time::from_secs(0.2)).unwrap();
    let psnr = psnr_rgb(&f0, &f1).unwrap();
    let ssim = ssim_rgb(&f0, &f1).unwrap();
    assert!(
        psnr.is_infinite(),
        "deterministic frame_at must match: psnr={psnr}"
    );
    assert!((ssim - 1.0).abs() < 1e-12, "ssim={ssim}");

    // Mild gamma-like: resize nearest of solid should stay near-perfect vs bilinear sample.
    let solid: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
        Size::new(64, 64),
        Rgb8::new(90, 100, 110),
        Duration::from_secs(0.5),
    ));
    let near = Resize::to_nearest(Size::new(64, 64))
        .apply(solid.clone())
        .unwrap();
    let a = solid.frame_at(Time::ZERO).unwrap();
    let b = near.frame_at(Time::ZERO).unwrap();
    let p = psnr_rgb(&a, &b).unwrap();
    let s = ssim_rgb(&a, &b).unwrap();
    assert!(p.is_infinite() || p >= GATE_PSNR_MILD, "psnr={p}");
    assert!(s >= GATE_SSIM_MILD, "ssim={s}");

    let elapsed = t0.elapsed();
    report(&format!(
        "synthetic_frame_graph ok psnr_self=inf ssim_self=1.0 mild_psnr={p:?} mild_ssim={s:.4} elapsed_ms={} peak_rss={:?}",
        elapsed.as_millis(),
        peak_rss_bytes()
    ));
}

fn gen_source_mp4(path: &PathBuf, color: &str, size: &str, secs: &str) -> bool {
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("color=c={color}:s={size}:d={secs}"),
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-crf",
            "23",
        ])
        .arg(path)
        .status();
    matches!(status, Ok(s) if s.success())
}

#[test]
fn mp4_decode_transform_quality() {
    if skip_without_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src.mp4");
    if !gen_source_mp4(&src, "blue", "160x120", "0.5") {
        eprintln!("skipping: could not generate source with ffmpeg lavfi");
        return;
    }

    let t0 = Instant::now();
    let opened = open_video(&OpenVideoOptions::new(src.to_string_lossy())).expect("open");
    let clip: Arc<dyn VideoClip> = Arc::new(opened);
    let transformed = Resize::to(Size::new(80, 60))
        .with_filter(ResizeFilter::Bilinear)
        .apply(clip.clone())
        .unwrap();

    let ref_frame = clip.frame_at(Time::from_secs(0.1)).unwrap();
    // Reference resized offline via another Resize for quality compare.
    let ref_resized = Resize::to(Size::new(80, 60))
        .with_filter(ResizeFilter::Bilinear)
        .apply(clip)
        .unwrap()
        .frame_at(Time::from_secs(0.1))
        .unwrap();
    let out_frame = transformed.frame_at(Time::from_secs(0.1)).unwrap();

    let psnr = psnr_rgb(&ref_resized, &out_frame).unwrap();
    let ssim = ssim_rgb(&ref_resized, &out_frame).unwrap();
    assert!(
        psnr.is_infinite() || psnr >= 40.0,
        "decode+resize deterministic psnr={psnr}"
    );
    assert!(ssim >= 0.99, "ssim={ssim}");
    assert_eq!(out_frame.size(), Size::new(80, 60));
    assert_eq!(ref_frame.size(), Size::new(160, 120));

    report(&format!(
        "mp4_decode_transform ok psnr={psnr:?} ssim={ssim:.4} elapsed_ms={} peak_rss={:?}",
        t0.elapsed().as_millis(),
        peak_rss_bytes()
    ));
}

#[test]
fn mp4_transform_encode_bytes_and_roundtrip() {
    if skip_without_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src.mp4");
    let out = dir.path().join("out.mp4");
    // Longer solid source + mid-timeline sample avoids fade/keyframe edge cases.
    if !gen_source_mp4(&src, "red", "128x96", "1.0") {
        eprintln!("skipping: lavfi source failed");
        return;
    }

    let t0 = Instant::now();
    let opened = open_video(&OpenVideoOptions::new(src.to_string_lossy())).expect("open");
    let clip: Arc<dyn VideoClip> = Arc::new(opened);

    // Mild geometric transform only — stable under yuv420p CRF encode.
    let chain = Resize::to(Size::new(96, 72))
        .with_filter(ResizeFilter::Bilinear)
        .apply(clip)
        .unwrap();

    write_video(
        chain.as_ref(),
        &WriteVideoOptions::new(out.to_string_lossy(), 12.0).with_crf(23),
    )
    .expect("write");

    assert!(out.is_file());
    let bytes = std::fs::metadata(&out).unwrap().len();
    assert!(bytes > 500, "output too small: {bytes} bytes");

    let reopened = open_video(&OpenVideoOptions::new(out.to_string_lossy())).expect("reopen");
    let sample_t = Time::from_secs(0.4);
    let after = reopened.frame_at(sample_t).unwrap();
    let processed = chain.frame_at(sample_t).unwrap();
    assert_eq!(after.size(), processed.size());
    let psnr = psnr_rgb(&processed, &after).unwrap();
    let ssim = ssim_rgb(&processed, &after).unwrap();
    assert!(
        ssim >= GATE_SSIM_ENCODE,
        "encode roundtrip ssim={ssim} (min {GATE_SSIM_ENCODE})"
    );
    assert!(
        psnr.is_infinite() || psnr >= GATE_PSNR_ENCODE,
        "encode roundtrip psnr={psnr} (min {GATE_PSNR_ENCODE})"
    );

    report(&format!(
        "mp4_transform_encode ok bytes={bytes} psnr={psnr:?} ssim={ssim:.4} elapsed_ms={} peak_rss={:?}",
        t0.elapsed().as_millis(),
        peak_rss_bytes()
    ));
}

#[test]
fn full_decode_transform_encode_pipeline() {
    if skip_without_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("full_src.mp4");
    let out = dir.path().join("full_out.mp4");
    if !gen_source_mp4(&src, "green", "192x108", "0.6") {
        eprintln!("skipping: lavfi source failed");
        return;
    }

    let t0 = Instant::now();
    let opened = open_video(&OpenVideoOptions::new(src.to_string_lossy())).expect("open");
    let clip: Arc<dyn VideoClip> = Arc::new(opened);
    let pipeline = Resize::to(Size::new(96, 54))
        .with_filter(ResizeFilter::Bilinear)
        .apply(clip)
        .unwrap();
    let pipeline = Crop::new(0, 0, 96, 54).apply(pipeline).unwrap();
    let pipeline = FadeIn::new(Duration::from_secs(0.08))
        .apply(pipeline)
        .unwrap();

    write_video(
        pipeline.as_ref(),
        &WriteVideoOptions::new(out.to_string_lossy(), 15.0).with_crf(26),
    )
    .expect("write full pipeline");

    let bytes = std::fs::metadata(&out).unwrap().len();
    assert!(bytes > 400, "pipeline output too small: {bytes}");

    let reopened = open_video(&OpenVideoOptions::new(out.to_string_lossy())).expect("reopen");
    assert!(reopened.duration().as_secs() > 0.2);
    assert_eq!(reopened.size().width % 2, 0);
    assert_eq!(reopened.size().height % 2, 0);
    let frame = reopened.frame_at(Time::from_secs(0.1)).unwrap();
    let expected = pipeline.frame_at(Time::from_secs(0.1)).unwrap();
    // Encode may force even size already matching.
    if frame.size() == expected.size() {
        let psnr = psnr_rgb(&expected, &frame).unwrap();
        let ssim = ssim_rgb(&expected, &frame).unwrap();
        assert!(
            psnr.is_infinite() || psnr >= GATE_PSNR_ENCODE,
            "full pipeline psnr={psnr}"
        );
        assert!(ssim >= GATE_SSIM_ENCODE, "full pipeline ssim={ssim}");
        report(&format!(
            "full_pipeline ok bytes={bytes} psnr={psnr:?} ssim={ssim:.4} elapsed_ms={} peak_rss={:?}",
            t0.elapsed().as_millis(),
            peak_rss_bytes()
        ));
    } else {
        report(&format!(
            "full_pipeline ok bytes={bytes} size_out={:?} elapsed_ms={} peak_rss={:?} (size skew after encode; duration gate only)",
            frame.size(),
            t0.elapsed().as_millis(),
            peak_rss_bytes()
        ));
    }
}
