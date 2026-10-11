//! Nonzero audio concatenation through AAC export and decoded output.

use reelforge_compose::ConcatAudio;
use reelforge_core::{AudioClip, AudioFormat, ColorClip, Duration, Rgb8, SilenceClip, Size, Time};
use reelforge_io::{AudioFileClip, WriteVideoOptions, ffmpeg_available, write_av};
use std::process::Command;
use std::sync::Arc;

#[test]
fn exported_concat_keeps_both_tones_and_the_intentional_gap() {
    if !ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut pieces: Vec<Arc<dyn AudioClip>> = Vec::new();
    for (index, frequency) in [440, 880].into_iter().enumerate() {
        let path = dir.path().join(format!("tone-{index}.wav"));
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency={frequency}:sample_rate=48000:duration=0.3"
            ))
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        pieces.push(Arc::new(AudioFileClip::open(&path).unwrap()));
        if index == 0 {
            pieces.push(Arc::new(SilenceClip::new(
                AudioFormat::STEREO_48K,
                Duration::from_secs(0.2),
            )));
        }
    }
    let audio = ConcatAudio::new(pieces).unwrap();
    let video = ColorClip::new(Size::new(32, 32), Rgb8::BLUE, Duration::from_secs(0.8));
    let output = dir.path().join("concat.mp4");
    write_av(
        &video,
        &audio,
        &WriteVideoOptions::new(output.to_string_lossy(), 20.0),
    )
    .unwrap();
    let decoded = AudioFileClip::open(output).unwrap();
    let rms = |seconds: f64| {
        let buffer = decoded.samples_at(Time::from_secs(seconds), 2400).unwrap();
        #[allow(clippy::cast_precision_loss)]
        (buffer
            .samples()
            .iter()
            .map(|sample| sample * sample)
            .sum::<f32>()
            / buffer.samples().len() as f32)
            .sqrt()
    };
    assert!(rms(0.1) > 0.025, "first tone lost after encode");
    assert!(rms(0.37) < 0.005, "intentional gap is not silent");
    assert!(rms(0.6) > 0.025, "second tone lost at a concat boundary");
}
