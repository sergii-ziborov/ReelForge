//! Integration tests that require a host `ffmpeg` install.

use reelforge_core::{ColorClip, Duration, Rgb8, Size, Time, VideoClip};
use reelforge_io::{ImageClip, WriteVideoOptions, ffmpeg_available, open_video, write_video};
use std::path::PathBuf;

fn skip_without_ffmpeg() -> bool {
    if ffmpeg_available() {
        false
    } else {
        eprintln!("skipping: ffmpeg/ffprobe not available");
        true
    }
}

#[test]
fn write_color_clip_and_reopen() {
    if skip_without_ffmpeg() {
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let out: PathBuf = dir.path().join("color.mp4");

    let clip =
        ColorClip::new(Size::new(64, 48), Rgb8::RED, Duration::from_secs(0.5)).with_fps(10.0);
    let opts = WriteVideoOptions::new(out.to_string_lossy(), 10.0).with_crf(28);
    write_video(&clip, &opts).expect("write_video");

    assert!(out.is_file(), "output file missing");
    let meta = std::fs::metadata(&out).expect("meta");
    assert!(meta.len() > 0, "empty output");

    let opened = open_video(&reelforge_io::OpenVideoOptions::new(out.to_string_lossy()))
        .expect("open_video");
    assert!(opened.duration().as_secs() > 0.2);
    assert_eq!(opened.size(), Size::new(64, 48));
    let frame = opened.frame_at(Time::from_secs(0.05)).expect("frame_at");
    // Lossy encode: red channel should dominate.
    let r = frame.data()[0];
    let g = frame.data()[1];
    let b = frame.data()[2];
    assert!(r > g && r > b, "expected reddish frame, got {r},{g},{b}");
}

#[test]
fn write_image_clip() {
    if skip_without_ffmpeg() {
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let png = dir.path().join("still.png");
    let mut img = image::RgbImage::new(32, 24);
    for p in img.pixels_mut() {
        *p = image::Rgb([0, 0, 255]);
    }
    image::DynamicImage::ImageRgb8(img)
        .save(&png)
        .expect("save png");

    let clip = ImageClip::from_path(&png, Duration::from_secs(0.4))
        .expect("image clip")
        .with_fps(12.0);
    let out = dir.path().join("still.mp4");
    write_video(
        &clip,
        &WriteVideoOptions::new(out.to_string_lossy(), 12.0).with_crf(28),
    )
    .expect("write image clip");
    assert!(out.is_file());
}
