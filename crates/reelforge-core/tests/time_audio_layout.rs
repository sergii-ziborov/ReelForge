//! Extra coverage for core pure types.

use reelforge_core::{
    Anchor, AudioBuffer, AudioFormat, Duration, Position, SampleLayout, Size, Time, TimeRange,
};

#[test]
fn time_ops() {
    let t = Time::try_from_secs(1.5).unwrap();
    assert!((t.as_secs() - 1.5).abs() < f64::EPSILON);
    assert!(Time::try_from_secs(f64::NAN).is_err());
    let d = Duration::try_from_secs(2.0).unwrap();
    assert!(d.is_positive());
    assert!((d.scale(0.5).as_secs() - 1.0).abs() < f64::EPSILON);
    assert!((d.max(Duration::from_secs(3.0)).as_secs() - 3.0).abs() < f64::EPSILON);
    assert!((d.min(Duration::from_secs(1.0)).as_secs() - 1.0).abs() < f64::EPSILON);
    let r = TimeRange::from_duration(Duration::from_secs(2.0)).unwrap();
    assert!(r.contains(Time::ZERO));
    assert!(!r.contains(Time::from_secs(2.0)));
    assert!((r.to_absolute(Time::from_secs(0.5)).as_secs() - 0.5).abs() < f64::EPSILON);
}

#[test]
fn audio_format_helpers() {
    let fmt = AudioFormat::new(44_100, SampleLayout::Mono).unwrap();
    assert_eq!(fmt.channels(), 1);
    assert!(AudioFormat::new(0, SampleLayout::Stereo).is_err());
    let n = fmt.frames_for_duration(Duration::from_secs(0.5));
    assert_eq!(n, 22_050);
    let buf = AudioBuffer::silence(fmt, 10).unwrap();
    assert_eq!(buf.frame_count(), 10);
    let mut loud = AudioBuffer::from_interleaved(fmt, vec![0.5; 10]).unwrap();
    loud.apply_gain(2.0);
    assert!((loud.samples()[0] - 1.0).abs() < f32::EPSILON);
}

#[test]
fn position_anchors() {
    let parent = Size::new(100, 50);
    let child = Size::new(10, 10);
    let (x, y) = Position::anchored(Anchor::BottomRight, 0, 0).resolve(parent, child);
    assert_eq!((x, y), (90, 40));
    assert!(Size::new(0, 1).require_positive().is_err());
    assert!(Size::new(4, 4).is_even());
}
