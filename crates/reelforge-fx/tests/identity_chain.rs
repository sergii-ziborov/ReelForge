//! Identity and volume pipeline smoke tests.

use reelforge_core::{
    AudioClip, AudioFormat, ColorClip, Duration, Rgb8, SilenceClip, Size, Time, VideoClip,
    VideoEffect, apply_audio_effects, apply_video_effects,
};
use reelforge_fx::{Identity, VolumeGain};
use std::sync::Arc;

#[test]
fn identity_video_passthrough() {
    let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
        Size::new(4, 4),
        Rgb8::BLUE,
        Duration::from_secs(0.5),
    ));
    let effects: Vec<Arc<dyn VideoEffect>> = vec![Arc::new(Identity)];
    let out = apply_video_effects(clip, &effects).unwrap();
    let _ = out.frame_at(Time::ZERO).unwrap();
}

#[test]
fn volume_and_identity_audio() {
    let clip: Arc<dyn AudioClip> = Arc::new(SilenceClip::new(
        AudioFormat::STEREO_48K,
        Duration::from_secs(0.1),
    ));
    let effects = vec![
        Arc::new(Identity) as Arc<dyn reelforge_core::AudioEffect>,
        Arc::new(VolumeGain::new(0.25)),
    ];
    let out = apply_audio_effects(clip, &effects).unwrap();
    let buf = out.samples_at(Time::ZERO, 16).unwrap();
    assert_eq!(buf.frame_count(), 16);
}
