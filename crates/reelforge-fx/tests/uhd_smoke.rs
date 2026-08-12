//! Smoke: 4K / 8K frames through common effect chains (no encode).

use reelforge_core::{ColorClip, Duration, Size, Time, VideoClip, VideoEffect};
use reelforge_fx::{BlackAndWhite, Crop, FadeIn, Resize, Rotate};
use std::sync::Arc;

fn sample(size: Size) -> Arc<dyn VideoClip> {
    Arc::new(
        ColorClip::new(
            size,
            reelforge_core::Rgb8::new(40, 80, 160),
            Duration::from_secs(1.0),
        )
        .with_fps(24.0),
    )
}

#[test]
fn uhd_4k_chain_frame_at() {
    let base = sample(Size::UHD_4K);
    assert_eq!(base.size(), Size::new(3840, 2160));
    let cropped = Crop::new(100, 50, 3200, 1800).apply(base).unwrap();
    let resized = Resize::to(Size::HD_1080).apply(cropped).unwrap();
    let faded = FadeIn::new(Duration::from_secs(0.25))
        .apply(resized)
        .unwrap();
    let gray = BlackAndWhite.apply(faded).unwrap();
    let f = gray.frame_at(Time::from_secs(0.1)).unwrap();
    assert_eq!(f.size(), Size::HD_1080);
    assert_eq!(f.data().len(), 1920 * 1080 * 3);
}

#[test]
fn uhd_8k_solid_and_downscale() {
    let base = sample(Size::UHD_8K);
    assert_eq!(base.size(), Size::new(7680, 4320));
    // One solid sample (Arc-cached) then resize path.
    let f = base.frame_at(Time::ZERO).unwrap();
    assert_eq!(f.size(), Size::UHD_8K);
    assert_eq!(f.data().len(), 7680 * 4320 * 3);

    let small = Resize::to(Size::HD_720).apply(base).unwrap();
    let out = small.frame_at(Time::ZERO).unwrap();
    assert_eq!(out.size(), Size::HD_720);
}

#[test]
fn uhd_4k_free_rotate_keeps_canvas() {
    let base = sample(Size::UHD_4K);
    let spun = Rotate::degrees(15.0).apply(base).unwrap();
    assert_eq!(spun.size(), Size::UHD_4K);
    let f = spun.frame_at(Time::ZERO).unwrap();
    assert_eq!(f.size(), Size::UHD_4K);
}
