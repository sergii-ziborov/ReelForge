//! Smoke the e2e harness (tiny lavfi case). Skips without host ffmpeg.
#![allow(clippy::print_stderr)]

use reelforge_io::{E2eCase, ffmpeg_available, percentile, run_e2e_case};

#[test]
fn percentile_is_stable() {
    let s = [10.0, 20.0, 30.0];
    assert!((percentile(&s, 0.5) - 20.0).abs() < 1e-9);
}

#[test]
fn smoke_privacy_or_skip() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let report = run_e2e_case(&E2eCase::smoke(), 1, dir.path(), None).expect("run");
    if let Some(why) = &report.skipped {
        eprintln!("skipped: {why}");
        return;
    }
    assert!(report.p50_ms > 0.0);
    assert!(report.output_bytes > 0);
}
