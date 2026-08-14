//! Load tests: long VFR index, 50-subject fused redaction, A/V mux drift, e2e encode.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use reelforge_core::{
    AudioFormat, ColorClip, Duration, MediaTime, Rgb8, Rgba8, SampleLayout, SilenceClip, Size,
    Time, VideoClip, VideoEffect,
};
use reelforge_fx::{RegionSample, RegionTrack, TrackSet, TrackedPrivacy};
use reelforge_io::{
    FfmpegTools, OpenVideoOptions, WriteVideoOptions, av_streams_aligned, ffmpeg_available,
    open_video, probe_audio, probe_frame_timing, write_av, write_video,
};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

fn skip_without_ffmpeg() -> bool {
    if ffmpeg_available() {
        false
    } else {
        eprintln!("skipping: ffmpeg/ffprobe not available");
        true
    }
}

fn crowd_tracks(n: usize, width: u32, height: u32, radius: f32) -> TrackSet {
    let mut set = TrackSet::new();
    let cols = ((n as f32).sqrt().ceil() as usize).max(1);
    let rows = n.div_ceil(cols);
    let cell_w = (width as f32) / cols as f32;
    let cell_h = (height as f32) / rows as f32;
    for i in 0..n {
        let col = i % cols;
        let row = i / cols;
        let cx = (col as f32 + 0.5) * cell_w;
        let cy = (row as f32 + 0.5) * cell_h;
        let mut tr = RegionTrack::new(format!("s{i}"));
        tr.push(RegionSample::new(0.0, cx, cy, radius));
        set.push(tr);
    }
    set
}

fn gen_color_mp4(path: &PathBuf, spec: &str) {
    let status = Command::new("ffmpeg")
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
            "30",
        ])
        .arg(path)
        .status()
        .expect("spawn ffmpeg");
    assert!(status.success(), "ffmpeg color source failed: {status}");
}

#[test]
fn long_vfr_index_stays_monotonic() {
    const N: usize = 36_000;
    let mut pts = Vec::with_capacity(N);
    let mut t = 0.0_f64;
    for i in 0..N {
        pts.push(t);
        // 20–40 ms steps (~25–50 fps VFR) over ~18 minutes.
        t += 0.020 + f64::from(u32::try_from((i * 17) % 21).unwrap_or(0)) * 0.001;
    }
    let idx = reelforge_io::FrameTimingIndex::from_pts_secs(pts, 1_000_000).expect("index");
    assert_eq!(idx.len(), N);
    let spots = [0_u64, 1, 100, 1_000, 18_000, 35_999];
    for i in spots {
        let mt = idx.pts_at(i).expect("pts");
        assert_eq!(idx.frame_index_at(mt), i);
        if i + 1 < N as u64 {
            let a = mt.as_secs();
            let b = idx.pts_at(i + 1).expect("next").as_secs();
            let mid = MediaTime::from_secs((a + b) * 0.5, 1_000_000).expect("mid");
            assert_eq!(idx.frame_index_at(mid), i, "midpoint of gap {i}");
        }
    }
    let start = idx.pts_at(10_000).unwrap();
    let end = idx.pts_at(10_100).unwrap();
    assert_eq!(idx.frame_range(start, end), (10_000, 10_100));
}

#[test]
fn fifty_subjects_fused_redaction_keeps_background() {
    let size = Size::new(640, 360);
    let clip: Arc<dyn VideoClip> =
        Arc::new(ColorClip::new(size, Rgb8::WHITE, Duration::from_secs(1.0)));
    let tracks = crowd_tracks(50, size.width, size.height, 12.0);
    let centers: Vec<(f32, f32)> = tracks
        .tracks
        .iter()
        .filter_map(|tr| tr.region_at(0.0).map(|(cx, cy, _, _)| (cx, cy)))
        .collect();
    assert_eq!(centers.len(), 50);

    let out = TrackedPrivacy::solid(tracks, Rgba8::new(0, 0, 0, 255))
        .apply(clip)
        .expect("apply")
        .frame_at(Time::ZERO)
        .expect("frame");

    let w = size.width as usize;
    let bpp = 3;
    for (cx, cy) in centers {
        let x = (cx.round() as usize).min(w - 1);
        let y = (cy.round() as usize).min(size.height as usize - 1);
        let i = (y * w + x) * bpp;
        assert!(out.data()[i] < 250, "subject at ({x},{y}) must be filled");
    }
    // Top-left of a gapped grid stays white (union ROI, not full-frame × N).
    assert_eq!(out.data()[0], 255);
    assert_eq!(out.data()[1], 255);
    assert_eq!(out.data()[2], 255);
}

#[test]
fn decode_redact_encode_forty_subjects() {
    if skip_without_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let input = dir.path().join("src.mp4");
    let output = dir.path().join("out.mp4");
    gen_color_mp4(&input, "color=c=white:s=160x90:d=0.4:r=10");

    let decoded = open_video(&OpenVideoOptions::new(input.to_string_lossy())).expect("decode");
    let size = decoded.size();
    let chain = TrackedPrivacy::gaussian(crowd_tracks(40, size.width, size.height, 8.0), 6.0)
        .apply(Arc::new(decoded))
        .expect("apply");
    write_video(
        chain.as_ref(),
        &WriteVideoOptions::new(output.to_string_lossy(), 10.0).with_crf(30),
    )
    .expect("encode");
    assert!(output.is_file());
    assert!(std::fs::metadata(&output).expect("meta").len() > 0);

    let again = open_video(&OpenVideoOptions::new(output.to_string_lossy())).expect("reopen");
    assert!(again.duration().as_secs() > 0.2);
    assert_eq!(again.size(), size);
}

#[test]
fn muxed_av_drift_stays_within_one_frame() {
    if skip_without_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("av.mp4");
    let video =
        ColorClip::new(Size::new(64, 48), Rgb8::BLUE, Duration::from_secs(0.5)).with_fps(10.0);
    let audio = SilenceClip::new(
        AudioFormat {
            sample_rate: 16_000,
            layout: SampleLayout::Mono,
        },
        Duration::from_secs(0.5),
    );
    write_av(
        &video,
        &audio,
        &WriteVideoOptions::new(out.to_string_lossy(), 10.0).with_crf(28),
    )
    .expect("write_av");

    let tools = FfmpegTools::discover().expect("tools");
    let opened = open_video(&OpenVideoOptions::new(out.to_string_lossy())).expect("open");
    let audio_probe = probe_audio(&tools, &out).expect("probe audio");
    let fps = opened.fps().unwrap_or(10.0);
    let drift_ok = av_streams_aligned(
        opened.duration().as_secs(),
        audio_probe.duration.as_secs(),
        fps,
    );
    assert!(
        drift_ok,
        "A/V drift too large: video={:.4}s audio={:.4}s fps={fps}",
        opened.duration().as_secs(),
        audio_probe.duration.as_secs()
    );
}

#[test]
fn long_vfr_file_timing_index() {
    if skip_without_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("vfr.mp4");
    // Jittered PTS inside the clip duration (stays VFR, no packets past EOF).
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=green:s=64x48:d=3:r=30",
            "-vf",
            "setpts=(N/30+0.006*not(eq(mod(N\\,3)\\,0)))/TB",
            "-fps_mode",
            "vfr",
            "-an",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-crf",
            "30",
        ])
        .arg(&path)
        .status()
        .expect("spawn ffmpeg");
    assert!(status.success(), "vfr encode failed: {status}");

    let tools = FfmpegTools::discover().expect("tools");
    let idx = probe_frame_timing(&tools, &path, 0).expect("pts");
    assert!(
        idx.len() >= 8,
        "expected several VFR packets, got {}",
        idx.len()
    );
    let first = idx.pts_at(0).unwrap();
    let last = idx.pts_at(u64::try_from(idx.len() - 1).unwrap()).unwrap();
    assert!(last.as_secs() > first.as_secs());
    let (a, b) = idx.frame_range(first, last);
    assert!(b > a);
    assert_eq!(idx.frame_index_at(first), 0);
}
