//! Freeze a rectangular region while the rest of the frame stays live.

use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// At time `t`, capture a region and paste it over subsequent frames.
#[derive(Debug, Clone, Copy)]
pub struct FreezeRegion {
    /// Source time of the frozen patch.
    pub t: Time,
    /// Region origin x.
    pub x: u32,
    /// Region origin y.
    pub y: u32,
    /// Region width.
    pub width: u32,
    /// Region height.
    pub height: u32,
}

impl FreezeRegion {
    /// Freeze the rectangle `(x,y,width,height)` sampled at `t`.
    #[must_use]
    pub const fn new(t: Time, x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            t,
            x,
            y,
            width,
            height,
        }
    }
}

impl VideoEffect for FreezeRegion {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        let frozen = clip.frame_at(self.t)?;
        let patch = crate::raster::crop_frame(&frozen, self.x, self.y, self.width, self.height)?;
        Ok(Arc::new(FreezeRegionVideo {
            inner: clip,
            patch,
            x: self.x,
            y: self.y,
        }))
    }
}

struct FreezeRegionVideo {
    inner: Arc<dyn VideoClip>,
    patch: Frame,
    x: u32,
    y: u32,
}

impl VideoClip for FreezeRegionVideo {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn size(&self) -> Size {
        self.inner.size()
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        let mut frame = self.inner.frame_at(t)?;
        blit_patch(&mut frame, &self.patch, self.x, self.y);
        Ok(frame)
    }
}

fn blit_patch(dst: &mut Frame, patch: &Frame, x: u32, y: u32) {
    let bpp = dst.format().bytes_per_pixel();
    if bpp != patch.format().bytes_per_pixel() {
        return;
    }
    let dw = dst.size().width as usize;
    let dh = dst.size().height as usize;
    let pw = patch.size().width as usize;
    let ph = patch.size().height as usize;
    let x0 = x as usize;
    let y0 = y as usize;
    let dst_data = dst.data_mut();
    let src = patch.data();
    for row in 0..ph {
        let dy = y0 + row;
        if dy >= dh {
            break;
        }
        for col in 0..pw {
            let dx = x0 + col;
            if dx >= dw {
                break;
            }
            let si = (row * pw + col) * bpp;
            let di = (dy * dw + dx) * bpp;
            dst_data[di..di + bpp].copy_from_slice(&src[si..si + bpp]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn freezes_patch() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(8, 8),
            Rgb8::BLUE,
            Duration::from_secs(1.0),
        ));
        let out = FreezeRegion::new(Time::ZERO, 1, 1, 2, 2)
            .apply(clip)
            .unwrap();
        let f = out.frame_at(Time::from_secs(0.5)).unwrap();
        assert_eq!(f.size(), Size::new(8, 8));
    }
}
