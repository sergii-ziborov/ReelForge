//! Pure option builders (no ffmpeg).

use reelforge_io::{OpenAudioOptions, OpenVideoOptions, WriteVideoOptions};

#[test]
fn write_options_chain() {
    let o = WriteVideoOptions::new("out.mp4", 30.0)
        .with_video_codec("libx264")
        .with_crf(20);
    assert!((o.fps - 30.0).abs() < f64::EPSILON);
    assert_eq!(o.video_codec.as_deref(), Some("libx264"));
    assert_eq!(o.crf, Some(20));
}

#[test]
fn open_options() {
    let v = OpenVideoOptions::new("in.mp4").video_only();
    assert!(!v.with_audio);
    let a = OpenAudioOptions::new("a.wav");
    assert_eq!(a.sample_rate, 48_000);
    assert!(a.stereo);
}
