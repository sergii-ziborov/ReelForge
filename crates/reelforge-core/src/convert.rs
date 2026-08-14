//! Explicit pixel conversion: native surfaces → packed RGB `Frame`.
//!
//! [`crate::VideoSurface::to_frame`] stays zero-copy packed RGB/RGBA only.
//! Use [`surface_to_rgb_frame`] when the source is BGRA, YUV420P, or NV12.

#![allow(
    clippy::many_single_char_names,
    clippy::needless_range_loop,
    clippy::needless_lifetimes
)]

use crate::error::{CoreError, Result};
use crate::frame::{Frame, FrameFormat};
use crate::surface::{ColorRange, ColorSpace, PixelFormat, VideoSurface};

/// Convert any **CPU** surface to a packed RGB8 [`Frame`].
///
/// Packed RGB is cloned/compacted. BGRA swaps channels. Planar YUV is
/// converted with the surface [`crate::ColorInfo`] (BT.601 / BT.709,
/// limited / full). [`crate::MemoryLocation::External`] fails — the host
/// must map the handle first.
///
/// # Errors
///
/// External surface, missing planes, or buffer geometry errors.
pub fn surface_to_rgb_frame(surface: &VideoSurface) -> Result<Frame> {
    if surface.location() == crate::surface::MemoryLocation::External
        || surface.external().is_some()
    {
        return Err(CoreError::invalid_frame(
            "external surface has no CPU pixels; map it before to_rgb_frame",
        ));
    }
    match surface.format() {
        PixelFormat::Rgb8 => surface.to_frame(),
        PixelFormat::Rgba8 => rgba_to_rgb(surface),
        PixelFormat::Bgra8 => bgra_to_rgb(surface),
        PixelFormat::Yuv420p => yuv420p_to_rgb(surface),
        PixelFormat::Nv12 => nv12_to_rgb(surface),
    }
}

fn packed_plane(surface: &VideoSurface, bpp: usize) -> Result<(&[u8], usize, usize)> {
    let plane = surface
        .plane(0)
        .ok_or_else(|| CoreError::invalid_frame("surface has no plane 0"))?;
    let w = usize::try_from(surface.size().width).unwrap_or(0);
    let h = usize::try_from(surface.size().height).unwrap_or(0);
    let row = w.saturating_mul(bpp);
    if plane.stride() < row {
        return Err(CoreError::invalid_frame("packed stride shorter than row"));
    }
    Ok((plane.data(), plane.stride(), h))
}

fn rgba_to_rgb(surface: &VideoSurface) -> Result<Frame> {
    let (data, stride, h) = packed_plane(surface, 4)?;
    let w = usize::try_from(surface.size().width).unwrap_or(0);
    let mut out = vec![0_u8; w.saturating_mul(h).saturating_mul(3)];
    for y in 0..h {
        let src = &data[y * stride..y * stride + w * 4];
        let dst = &mut out[y * w * 3..(y + 1) * w * 3];
        for x in 0..w {
            dst[x * 3] = src[x * 4];
            dst[x * 3 + 1] = src[x * 4 + 1];
            dst[x * 3 + 2] = src[x * 4 + 2];
        }
    }
    Frame::from_raw(surface.size(), FrameFormat::Rgb8, out)
}

fn bgra_to_rgb(surface: &VideoSurface) -> Result<Frame> {
    let (data, stride, h) = packed_plane(surface, 4)?;
    let w = usize::try_from(surface.size().width).unwrap_or(0);
    let mut out = vec![0_u8; w.saturating_mul(h).saturating_mul(3)];
    for y in 0..h {
        let src = &data[y * stride..y * stride + w * 4];
        let dst = &mut out[y * w * 3..(y + 1) * w * 3];
        for x in 0..w {
            dst[x * 3] = src[x * 4 + 2];
            dst[x * 3 + 1] = src[x * 4 + 1];
            dst[x * 3 + 2] = src[x * 4];
        }
    }
    Frame::from_raw(surface.size(), FrameFormat::Rgb8, out)
}

fn yuv420p_to_rgb(surface: &VideoSurface) -> Result<Frame> {
    let y = surface
        .plane(0)
        .ok_or_else(|| CoreError::invalid_frame("yuv420p missing Y"))?;
    let u = surface
        .plane(1)
        .ok_or_else(|| CoreError::invalid_frame("yuv420p missing U"))?;
    let v = surface
        .plane(2)
        .ok_or_else(|| CoreError::invalid_frame("yuv420p missing V"))?;
    let w = usize::try_from(surface.size().width).unwrap_or(0);
    let h = usize::try_from(surface.size().height).unwrap_or(0);
    let matrix = YuvMatrix::from_color(surface.color().space, surface.color().range);
    let mut out = vec![0_u8; w.saturating_mul(h).saturating_mul(3)];
    for row in 0..h {
        let cy = row / 2;
        let y_row = &y.data()[row * y.stride()..];
        let u_row = &u.data()[cy * u.stride()..];
        let v_row = &v.data()[cy * v.stride()..];
        let dst = &mut out[row * w * 3..];
        for col in 0..w {
            let cx = col / 2;
            let rgb = matrix.convert(y_row[col], u_row[cx], v_row[cx]);
            let i = col * 3;
            dst[i] = rgb[0];
            dst[i + 1] = rgb[1];
            dst[i + 2] = rgb[2];
        }
    }
    Frame::from_raw(surface.size(), FrameFormat::Rgb8, out)
}

fn nv12_to_rgb(surface: &VideoSurface) -> Result<Frame> {
    let y = surface
        .plane(0)
        .ok_or_else(|| CoreError::invalid_frame("nv12 missing Y"))?;
    let uv = surface
        .plane(1)
        .ok_or_else(|| CoreError::invalid_frame("nv12 missing UV"))?;
    let w = usize::try_from(surface.size().width).unwrap_or(0);
    let h = usize::try_from(surface.size().height).unwrap_or(0);
    let matrix = YuvMatrix::from_color(surface.color().space, surface.color().range);
    let mut out = vec![0_u8; w.saturating_mul(h).saturating_mul(3)];
    for row in 0..h {
        let cy = row / 2;
        let y_row = &y.data()[row * y.stride()..];
        let uv_row = &uv.data()[cy * uv.stride()..];
        let dst = &mut out[row * w * 3..];
        for col in 0..w {
            let cx = col / 2;
            let rgb = matrix.convert(y_row[col], uv_row[cx * 2], uv_row[cx * 2 + 1]);
            let i = col * 3;
            dst[i] = rgb[0];
            dst[i + 1] = rgb[1];
            dst[i + 2] = rgb[2];
        }
    }
    Frame::from_raw(surface.size(), FrameFormat::Rgb8, out)
}

#[derive(Clone, Copy)]
enum YuvMatrix {
    Limited601,
    Limited709,
    Full,
}

impl YuvMatrix {
    fn from_color(space: ColorSpace, range: ColorRange) -> Self {
        if range == ColorRange::Full {
            return Self::Full;
        }
        match space {
            ColorSpace::Bt709 | ColorSpace::Bt2020 => Self::Limited709,
            ColorSpace::Bt601 | ColorSpace::Rgb | ColorSpace::Unspecified => Self::Limited601,
        }
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn convert(self, y: u8, u: u8, v: u8) -> [u8; 3] {
        let yy = i32::from(y);
        let d = i32::from(u) - 128;
        let e = i32::from(v) - 128;
        let (r, g, b) = match self {
            Self::Full => (
                yy + ((359 * e) >> 8),
                yy - ((88 * d + 183 * e) >> 8),
                yy + ((454 * d) >> 8),
            ),
            Self::Limited601 => {
                let c = yy - 16;
                (
                    (298 * c + 409 * e + 128) >> 8,
                    (298 * c - 100 * d - 208 * e + 128) >> 8,
                    (298 * c + 516 * d + 128) >> 8,
                )
            }
            Self::Limited709 => {
                let c = yy - 16;
                (
                    (298 * c + 459 * e + 128) >> 8,
                    (298 * c - 55 * d - 136 * e + 128) >> 8,
                    (298 * c + 541 * d + 128) >> 8,
                )
            }
        };
        [clamp_u8(r), clamp_u8(g), clamp_u8(b)]
    }
}

fn clamp_u8(v: i32) -> u8 {
    u8::try_from(v.clamp(0, 255)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Size;
    use crate::media_time::MediaTime;
    use crate::plane::SurfacePlane;
    use crate::surface::{ColorInfo, StreamTimeBase, VideoSurface};

    fn ts() -> MediaTime {
        MediaTime::zero(1_000)
    }

    #[test]
    fn bgra_swaps_to_rgb() {
        // one pixel: B=0, G=0, R=255, A=255 → red
        let plane = SurfacePlane::new(1, 1, 4, vec![0, 0, 255, 255]).unwrap();
        let s = VideoSurface::from_planes(
            PixelFormat::Bgra8,
            Size::new(1, 1),
            vec![plane],
            ts(),
            None,
            ColorInfo::default(),
            StreamTimeBase::HZ_1K,
        )
        .unwrap();
        assert!(s.to_frame().is_err());
        let f = surface_to_rgb_frame(&s).unwrap();
        assert_eq!(f.data(), &[255, 0, 0]);
    }

    #[test]
    fn yuv420p_white_full_range() {
        let y = SurfacePlane::new(2, 2, 2, vec![255, 255, 255, 255]).unwrap();
        let u = SurfacePlane::new(1, 1, 1, vec![128]).unwrap();
        let v = SurfacePlane::new(1, 1, 1, vec![128]).unwrap();
        let color = ColorInfo {
            range: ColorRange::Full,
            space: ColorSpace::Bt601,
            ..ColorInfo::default()
        };
        let s = VideoSurface::from_planes(
            PixelFormat::Yuv420p,
            Size::new(2, 2),
            vec![y, u, v],
            ts(),
            None,
            color,
            StreamTimeBase::HZ_1K,
        )
        .unwrap();
        let f = surface_to_rgb_frame(&s).unwrap();
        assert!(f.data().iter().all(|&c| c >= 250));
    }

    #[test]
    fn nv12_neutral_is_gray() {
        let y = SurfacePlane::new(2, 2, 2, vec![128, 128, 128, 128]).unwrap();
        let uv = SurfacePlane::new(2, 1, 2, vec![128, 128]).unwrap();
        let color = ColorInfo {
            range: ColorRange::Full,
            space: ColorSpace::Bt601,
            ..ColorInfo::default()
        };
        let s = VideoSurface::from_planes(
            PixelFormat::Nv12,
            Size::new(2, 2),
            vec![y, uv],
            ts(),
            None,
            color,
            StreamTimeBase::HZ_1K,
        )
        .unwrap();
        let f = surface_to_rgb_frame(&s).unwrap();
        for chunk in f.data().chunks(3) {
            assert!((i16::from(chunk[0]) - i16::from(chunk[1])).abs() < 4);
            assert!((i16::from(chunk[1]) - i16::from(chunk[2])).abs() < 4);
        }
    }
}
