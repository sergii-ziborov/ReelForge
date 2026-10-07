//! Multi-layer video compositing.

use crate::blit::{blit_over, blit_over_premul, solid_canvas};
use crate::layer::CompositeLayer;
use crate::{ComposeError, Result};
use reelforge_core::{
    AlphaMode, CoreError, Duration, Frame, FrameFormat, Mask, Rgb8, Size, Time, VideoClip,
};
use std::sync::Arc;

/// Stacked video composition: background + ordered layers with positions.
///
/// Layers are drawn in ascending [`CompositeLayer::layer_index`] order (higher
/// index on top). Each layer is active for
/// `[start, start + clip.duration())` on the composite timeline.
///
/// A color background returns RGB. [`CompositeVideo::transparent`] returns
/// premultiplied RGBA, so a group of parts can sit inside another scene
/// without gaining an opaque rectangle.
#[derive(Clone)]
pub struct CompositeVideo {
    size: Size,
    duration: Duration,
    background: Rgb8,
    /// Uncovered pixels stay transparent instead of [`Self::background`].
    transparent: bool,
    layers: Vec<CompositeLayer>,
    fps: Option<f64>,
}

impl CompositeVideo {
    /// Build a composite with default black background.
    ///
    /// Duration is the maximum of `layer.start + layer.clip.duration()`.
    /// FPS is the maximum FPS reported by any layer, if any.
    ///
    /// # Errors
    ///
    /// Returns [`ComposeError`] when size is invalid, no layers are given, or
    /// duration would be zero.
    pub fn new(size: Size, layers: Vec<CompositeLayer>) -> Result<Self> {
        Self::with_background(size, Rgb8::BLACK, layers)
    }

    /// Build a composite with an explicit background color.
    ///
    /// # Errors
    ///
    /// Returns [`ComposeError`] when size is invalid, no layers are given, or
    /// duration would be zero.
    pub fn with_background(
        size: Size,
        background: Rgb8,
        mut layers: Vec<CompositeLayer>,
    ) -> Result<Self> {
        size.require_positive().map_err(ComposeError::from)?;
        if layers.is_empty() {
            return Err(ComposeError::Message(
                "composite requires at least one layer".into(),
            ));
        }

        layers.sort_by_key(|l| l.layer_index);

        let mut duration = Duration::ZERO;
        let mut fps = None;
        for layer in &layers {
            if layer.start.as_secs() < 0.0 || !layer.start.as_secs().is_finite() {
                return Err(ComposeError::Message(
                    "layer start must be finite and >= 0".into(),
                ));
            }
            if !layer.clip.duration().is_positive() {
                return Err(ComposeError::Message(
                    "each layer clip must have positive duration".into(),
                ));
            }
            duration = duration.max(layer.contributes_duration());
            if let Some(f) = layer.clip.fps() {
                fps = Some(fps.map_or(f, |cur: f64| cur.max(f)));
            }
        }
        if !duration.is_positive() {
            return Err(ComposeError::Message(
                "composite duration must be positive".into(),
            ));
        }

        Ok(Self {
            size,
            duration,
            background,
            transparent: false,
            layers,
            fps,
        })
    }

    /// Build a group whose uncovered pixels stay transparent.
    ///
    /// The sampled frame is premultiplied RGBA. Nest it in another composite
    /// to keep the holes.
    ///
    /// # Errors
    ///
    /// Returns [`ComposeError`] when size is invalid, no layers are given, or
    /// duration would be zero.
    pub fn transparent(size: Size, layers: Vec<CompositeLayer>) -> Result<Self> {
        let mut video = Self::with_background(size, Rgb8::BLACK, layers)?;
        video.transparent = true;
        Ok(video)
    }

    /// Canvas size.
    #[must_use]
    pub const fn canvas_size(&self) -> Size {
        self.size
    }

    /// Background fill color.
    ///
    /// Ignored when [`Self::is_transparent`] is set.
    #[must_use]
    pub const fn background(&self) -> Rgb8 {
        self.background
    }

    /// Whether uncovered pixels stay transparent.
    #[must_use]
    pub const fn is_transparent(&self) -> bool {
        self.transparent
    }

    /// Layers in draw order (low → high).
    #[must_use]
    pub fn layers(&self) -> &[CompositeLayer] {
        &self.layers
    }

    /// Keep the canvas up through `until` when that is later than the last layer.
    ///
    /// Frames after the last layer stay the background color, or transparent
    /// when this group has no background. A shorter `until` does not trim layers.
    #[must_use]
    pub fn hold_until(mut self, until: Duration) -> Self {
        if until.as_secs() > self.duration.as_secs() {
            self.duration = until;
        }
        self
    }
}

impl VideoClip for CompositeVideo {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn size(&self) -> Size {
        self.size
    }

    fn fps(&self) -> Option<f64> {
        self.fps
    }

    fn frame_at(&self, t: Time) -> reelforge_core::Result<Frame> {
        if t.as_secs() < 0.0 || t.as_secs() >= self.duration.as_secs() {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }

        if self.transparent {
            return self.paint_transparent(t);
        }
        self.paint_rgb(t)
    }
}

impl CompositeVideo {
    fn paint_rgb(&self, t: Time) -> reelforge_core::Result<Frame> {
        let mut canvas = solid_canvas(self.size, self.background)
            .map_err(|e| CoreError::invalid_frame(format!("composite canvas: {e}")))?;
        self.paint_layers(t, &mut canvas, false)?;
        Ok(canvas)
    }

    fn paint_transparent(&self, t: Time) -> reelforge_core::Result<Frame> {
        let mut canvas = Frame::zeros(self.size, FrameFormat::Rgba8)
            .map_err(|e| CoreError::invalid_frame(format!("composite canvas: {e}")))?
            .with_alpha_mode(AlphaMode::Premultiplied)?;
        self.paint_layers(t, &mut canvas, true)?;
        Ok(canvas)
    }

    fn paint_layers(
        &self,
        t: Time,
        canvas: &mut Frame,
        premul: bool,
    ) -> reelforge_core::Result<()> {
        for layer in &self.layers {
            if !layer.active_at(t) {
                continue;
            }
            let local = layer.local_time(t);
            let mut frame = layer.clip.frame_at(local)?;
            let mut mask = layer.clip.mask_at(local)?;
            if let Some(mask) = &mask
                && mask.size() != frame.size()
            {
                return Err(CoreError::invalid_frame(
                    "composite refuses a coverage mask that does not match the picture",
                ));
            }
            if let Some(sample) = &layer.scale_at {
                let factor = sample(t);
                if (factor - 1.0).abs() > 1.0e-4 {
                    frame = nearest_scale(&frame, factor)?;
                    if let Some(coverage) = mask.as_ref() {
                        mask = Some(nearest_scale_mask(coverage, frame.size())?);
                    }
                }
            }
            let (ox, oy) = if let Some(sample) = &layer.position_at {
                sample(t)
            } else {
                layer.position.resolve(self.size, frame.size())
            };
            let opacity = layer
                .opacity_at
                .as_ref()
                .map_or(layer.opacity, |sample| sample(t).clamp(0.0, 1.0));
            let painted = if premul {
                blit_over_premul(canvas, &frame, ox, oy, opacity, mask.as_ref())
            } else {
                blit_over(canvas, &frame, ox, oy, opacity, mask.as_ref())
            };
            painted.map_err(|e| CoreError::invalid_frame(format!("blit: {e}")))?;
        }
        Ok(())
    }
}

#[allow(clippy::similar_names)] // source and destination width/height
fn nearest_scale(src: &Frame, factor: f32) -> reelforge_core::Result<Frame> {
    if !factor.is_finite() || factor <= 0.0 {
        return Err(CoreError::invalid_frame(
            "composite refuses a non-positive scale",
        ));
    }
    let src_w = src.size().width;
    let src_h = src.size().height;
    let dst_w = scaled_dim(src_w, factor)?;
    let dst_h = scaled_dim(src_h, factor)?;
    if dst_w == src_w && dst_h == src_h {
        return Ok(src.clone());
    }
    let bpp = src.format().bytes_per_pixel();
    let src_w_us = usize::try_from(src_w)
        .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
    let dst_w_us = usize::try_from(dst_w)
        .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
    let dst_h_us = usize::try_from(dst_h)
        .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
    let dst_len = dst_w_us
        .checked_mul(dst_h_us)
        .and_then(|pixels| pixels.checked_mul(bpp))
        .ok_or_else(|| CoreError::invalid_frame("composite refuses an oversized scale"))?;
    let src_data = src.data();
    let mut dst = vec![0_u8; dst_len];
    for y in 0..dst_h {
        let sy = usize::try_from((u64::from(y) * u64::from(src_h)) / u64::from(dst_h))
            .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
        let y_us = usize::try_from(y)
            .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
        for x in 0..dst_w {
            let sx = usize::try_from((u64::from(x) * u64::from(src_w)) / u64::from(dst_w))
                .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
            let x_us = usize::try_from(x)
                .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
            let from = (sy * src_w_us + sx) * bpp;
            let to = (y_us * dst_w_us + x_us) * bpp;
            dst[to..to + bpp].copy_from_slice(&src_data[from..from + bpp]);
        }
    }
    Frame::from_raw(Size::new(dst_w, dst_h), src.format(), dst)?.with_alpha_mode(src.alpha_mode())
}

#[allow(clippy::similar_names)]
fn nearest_scale_mask(src: &Mask, dst: Size) -> reelforge_core::Result<Mask> {
    if src.size() == dst {
        return Ok(src.clone());
    }
    let src_w = src.size().width;
    let src_h = src.size().height;
    let dst_w = dst.width;
    let dst_h = dst.height;
    let src_w_us = usize::try_from(src_w)
        .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
    let dst_w_us = usize::try_from(dst_w)
        .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
    let dst_h_us = usize::try_from(dst_h)
        .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
    let len = dst_w_us
        .checked_mul(dst_h_us)
        .ok_or_else(|| CoreError::invalid_frame("composite refuses an oversized scale"))?;
    let data = src.data();
    let mut out = vec![0.0_f32; len];
    for y in 0..dst_h {
        let sy = usize::try_from((u64::from(y) * u64::from(src_h)) / u64::from(dst_h))
            .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
        let y_us = usize::try_from(y)
            .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
        for x in 0..dst_w {
            let sx = usize::try_from((u64::from(x) * u64::from(src_w)) / u64::from(dst_w))
                .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
            let x_us = usize::try_from(x)
                .map_err(|_| CoreError::invalid_frame("composite refuses an oversized scale"))?;
            let sample = data[sy * src_w_us + sx];
            if !sample.is_finite() {
                return Err(CoreError::invalid_frame(
                    "composite refuses a non-finite coverage mask",
                ));
            }
            out[y_us * dst_w_us + x_us] = sample.clamp(0.0, 1.0);
        }
    }
    Mask::from_raw(dst, out)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn scaled_dim(src: u32, factor: f32) -> reelforge_core::Result<u32> {
    let wide = f64::from(src) * f64::from(factor);
    if !wide.is_finite() || !(1.0..=4096.0).contains(&wide) {
        return Err(CoreError::invalid_frame(
            "composite refuses a scale outside 1..=4096 pixels",
        ));
    }
    Ok(wide.round() as u32)
}

/// Compose layers onto a canvas; returns a trait object.
///
/// # Errors
///
/// Propagates [`CompositeVideo::new`] errors.
pub fn composite_video(size: Size, layers: Vec<CompositeLayer>) -> Result<Arc<dyn VideoClip>> {
    Ok(Arc::new(CompositeVideo::new(size, layers)?))
}

/// Compose layers with a background color.
///
/// # Errors
///
/// Propagates [`CompositeVideo::with_background`] errors.
pub fn composite_video_with_background(
    size: Size,
    background: Rgb8,
    layers: Vec<CompositeLayer>,
) -> Result<Arc<dyn VideoClip>> {
    Ok(Arc::new(CompositeVideo::with_background(
        size, background, layers,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Position};

    #[test]
    fn layer_order_top_wins() {
        let bottom = CompositeLayer::new(Arc::new(ColorClip::new(
            Size::new(4, 4),
            Rgb8::RED,
            Duration::from_secs(1.0),
        )))
        .with_layer_index(0);
        let top = CompositeLayer::new(Arc::new(ColorClip::new(
            Size::new(4, 4),
            Rgb8::BLUE,
            Duration::from_secs(1.0),
        )))
        .with_layer_index(1);
        let comp = CompositeVideo::new(Size::new(4, 4), vec![top, bottom]).unwrap();
        let f = comp.frame_at(Time::ZERO).unwrap();
        assert_eq!(&f.data()[0..3], &[0, 0, 255]);
    }

    #[test]
    fn position_offset() {
        let layer = CompositeLayer::new(Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::GREEN,
            Duration::from_secs(1.0),
        )))
        .with_position(Position::absolute(1, 1));
        let comp = CompositeVideo::new(Size::new(4, 4), vec![layer]).unwrap();
        let f = comp.frame_at(Time::ZERO).unwrap();
        // (0,0) background black
        assert_eq!(&f.data()[0..3], &[0, 0, 0]);
        // (1,1) green
        let i = (4 + 1) * 3;
        assert_eq!(&f.data()[i..i + 3], &[0, 255, 0]);
    }

    #[test]
    fn start_time_delays_layer() {
        let layer = CompositeLayer::new(Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        )))
        .with_start(Time::from_secs(1.0));
        let comp = CompositeVideo::new(Size::new(2, 2), vec![layer]).unwrap();
        assert!((comp.duration().as_secs() - 2.0).abs() < 1e-9);
        let early = comp.frame_at(Time::from_secs(0.5)).unwrap();
        assert_eq!(&early.data()[0..3], &[0, 0, 0]);
        let late = comp.frame_at(Time::from_secs(1.5)).unwrap();
        assert_eq!(&late.data()[0..3], &[255, 255, 255]);
    }

    #[test]
    fn opacity_blends() {
        let layer = CompositeLayer::new(Arc::new(ColorClip::new(
            Size::new(1, 1),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        )))
        .with_opacity(0.5);
        let comp =
            CompositeVideo::with_background(Size::new(1, 1), Rgb8::BLACK, vec![layer]).unwrap();
        let f = comp.frame_at(Time::ZERO).unwrap();
        assert!(f.data()[0] > 100 && f.data()[0] < 160);
    }

    #[test]
    fn zero_opacity_shows_background() {
        let layer = CompositeLayer::new(Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        )))
        .with_opacity(0.0);
        let comp =
            CompositeVideo::with_background(Size::new(2, 2), Rgb8::BLACK, vec![layer]).unwrap();
        let f0 = comp.frame_at(Time::ZERO).unwrap();
        assert_eq!(&f0.data()[0..3], &[0, 0, 0]);
    }

    #[test]
    fn hold_until_keeps_background_after_the_last_layer() {
        let layer = CompositeLayer::new(Arc::new(ColorClip::new(
            Size::new(1, 1),
            Rgb8::RED,
            Duration::from_secs(1.0),
        )));
        let held = CompositeVideo::new(Size::new(1, 1), vec![layer])
            .unwrap()
            .hold_until(Duration::from_secs(2.5));
        assert!((held.duration().as_secs() - 2.5).abs() < 1e-9);
        let tail = held.frame_at(Time::from_secs(2.0)).unwrap();
        assert_eq!(&tail.data()[0..3], &[0, 0, 0]);

        let layer = CompositeLayer::new(Arc::new(ColorClip::new(
            Size::new(1, 1),
            Rgb8::RED,
            Duration::from_secs(1.0),
        )));
        let same = CompositeVideo::new(Size::new(1, 1), vec![layer])
            .unwrap()
            .hold_until(Duration::from_secs(0.4));
        assert!((same.duration().as_secs() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn transparent_group_keeps_a_hole_over_blue() {
        use reelforge_core::{AlphaMode, FrameFormat};
        let part = Arc::new(ColorClip::new(
            Size::new(1, 1),
            Rgb8::RED,
            Duration::from_secs(1.0),
        ));
        let group =
            CompositeVideo::transparent(Size::new(2, 1), vec![CompositeLayer::new(part)]).unwrap();
        assert!(group.is_transparent());
        let grouped = group.frame_at(Time::ZERO).unwrap();
        assert_eq!(grouped.format(), FrameFormat::Rgba8);
        assert_eq!(grouped.alpha_mode(), AlphaMode::Premultiplied);
        assert_eq!(&grouped.data()[0..4], &[255, 0, 0, 255]);
        assert_eq!(&grouped.data()[4..8], &[0, 0, 0, 0]);

        let nested = CompositeVideo::with_background(
            Size::new(2, 1),
            Rgb8::BLUE,
            vec![CompositeLayer::new(Arc::new(group))],
        )
        .unwrap();
        let painted = nested.frame_at(Time::ZERO).unwrap();
        assert_eq!(&painted.data()[0..3], &[255, 0, 0]);
        assert_eq!(&painted.data()[3..6], &[0, 0, 255]);
    }

    struct QuarterBar;

    impl VideoClip for QuarterBar {
        fn duration(&self) -> Duration {
            Duration::from_secs(1.0)
        }

        fn size(&self) -> Size {
            Size::new(2, 1)
        }

        fn frame_at(&self, _: Time) -> reelforge_core::Result<Frame> {
            let mut frame = Frame::zeros(Size::new(2, 1), FrameFormat::Rgb8)?;
            let data = frame.data_mut();
            data[0] = 255;
            data[1] = 255;
            data[2] = 255;
            Ok(frame)
        }

        fn mask_at(&self, _: Time) -> reelforge_core::Result<Option<Mask>> {
            Ok(Some(Mask::from_raw(Size::new(2, 1), vec![0.25, 0.0])?))
        }
    }

    #[test]
    fn scale_moves_coverage_with_the_picture() {
        let layer =
            CompositeLayer::new(Arc::new(QuarterBar)).with_scale_at(Arc::new(|_: Time| 2.0));
        let on_black = CompositeVideo::new(Size::new(4, 2), vec![layer]).unwrap();
        let dark = on_black.frame_at(Time::ZERO).unwrap();
        assert_eq!(&dark.data()[0..3], &[64, 64, 64]);
        assert_eq!(&dark.data()[6..9], &[0, 0, 0]);
        assert_eq!(on_black.frame_at(Time::ZERO).unwrap().data(), dark.data());

        let layer =
            CompositeLayer::new(Arc::new(QuarterBar)).with_scale_at(Arc::new(|_: Time| 2.0));
        let on_white =
            CompositeVideo::with_background(Size::new(4, 2), Rgb8::WHITE, vec![layer]).unwrap();
        let light = on_white.frame_at(Time::ZERO).unwrap();
        assert!(light.data().iter().all(|channel| *channel == 255));
    }
}
