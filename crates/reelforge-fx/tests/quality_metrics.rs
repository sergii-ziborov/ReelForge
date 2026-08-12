//! Quality metrics smoke: PSNR/SSIM on effect chains stay high for identity-ish ops.
#![allow(clippy::many_single_char_names)]

use reelforge_core::{ColorClip, Duration, Size, Time, VideoClip, VideoEffect, psnr_rgb, ssim_rgb};
use reelforge_fx::{GammaCorrection, Resize, ResizeFilter};
use std::sync::Arc;

#[test]
fn identity_resize_bicubic_solid_perfect() {
    let clip = Arc::new(ColorClip::new(
        Size::new(32, 32),
        reelforge_core::Rgb8::new(10, 20, 30),
        Duration::from_secs(0.5),
    ));
    let same = Resize::to(Size::new(32, 32))
        .with_filter(ResizeFilter::Bicubic)
        .apply(clip.clone())
        .unwrap();
    let a = clip.frame_at(Time::ZERO).unwrap();
    let b = same.frame_at(Time::ZERO).unwrap();
    assert!(psnr_rgb(&a, &b).unwrap().is_infinite());
    assert!((ssim_rgb(&a, &b).unwrap() - 1.0).abs() < 1e-9);
}

#[test]
fn mild_gamma_still_similar() {
    let clip = Arc::new(ColorClip::new(
        Size::new(64, 64),
        reelforge_core::Rgb8::new(80, 100, 120),
        Duration::from_secs(0.5),
    ));
    let g = GammaCorrection::new(0.95).apply(clip.clone()).unwrap();
    let a = clip.frame_at(Time::ZERO).unwrap();
    let b = g.frame_at(Time::ZERO).unwrap();
    let p = psnr_rgb(&a, &b).unwrap();
    let s = ssim_rgb(&a, &b).unwrap();
    assert!(p > 30.0, "psnr={p}");
    assert!(s > 0.95, "ssim={s}");
}
