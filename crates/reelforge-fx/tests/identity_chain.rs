//! Identity and volume pipeline smoke tests.

use reelforge_core::{
    AudioClip, AudioFormat, ColorClip, Duration, Rgb8, SilenceClip, Size, Time, VideoClip,
    VideoEffect, apply_audio_effects, apply_video_effects,
};
use reelforge_fx::{Crop, Identity, MultiplyColor, Resize, Rotate, VolumeGain};
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
fn spatial_and_color_noops_keep_the_original_clip() {
    let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
        Size::new(4, 4),
        Rgb8::BLUE,
        Duration::from_secs(0.5),
    ));
    let effects: Vec<Box<dyn VideoEffect>> = vec![
        Box::new(Crop::new(0, 0, 4, 4)),
        Box::new(Resize::to(Size::new(4, 4))),
        Box::new(Rotate::degrees(0.0)),
        Box::new(MultiplyColor::new(1.0)),
    ];
    for effect in effects {
        let output = effect.apply(Arc::clone(&clip)).unwrap();
        // Retaining the original also preserves native surfaces, PTS, IDs and
        // color tags that a packed-pixel wrapper cannot reconstruct.
        assert!(Arc::ptr_eq(&output, &clip));
    }
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
