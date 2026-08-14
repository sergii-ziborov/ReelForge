//! Pure option builders (no ffmpeg).

use reelforge_core::SampleLayout;
use reelforge_io::{OpenAudioOptions, OpenVideoOptions, WriteVideoOptions};

#[test]
fn write_options_chain() {
    let o = WriteVideoOptions::new("out.mp4", 30.0)
        .with_video_codec("libx264")
        .with_crf(20);
    assert!((o.fps - 30.0).abs() < f64::EPSILON);
    assert_eq!(o.video_codec.as_deref(), Some("libx264"));
    assert_eq!(o.crf, Some(20));

    let nv = WriteVideoOptions::new("out.mp4", 24.0).with_nvenc(23);
    assert_eq!(nv.video_codec.as_deref(), Some("h264_nvenc"));
    assert!(nv.crf.is_none());
    assert!(nv.extra_ffmpeg_args.iter().any(|a| a == "-cq"));
    assert!(o.prefer_native_encode);
    assert!(
        !WriteVideoOptions::new("out.mp4", 24.0)
            .with_rgb_encode()
            .prefer_native_encode
    );
}

#[test]
fn open_options() {
    let v = OpenVideoOptions::new("in.mp4").video_only();
    assert!(!v.with_audio);
    let a = OpenAudioOptions::new("a.wav");
    assert_eq!(a.sample_rate, 48_000);
    assert!(a.stereo);
    assert!(!a.native_layout);
    let native = OpenAudioOptions::new("a.wav").with_native_layout();
    assert!(native.native_layout);
    let five = OpenAudioOptions::new("a.wav").with_layout(SampleLayout::Surround51);
    assert_eq!(five.layout, Some(SampleLayout::Surround51));
}
