//! Coverage that moves with the picture.
//!
//! Source space is the clip before crop, scale, and rotate. Crop space is the
//! kept rectangle. Output space is the clip after scale and rotate. Canvas
//! space adds the layer origin. [`source_pixel`] walks those steps backwards
//! with the same indexes as the pixel kernels.

use crate::scale::{ResizeFilter, build_axis_lerps, cubic_weights};
use reelforge_core::{CoreError, Mask, Result, Size, Time, VideoClip};

/// One step from a parent pixel space into the next one.
///
/// List steps from the source toward the canvas. [`source_pixel`] walks them
/// backwards. Nearest resize uses `(dst * src_len) / dst_len`, the same index
/// as [`crate::raster::resize_nearest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelMap {
    /// Parent `(x + left, y + top)` is kept as `(x, y)`.
    Crop {
        /// Left edge in the parent, in pixels.
        left: u32,
        /// Top edge in the parent, in pixels.
        top: u32,
        /// Width of the kept rectangle.
        width: u32,
        /// Height of the kept rectangle.
        height: u32,
    },
    /// Nearest resize from `src_*` pixels onto `dst_*` pixels.
    Nearest {
        /// Parent width.
        src_width: u32,
        /// Parent height.
        src_height: u32,
        /// Output width.
        dst_width: u32,
        /// Output height.
        dst_height: u32,
    },
    /// Half turn. Output `(x, y)` reads `(width - 1 - x, height - 1 - y)`.
    HalfTurn {
        /// Width, unchanged by the turn.
        width: u32,
        /// Height, unchanged by the turn.
        height: u32,
    },
    /// Clockwise quarter turn. Output size is `(src_height, src_width)`.
    QuarterTurn {
        /// Parent width.
        src_width: u32,
        /// Parent height.
        src_height: u32,
    },
    /// Output `(x, y)` sits on the canvas at `(x + origin_x, y + origin_y)`.
    Layer {
        /// Canvas column of output `(0, 0)`.
        origin_x: i32,
        /// Canvas row of output `(0, 0)`.
        origin_y: i32,
        /// Output width.
        width: u32,
        /// Output height.
        height: u32,
    },
}

/// Map a canvas or output pixel back to the source pixel it came from.
///
/// Returns [`None`] when the point falls outside a step. Filtered resizes are
/// not a single source pixel; use [`PixelMap::Nearest`] for an exact edit.
#[must_use]
pub fn source_pixel(steps: &[PixelMap], x: i32, y: i32) -> Option<(u32, u32)> {
    let mut x = x;
    let mut y = y;
    for step in steps.iter().rev() {
        (x, y) = invert_step(*step, x, y)?;
    }
    let x = u32::try_from(x).ok()?;
    let y = u32::try_from(y).ok()?;
    Some((x, y))
}

fn invert_step(step: PixelMap, x: i32, y: i32) -> Option<(i32, i32)> {
    match step {
        PixelMap::Crop {
            left,
            top,
            width,
            height,
        } => {
            let x = u32::try_from(x).ok()?;
            let y = u32::try_from(y).ok()?;
            if x >= width || y >= height {
                return None;
            }
            Some((i32::try_from(x + left).ok()?, i32::try_from(y + top).ok()?))
        }
        PixelMap::Nearest {
            src_width,
            src_height,
            dst_width,
            dst_height,
        } => {
            let x = u32::try_from(x).ok()?;
            let y = u32::try_from(y).ok()?;
            if dst_width == 0 || dst_height == 0 || x >= dst_width || y >= dst_height {
                return None;
            }
            let sx = (u64::from(x) * u64::from(src_width)) / u64::from(dst_width);
            let sy = (u64::from(y) * u64::from(src_height)) / u64::from(dst_height);
            Some((i32::try_from(sx).ok()?, i32::try_from(sy).ok()?))
        }
        PixelMap::HalfTurn { width, height } => {
            let x = u32::try_from(x).ok()?;
            let y = u32::try_from(y).ok()?;
            if width == 0 || height == 0 || x >= width || y >= height {
                return None;
            }
            Some((
                i32::try_from(width - 1 - x).ok()?,
                i32::try_from(height - 1 - y).ok()?,
            ))
        }
        PixelMap::QuarterTurn {
            src_width,
            src_height,
        } => {
            let x = u32::try_from(x).ok()?;
            let y = u32::try_from(y).ok()?;
            if src_width == 0 || src_height == 0 || x >= src_height || y >= src_width {
                return None;
            }
            let src_x = y;
            let src_y = src_height - 1 - x;
            Some((i32::try_from(src_x).ok()?, i32::try_from(src_y).ok()?))
        }
        PixelMap::Layer {
            origin_x,
            origin_y,
            width,
            height,
        } => {
            let x = x.checked_sub(origin_x)?;
            let y = y.checked_sub(origin_y)?;
            let x = u32::try_from(x).ok()?;
            let y = u32::try_from(y).ok()?;
            if x >= width || y >= height {
                return None;
            }
            Some((i32::try_from(x).ok()?, i32::try_from(y).ok()?))
        }
    }
}

pub(crate) fn picture_mask(clip: &dyn VideoClip, t: Time) -> Result<Option<Mask>> {
    let Some(mask) = clip.mask_at(t)? else {
        return Ok(None);
    };
    let picture = clip.size();
    if mask.size() != picture {
        return Err(CoreError::invalid_frame(format!(
            "mask {:?} does not match picture {picture:?}",
            mask.size()
        )));
    }
    Ok(Some(mask))
}

pub(crate) fn crop_mask(mask: &Mask, x: u32, y: u32, width: u32, height: u32) -> Result<Mask> {
    if width == 0 || height == 0 {
        return Err(CoreError::invalid_frame("crop size must be positive"));
    }
    let src = mask.size();
    if x.saturating_add(width) > src.width || y.saturating_add(height) > src.height {
        return Err(CoreError::invalid_frame(format!(
            "mask crop ({x},{y},{width}x{height}) exceeds {src:?}"
        )));
    }
    let data = mask.data();
    let sw = src.width as usize;
    let row = width as usize;
    let mut out = Vec::with_capacity(row * height as usize);
    for dy in 0..height as usize {
        let start = (y as usize + dy) * sw + x as usize;
        out.extend_from_slice(&data[start..start + row]);
    }
    Mask::from_raw(Size::new(width, height), out)
}

pub(crate) fn resize_mask(mask: &Mask, new_size: Size, filter: ResizeFilter) -> Result<Mask> {
    new_size.require_positive()?;
    if mask.size() == new_size {
        return Ok(mask.clone());
    }
    let samples = match filter {
        ResizeFilter::Nearest => resize_mask_nearest(mask, new_size),
        ResizeFilter::Bilinear => resize_mask_bilinear(mask, new_size),
        ResizeFilter::Bicubic => resize_mask_bicubic(mask, new_size),
    };
    Mask::from_raw(new_size, samples)
}

fn resize_mask_nearest(mask: &Mask, new_size: Size) -> Vec<f32> {
    let src = mask.size();
    let data = mask.data();
    let sw = src.width as usize;
    let sh = src.height as usize;
    let dw = new_size.width as usize;
    let dh = new_size.height as usize;
    let mut out = vec![0.0_f32; dw * dh];
    for dy in 0..dh {
        let sy = (dy * sh) / dh;
        for dx in 0..dw {
            let sx = (dx * sw) / dw;
            out[dy * dw + dx] = data[sy * sw + sx];
        }
    }
    out
}

fn resize_mask_bilinear(mask: &Mask, new_size: Size) -> Vec<f32> {
    let src = mask.size();
    let data = mask.data();
    let sw = src.width as usize;
    let sh = src.height as usize;
    let dw = new_size.width as usize;
    let dh = new_size.height as usize;
    let x_lerps = build_axis_lerps(sw, dw);
    let y_lerps = build_axis_lerps(sh, dh);
    let mut out = vec![0.0_f32; dw * dh];
    for (dy, y) in y_lerps.iter().enumerate() {
        let wy0 = f32::from(y.w0) / 256.0;
        let wy1 = f32::from(256_u16.saturating_sub(y.w0)) / 256.0;
        for (dx, x) in x_lerps.iter().enumerate() {
            let wx0 = f32::from(x.w0) / 256.0;
            let wx1 = f32::from(256_u16.saturating_sub(x.w0)) / 256.0;
            let p00 = data[y.i0 * sw + x.i0];
            let p01 = data[y.i0 * sw + x.i1];
            let p10 = data[y.i1 * sw + x.i0];
            let p11 = data[y.i1 * sw + x.i1];
            let top = p00 * wx0 + p01 * wx1;
            let bot = p10 * wx0 + p11 * wx1;
            out[dy * dw + dx] = top * wy0 + bot * wy1;
        }
    }
    out
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]
fn resize_mask_bicubic(mask: &Mask, new_size: Size) -> Vec<f32> {
    let src = mask.size();
    let data = mask.data();
    let sw = src.width as usize;
    let sh = src.height as usize;
    let dw = new_size.width as usize;
    let dh = new_size.height as usize;
    let max_x = sw.saturating_sub(1) as i32;
    let max_y = sh.saturating_sub(1) as i32;
    let mut out = vec![0.0_f32; dw * dh];
    for dy in 0..dh {
        let sy = if dh == 1 {
            0.0
        } else {
            (dy as f32 + 0.5) * sh as f32 / dw_f32(dh) - 0.5
        };
        let sy = sy.clamp(0.0, max_y as f32);
        let iy = sy.floor() as i32;
        let wy = cubic_weights(sy - iy as f32);
        let ys = cubic_indexes(iy, max_y);
        for dx in 0..dw {
            let sx = if dw == 1 {
                0.0
            } else {
                (dx as f32 + 0.5) * sw as f32 / dw_f32(dw) - 0.5
            };
            let sx = sx.clamp(0.0, max_x as f32);
            let ix = sx.floor() as i32;
            let wx = cubic_weights(sx - ix as f32);
            let xs = cubic_indexes(ix, max_x);
            let mut col = [0.0_f32; 4];
            for (j, &yy) in ys.iter().enumerate() {
                let mut row_v = 0.0;
                for (i, &xx) in xs.iter().enumerate() {
                    row_v += data[yy * sw + xx] * wx[i];
                }
                col[j] = row_v;
            }
            let v = col[0] * wy[0] + col[1] * wy[1] + col[2] * wy[2] + col[3] * wy[3];
            out[dy * dw + dx] = v.clamp(0.0, 1.0);
        }
    }
    out
}

fn dw_f32(len: usize) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    {
        len as f32
    }
}

#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn cubic_indexes(i: i32, max_i: i32) -> [usize; 4] {
    [
        (i - 1).clamp(0, max_i) as usize,
        i.clamp(0, max_i) as usize,
        (i + 1).clamp(0, max_i) as usize,
        (i + 2).clamp(0, max_i) as usize,
    ]
}

pub(crate) fn rotate_mask_cw90(mask: &Mask) -> Result<Mask> {
    let src = mask.size();
    let sw = src.width as usize;
    let sh = src.height as usize;
    let data = mask.data();
    let mut out = vec![0.0_f32; data.len()];
    for y in 0..sh {
        for x in 0..sw {
            let nx = sh - 1 - y;
            let ny = x;
            out[ny * sh + nx] = data[y * sw + x];
        }
    }
    Mask::from_raw(Size::new(src.height, src.width), out)
}

pub(crate) fn rotate_mask_180(mask: &Mask) -> Result<Mask> {
    let size = mask.size();
    let data = mask.data();
    let out: Vec<f32> = data.iter().rev().copied().collect();
    Mask::from_raw(size, out)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(crate) fn rotate_mask_degrees(mask: &Mask, degrees: f32) -> Result<Mask> {
    let mut d = degrees % 360.0;
    if d < 0.0 {
        d += 360.0;
    }
    if (d - 0.0).abs() < 1e-3 || (d - 360.0).abs() < 1e-3 {
        return Ok(mask.clone());
    }
    if (d - 180.0).abs() < 1e-3 {
        return rotate_mask_180(mask);
    }
    let size = mask.size();
    let w = size.width as usize;
    let h = size.height as usize;
    let data = mask.data();
    let mut out = vec![0.0_f32; data.len()];
    let rad = (-degrees).to_radians();
    let (sin_t, cos_t) = rad.sin_cos();
    let cx = (w as f32 - 1.0) * 0.5;
    let cy = (h as f32 - 1.0) * 0.5;
    for y in 0..h {
        let dy = y as f32 - cy;
        for x in 0..w {
            let dx = x as f32 - cx;
            let sx = cos_t * dx - sin_t * dy + cx;
            let sy = sin_t * dx + cos_t * dy + cy;
            if sx < 0.0 || sy < 0.0 {
                continue;
            }
            let sx = sx.round() as isize;
            let sy = sy.round() as isize;
            if sx < 0 || sy < 0 || sx as usize >= w || sy as usize >= h {
                continue;
            }
            out[y * w + x] = data[sy as usize * w + sx as usize];
        }
    }
    Mask::from_raw(size, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::VideoEffect;

    #[test]
    fn quarter_turn_maps_back_to_the_source_column() {
        let steps = [PixelMap::QuarterTurn {
            src_width: 2,
            src_height: 1,
        }];
        assert_eq!(source_pixel(&steps, 0, 1), Some((1, 0)));
        assert_eq!(source_pixel(&steps, 0, 2), None);
    }

    #[test]
    fn canvas_edit_maps_through_crop_scale_and_half_turn() {
        let steps = [
            PixelMap::Crop {
                left: 1,
                top: 0,
                width: 2,
                height: 2,
            },
            PixelMap::Nearest {
                src_width: 2,
                src_height: 2,
                dst_width: 4,
                dst_height: 2,
            },
            PixelMap::HalfTurn {
                width: 4,
                height: 2,
            },
            PixelMap::Layer {
                origin_x: 1,
                origin_y: 0,
                width: 4,
                height: 2,
            },
        ];
        assert_eq!(source_pixel(&steps, 4, 0), Some((1, 1)));
        assert_eq!(source_pixel(&steps, 3, 0), Some((1, 1)));
        assert_eq!(source_pixel(&steps, 0, 0), None);
    }

    struct Still {
        frame: reelforge_core::Frame,
        mask: Mask,
    }

    impl VideoClip for Still {
        fn duration(&self) -> reelforge_core::Duration {
            reelforge_core::Duration::from_secs(1.0)
        }

        fn size(&self) -> Size {
            self.frame.size()
        }

        fn fps(&self) -> Option<f64> {
            None
        }

        fn frame_at(&self, t: Time) -> Result<reelforge_core::Frame> {
            if t.as_secs() < 0.0 || t.as_secs() >= 1.0 {
                return Err(CoreError::TimeOutOfRange {
                    time: t,
                    range: (Time::ZERO, Time::from_secs(1.0)),
                });
            }
            Ok(self.frame.clone())
        }

        fn mask_at(&self, t: Time) -> Result<Option<Mask>> {
            let _ = self.frame_at(t)?;
            Ok(Some(self.mask.clone()))
        }
    }

    #[test]
    fn resize_keeps_quarter_coverage() {
        use reelforge_core::{Frame, Rgb8};
        use std::sync::Arc;
        let mask = Mask::from_raw(Size::new(2, 2), vec![0.25; 4]).unwrap();
        let frame = Frame::solid_rgb(Size::new(2, 2), Rgb8::RED).unwrap();
        let clip = Arc::new(Still { frame, mask });
        for make in [
            crate::Resize::to(Size::new(5, 3)),
            crate::Resize::to_bicubic(Size::new(5, 3)),
        ] {
            let resized = make.apply(Arc::clone(&clip) as Arc<dyn VideoClip>).unwrap();
            let got = resized.mask_at(Time::ZERO).unwrap().expect("mask");
            assert_eq!(got.size(), Size::new(5, 3));
            for sample in got.data() {
                assert!((sample - 0.25).abs() < 1e-4, "sample {sample}");
            }
        }
    }

    #[test]
    fn crop_scale_rotate_keeps_the_mark_on_the_canvas() {
        use reelforge_compose::{CompositeLayer, CompositeVideo};
        use reelforge_core::{Frame, FrameFormat, Position, Rgb8};
        use std::sync::Arc;
        let mut pixels = vec![0_u8; 24];
        pixels[15] = 200;
        let frame = Frame::from_raw(Size::new(4, 2), FrameFormat::Rgb8, pixels).unwrap();
        let mut coverage = vec![0.0; 8];
        coverage[5] = 0.25;
        let mask = Mask::from_raw(Size::new(4, 2), coverage).unwrap();
        let clip: Arc<dyn VideoClip> = Arc::new(Still { frame, mask });
        let turned = crate::Rotate::half()
            .apply(
                crate::Resize::to_nearest(Size::new(4, 2))
                    .apply(crate::Crop::new(1, 0, 2, 2).apply(clip).unwrap())
                    .unwrap(),
            )
            .unwrap();
        let mask = turned.mask_at(Time::ZERO).unwrap().expect("mask");
        let frame = turned.frame_at(Time::ZERO).unwrap();
        assert!((mask.data()[2] - 0.25).abs() < 1e-6);
        assert!((mask.data()[3] - 0.25).abs() < 1e-6);
        assert!(mask.data()[0].abs() < 1e-6);
        assert_eq!(&frame.data()[6..9], &[200, 0, 0]);
        assert_eq!(&frame.data()[9..12], &[200, 0, 0]);

        let layer = CompositeLayer::new(turned).with_position(Position::absolute(1, 0));
        let canvas =
            CompositeVideo::with_background(Size::new(6, 2), Rgb8::BLACK, vec![layer]).unwrap();
        let painted = canvas.frame_at(Time::ZERO).unwrap();
        assert_eq!(painted.data()[0], 0);
        assert_eq!(painted.data()[9], 50);
        assert_eq!(painted.data()[12], 50);
        assert_eq!(
            source_pixel(
                &[
                    PixelMap::Crop {
                        left: 1,
                        top: 0,
                        width: 2,
                        height: 2,
                    },
                    PixelMap::Nearest {
                        src_width: 2,
                        src_height: 2,
                        dst_width: 4,
                        dst_height: 2,
                    },
                    PixelMap::HalfTurn {
                        width: 4,
                        height: 2,
                    },
                    PixelMap::Layer {
                        origin_x: 1,
                        origin_y: 0,
                        width: 4,
                        height: 2,
                    },
                ],
                4,
                0,
            ),
            Some((1, 1))
        );
    }
}
