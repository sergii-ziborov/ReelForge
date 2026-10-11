//! Continuous source-space deformation with alpha-correct filtered sampling.

use reelforge_core::{
    AlphaMode, CoreError, Duration, Frame, FrameFormat, Mask, Result, Size, Time, VideoClip,
    VideoEffect,
};
use std::sync::Arc;

/// Inverse sampling offsets in source pixels: `(time_seconds, x, y) -> (dx, dy)`.
/// A zero offset leaves a pixel fixed. Positive dx reads farther right.
pub type DisplacementField = Arc<dyn Fn(f64, f64, f64) -> (f64, f64) + Send + Sync>;

/// Warp a clip with a continuous bounded field. Useful for pinned foliage,
/// fabric flutter, water ripples and heat haze; it does not simulate 3D cloth.
/// Fields can incorporate masks and share the scene's environmental clock.
#[derive(Clone)]
pub struct Displacement {
    /// Source-space field evaluated independently of output frame rate.
    pub field: DisplacementField,
    /// Maximum absolute offset on either axis. Exceeding it fails sampling.
    pub max_offset: f64,
}
impl Displacement {
    /// Construct a deformation; parameters are checked when applying it.
    #[must_use]
    pub fn new<F>(max_offset: f64, field: F) -> Self
    where
        F: Fn(f64, f64, f64) -> (f64, f64) + Send + Sync + 'static,
    {
        Self {
            field: Arc::new(field),
            max_offset,
        }
    }
}
impl VideoEffect for Displacement {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        if !self.max_offset.is_finite() || !(0.0..=4096.0).contains(&self.max_offset) {
            return Err(CoreError::invalid_frame("invalid displacement bound"));
        }
        Ok(Arc::new(Warped {
            inner: clip,
            effect: self.clone(),
        }))
    }
}
struct Warped {
    inner: Arc<dyn VideoClip>,
    effect: Displacement,
}
impl Warped {
    fn coordinate(&self, t: Time, x: u32, y: u32) -> Result<(f64, f64)> {
        let (dx, dy) = (self.effect.field)(t.as_secs(), f64::from(x), f64::from(y));
        if !dx.is_finite()
            || !dy.is_finite()
            || dx.abs() > self.effect.max_offset
            || dy.abs() > self.effect.max_offset
        {
            return Err(CoreError::invalid_frame(
                "displacement field exceeded its finite bound",
            ));
        }
        Ok((f64::from(x) + dx, f64::from(y) + dy))
    }
}
impl VideoClip for Warped {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }
    fn size(&self) -> Size {
        self.inner.size()
    }
    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }
    fn sample_times(&self) -> Option<Vec<Time>> {
        self.inner.sample_times()
    }
    fn requires_frame_grid(&self) -> bool {
        self.inner.requires_frame_grid()
    }
    fn frame_at(&self, t: Time) -> Result<Frame> {
        let source = self.inner.frame_at(t)?;
        let size = source.size();
        let bpp = source.format().bytes_per_pixel();
        let mut out = vec![0; source.data().len()];
        for y in 0..size.height {
            for x in 0..size.width {
                let (sx, sy) = self.coordinate(t, x, y)?;
                let pixel = sample_displaced(&source, sx, sy)?;
                let i = (y as usize * size.width as usize + x as usize) * bpp;
                out[i..i + bpp].copy_from_slice(&pixel[..bpp]);
            }
        }
        Frame::from_raw(size, source.format(), out)?.with_alpha_mode(source.alpha_mode())
    }
    #[allow(clippy::cast_possible_truncation)] // Bilinear coverage stays within 0..=1.
    fn mask_at(&self, t: Time) -> Result<Option<Mask>> {
        let Some(mask) = crate::mask_geom::picture_mask(self.inner.as_ref(), t)? else {
            return Ok(None);
        };
        let size = mask.size();
        let mut out = vec![0.0; mask.data().len()];
        for y in 0..size.height {
            for x in 0..size.width {
                let (sx, sy) = self.coordinate(t, x, y)?;
                let mut value = 0.0;
                for (px, py, weight) in taps(sx, sy, size) {
                    value +=
                        f64::from(mask.data()[py as usize * size.width as usize + px as usize])
                            * weight;
                }
                out[y as usize * size.width as usize + x as usize] = value as f32;
            }
        }
        Ok(Some(Mask::from_raw(size, out)?))
    }
}

/// Bilinear source sample shared with sparse regional renderers. Exterior taps
/// are transparent for RGBA and black for RGB. Straight alpha is filtered in
/// premultiplied space; premultiplied inputs keep their representation.
///
/// # Errors
/// Returns an error for non-finite coordinates.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Explicitly rounded/clamped byte quantization.
pub fn sample_displaced(source: &Frame, x: f64, y: f64) -> Result<[u8; 4]> {
    if !x.is_finite() || !y.is_finite() {
        return Err(CoreError::invalid_frame("non-finite deformation sample"));
    }
    let bpp = source.format().bytes_per_pixel();
    let rgba = source.format() == FrameFormat::Rgba8;
    let straight = rgba && source.alpha_mode() == AlphaMode::Straight;
    let mut channels = [0.0; 4];
    for (px, py, weight) in taps(x, y, source.size()) {
        let i = (py as usize * source.size().width as usize + px as usize) * bpp;
        let alpha = if rgba {
            f64::from(source.data()[i + 3]) / 255.0
        } else {
            1.0
        };
        for (c, value) in channels.iter_mut().enumerate().take(3) {
            *value += f64::from(source.data()[i + c]) * weight * if straight { alpha } else { 1.0 };
        }
        channels[3] += 255.0 * alpha * weight;
    }
    if straight && channels[3] > 0.0 {
        let alpha = channels[3] / 255.0;
        for value in channels.iter_mut().take(3) {
            *value /= alpha;
        }
    }
    Ok(channels.map(|v| v.round().clamp(0.0, 255.0) as u8))
}
#[allow(clippy::cast_possible_truncation)] // Coordinates are bounded by u32 dimensions before flooring.
fn taps(x: f64, y: f64, size: Size) -> impl Iterator<Item = (u32, u32, f64)> {
    let valid = x >= -1.0 && y >= -1.0 && x < f64::from(size.width) && y < f64::from(size.height);
    let ix = x.clamp(-2.0, f64::from(size.width)).floor() as i64;
    let iy = y.clamp(-2.0, f64::from(size.height)).floor() as i64;
    let fx = x - x.floor();
    let fy = y - y.floor();
    [
        (ix, iy, (1.0 - fx) * (1.0 - fy)),
        (ix + 1, iy, fx * (1.0 - fy)),
        (ix, iy + 1, (1.0 - fx) * fy),
        (ix + 1, iy + 1, fx * fy),
    ]
    .into_iter()
    .filter(move |(px, py, _)| {
        valid && *px >= 0 && *py >= 0 && *px < i64::from(size.width) && *py < i64::from(size.height)
    })
    .map(|(px, py, w)| (u32::try_from(px).unwrap(), u32::try_from(py).unwrap(), w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};
    struct Masked;
    impl VideoClip for Masked {
        fn duration(&self) -> Duration {
            Duration::from_secs(1.0)
        }
        fn size(&self) -> Size {
            Size::new(4, 1)
        }
        fn frame_at(&self, _: Time) -> Result<Frame> {
            Frame::from_raw(
                self.size(),
                FrameFormat::Rgb8,
                vec![0, 0, 0, 0, 0, 0, 255, 0, 0, 255, 0, 0],
            )
        }
        fn mask_at(&self, _: Time) -> Result<Option<Mask>> {
            Ok(Some(Mask::from_raw(self.size(), vec![0.0, 0.0, 1.0, 1.0])?))
        }
    }
    #[test]
    fn explicit_coverage_tracks_the_same_filtered_coordinates_as_picture() {
        let clip = Displacement::new(1.0, |_, _, _| (0.5, 0.0))
            .apply(Arc::new(Masked))
            .unwrap();
        let frame = clip.frame_at(Time::ZERO).unwrap();
        let mask = clip.mask_at(Time::ZERO).unwrap().unwrap();
        assert_eq!(frame.data()[3], 128);
        assert!((mask.data()[1] - 0.5).abs() < f32::EPSILON);
        assert!((mask.data()[3] - 0.5).abs() < f32::EPSILON);
    }
    #[test]
    fn sampling_avoids_transparent_colour_fringes_and_preserves_premultiplied_pixels() {
        let f = Frame::from_raw(
            Size::new(2, 1),
            FrameFormat::Rgba8,
            vec![255, 0, 0, 0, 0, 0, 255, 255],
        )
        .unwrap();
        assert_eq!(sample_displaced(&f, 0.5, 0.0).unwrap(), [0, 0, 255, 128]);
        let p = Frame::from_raw(
            Size::new(2, 1),
            FrameFormat::Rgba8,
            vec![0, 0, 0, 0, 0, 0, 128, 128],
        )
        .unwrap()
        .with_alpha_mode(AlphaMode::Premultiplied)
        .unwrap();
        assert_eq!(sample_displaced(&p, 0.5, 0.0).unwrap(), [0, 0, 64, 64]);
        assert!(sample_displaced(&f, f64::NAN, 0.0).is_err());
    }
    #[test]
    fn continuous_field_keeps_pinned_row_and_fails_an_unbounded_callback() {
        let source: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(4, 4),
            Rgb8::new(10, 20, 30),
            Duration::from_secs(2.0),
        ));
        let warp = Displacement::new(2.0, |t, _, y| (t * y / 3.0, 0.0))
            .apply(Arc::clone(&source))
            .unwrap();
        let f = warp.frame_at(Time::from_secs(0.5)).unwrap();
        assert_eq!(
            &f.data()[..12],
            &source.frame_at(Time::ZERO).unwrap().data()[..12]
        );
        assert_eq!(warp.frame_at(Time::from_secs(0.5)).unwrap(), f);
        let bad = Displacement::new(1.0, |_, _, _| (2.0, 0.0))
            .apply(source)
            .unwrap();
        assert!(bad.frame_at(Time::ZERO).is_err());
    }
}
