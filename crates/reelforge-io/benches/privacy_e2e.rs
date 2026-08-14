//! Decode → fused redaction → encode-ready / encode benches (not frame-op micros).
#![allow(
    missing_docs,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use reelforge_core::{ColorClip, Duration, Size, Time, VideoClip, VideoEffect};
use reelforge_fx::{RegionSample, RegionTrack, TrackSet, TrackedPrivacy};
use reelforge_io::{
    OpenVideoOptions, WriteVideoOptions, ffmpeg_available, frame_to_rgb24, open_video, write_video,
};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration as StdDuration;

fn crowd_tracks(n: usize, width: u32, height: u32) -> TrackSet {
    let mut set = TrackSet::new();
    let cols = ((n as f32).sqrt().ceil() as usize).max(1);
    let rows = n.div_ceil(cols);
    let cell_w = (width as f32) / cols as f32;
    let cell_h = (height as f32) / rows as f32;
    let radius = (cell_w.min(cell_h) * 0.22).max(6.0);
    for i in 0..n {
        let col = i % cols;
        let row = i / cols;
        let cx = (col as f32 + 0.5) * cell_w;
        let cy = (row as f32 + 0.5) * cell_h;
        let mut tr = RegionTrack::new(format!("s{i}"));
        tr.push(RegionSample::new(0.0, cx, cy, radius));
        tr.push(RegionSample::new(1.0, cx + 4.0, cy + 2.0, radius));
        set.push(tr);
    }
    set
}

fn redact_clip(size: Size, subjects: usize, secs: f64) -> Arc<dyn VideoClip> {
    let src: Arc<dyn VideoClip> = Arc::new(
        ColorClip::new(
            size,
            reelforge_core::Rgb8::new(40, 80, 160),
            Duration::from_secs(secs),
        )
        .with_fps(24.0),
    );
    TrackedPrivacy::gaussian(crowd_tracks(subjects, size.width, size.height), 8.0)
        .apply(src)
        .expect("apply privacy")
}

fn gen_color_mp4(path: &PathBuf, w: u32, h: u32, secs: &str, fps: &str) {
    let size = format!("{w}x{h}");
    let src = format!("color=c=red:s={size}:d={secs}:r={fps}");
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &src,
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

fn bench_privacy_e2e(c: &mut Criterion) {
    let mut g = c.benchmark_group("privacy_e2e");
    g.measurement_time(StdDuration::from_secs(3));
    g.sample_size(20);

    let hd = redact_clip(Size::HD_720, 40, 1.0);
    g.bench_function("redact_720p_40_subjects", |b| {
        b.iter(|| {
            let f = hd.frame_at(Time::from_secs(0.2)).unwrap();
            black_box(f.data().len())
        });
    });

    g.bench_function("redact_720p_pack_rgb24", |b| {
        b.iter(|| {
            let f = hd.frame_at(Time::from_secs(0.2)).unwrap();
            let raw = frame_to_rgb24(&f).unwrap();
            black_box(raw.len())
        });
    });

    if ffmpeg_available() {
        let dir = tempfile::tempdir().expect("tempdir");
        let input = dir.path().join("src.mp4");
        let output = dir.path().join("out.mp4");
        gen_color_mp4(&input, 320, 180, "0.4", "10");
        let decoded = open_video(&OpenVideoOptions::new(input.to_string_lossy())).expect("open");
        let size = decoded.size();
        let chain = TrackedPrivacy::gaussian(crowd_tracks(32, size.width, size.height), 6.0)
            .apply(Arc::new(decoded))
            .expect("apply");

        g.bench_function("decode_redact_frame", |b| {
            b.iter(|| {
                let f = chain.frame_at(Time::from_secs(0.1)).unwrap();
                black_box(f.data().len())
            });
        });

        g.measurement_time(StdDuration::from_secs(6));
        g.sample_size(10);
        g.bench_function("decode_redact_encode", |b| {
            b.iter(|| {
                write_video(
                    chain.as_ref(),
                    &WriteVideoOptions::new(output.to_string_lossy(), 10.0).with_crf(30),
                )
                .expect("encode");
                black_box(std::fs::metadata(&output).map_or(0, |m| m.len()))
            });
        });
        // Keep `dir` alive for the encode bench.
        g.finish();
        drop(dir);
        return;
    }

    g.finish();
}

criterion_group!(benches, bench_privacy_e2e);
criterion_main!(benches);
