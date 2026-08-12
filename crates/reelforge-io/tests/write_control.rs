//! `WriteControl`: progress, cancel, streaming audio, optional `FFmpeg` encode.

use reelforge_core::{AudioFormat, ColorClip, Duration, Rgb8, SampleLayout, SilenceClip, Size};
use reelforge_io::{
    CancelToken, IoError, WriteControl, WriteProgress, WriteStage, WriteVideoOptions,
    ffmpeg_available, write_av_with, write_video_with,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

fn skip_without_ffmpeg() -> bool {
    if ffmpeg_available() {
        false
    } else {
        eprintln!("skipping: ffmpeg/ffprobe not available");
        true
    }
}

#[test]
fn write_video_reports_progress_and_done() {
    if skip_without_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let out: PathBuf = dir.path().join("prog.mp4");
    let stages = Arc::new(Mutex::new(Vec::new()));
    let stages2 = Arc::clone(&stages);
    let last_frame = Arc::new(AtomicU64::new(0));
    let last2 = Arc::clone(&last_frame);

    let control =
        WriteControl::new()
            .with_max_in_flight(2)
            .with_progress(move |p: WriteProgress| {
                stages2.lock().unwrap().push(p.stage);
                if p.stage == WriteStage::Video {
                    last2.store(p.index, Ordering::SeqCst);
                }
            });

    let clip =
        ColorClip::new(Size::new(64, 48), Rgb8::GREEN, Duration::from_secs(0.25)).with_fps(12.0);
    write_video_with(
        &clip,
        &WriteVideoOptions::new(out.to_string_lossy(), 12.0).with_crf(28),
        &control,
    )
    .expect("write_video_with");

    assert!(out.is_file());
    assert!(last_frame.load(Ordering::SeqCst) > 0);
    let seen = stages.lock().unwrap().clone();
    assert!(seen.contains(&WriteStage::Video));
    assert!(seen.contains(&WriteStage::Done));
}

#[test]
fn write_av_streams_audio_and_muxes() {
    if skip_without_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let out: PathBuf = dir.path().join("av.mp4");
    let audio_hits = Arc::new(AtomicU64::new(0));
    let hits2 = Arc::clone(&audio_hits);
    let control = WriteControl::new().with_progress(move |p| {
        if p.stage == WriteStage::Audio {
            hits2.fetch_add(1, Ordering::SeqCst);
        }
    });

    let video =
        ColorClip::new(Size::new(64, 48), Rgb8::BLUE, Duration::from_secs(0.3)).with_fps(10.0);
    let audio = SilenceClip::new(
        AudioFormat {
            sample_rate: 16_000,
            layout: SampleLayout::Mono,
        },
        Duration::from_secs(0.3),
    );
    write_av_with(
        &video,
        &audio,
        &WriteVideoOptions::new(out.to_string_lossy(), 10.0).with_crf(28),
        &control,
    )
    .expect("write_av_with");
    assert!(out.is_file());
    assert!(std::fs::metadata(&out).unwrap().len() > 0);
    assert!(audio_hits.load(Ordering::SeqCst) >= 1);
}

#[test]
fn pre_cancelled_write_errors() {
    if skip_without_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let out: PathBuf = dir.path().join("cancel.mp4");
    let token = CancelToken::new();
    token.cancel();
    let control = WriteControl::new().with_cancel(token);
    let clip =
        ColorClip::new(Size::new(32, 24), Rgb8::RED, Duration::from_secs(0.5)).with_fps(12.0);
    let err = write_video_with(
        &clip,
        &WriteVideoOptions::new(out.to_string_lossy(), 12.0).with_crf(28),
        &control,
    );
    assert!(matches!(err, Err(IoError::Cancelled)));
}
