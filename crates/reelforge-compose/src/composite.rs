//! Multi-layer video compositing.

use crate::blit::{blit_over, solid_canvas};
use crate::layer::CompositeLayer;
use crate::{ComposeError, Result};
use reelforge_core::{CoreError, Duration, Frame, Rgb8, Size, Time, VideoClip};
use std::sync::Arc;

/// Stacked video composition: background + ordered layers with positions.
///
/// Layers are drawn in ascending [`CompositeLayer::layer_index`] order (higher
/// index on top). Each layer is active for
/// `[start, start + clip.duration())` on the composite timeline.
#[derive(Clone)]
pub struct CompositeVideo {
    size: Size,
    duration: Duration,
    background: Rgb8,
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
            layers,
            fps,
        })
    }

    /// Canvas size.
    #[must_use]
    pub const fn canvas_size(&self) -> Size {
        self.size
    }

    /// Background fill color.
    #[must_use]
    pub const fn background(&self) -> Rgb8 {
        self.background
    }

    /// Layers in draw order (low → high).
    #[must_use]
    pub fn layers(&self) -> &[CompositeLayer] {
        &self.layers
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

        let mut canvas = solid_canvas(self.size, self.background)
            .map_err(|e| CoreError::invalid_frame(format!("composite canvas: {e}")))?;

        for layer in &self.layers {
            if !layer.active_at(t) {
                continue;
            }
            let local = layer.local_time(t);
            let frame = layer.clip.frame_at(local)?;
            let mask = layer.clip.mask_at(local)?;
            let (ox, oy) = layer.position.resolve(self.size, frame.size());
            blit_over(&mut canvas, &frame, ox, oy, layer.opacity, mask.as_ref())
                .map_err(|e| CoreError::invalid_frame(format!("blit: {e}")))?;
        }

        Ok(canvas)
    }
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
}
