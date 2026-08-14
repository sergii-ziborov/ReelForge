//! Sequential video concatenation.

use crate::timeline::map_concat_time;
use crate::{ComposeError, Result};
use reelforge_core::{Duration, Frame, Size, Time, VideoClip, VideoSurface};
use std::sync::Arc;

/// Concatenate video clips end-to-end on a shared timeline.
///
/// All clips must share the same [`Size`]. Duration is the sum of inputs.
/// FPS is the first defined FPS among clips, if any.
#[derive(Clone)]
pub struct ConcatVideo {
    clips: Vec<Arc<dyn VideoClip>>,
    /// Exclusive end times on the concatenated timeline for each clip.
    ends: Vec<Duration>,
    size: Size,
    duration: Duration,
    fps: Option<f64>,
}

impl ConcatVideo {
    /// Build a concatenation of `clips` in order.
    ///
    /// # Errors
    ///
    /// Returns [`ComposeError`] when the list is empty or sizes differ.
    pub fn new(clips: Vec<Arc<dyn VideoClip>>) -> Result<Self> {
        if clips.is_empty() {
            return Err(ComposeError::Message(
                "concatenate_video requires at least one clip".into(),
            ));
        }
        let size = clips[0].size();
        size.require_positive().map_err(ComposeError::from)?;

        let mut ends = Vec::with_capacity(clips.len());
        let mut total = Duration::ZERO;
        let mut fps = None;

        for clip in &clips {
            if clip.size() != size {
                return Err(ComposeError::Message(format!(
                    "all clips must share size {:?}, found {:?}",
                    size,
                    clip.size()
                )));
            }
            if !clip.duration().is_positive() {
                return Err(ComposeError::Message(
                    "each clip must have positive duration".into(),
                ));
            }
            total += clip.duration();
            ends.push(total);
            if fps.is_none() {
                fps = clip.fps();
            }
        }

        Ok(Self {
            clips,
            ends,
            size,
            duration: total,
            fps,
        })
    }
}

impl VideoClip for ConcatVideo {
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
        let (i, local) = map_concat_time(&self.ends, self.duration, t)?;
        self.clips[i].frame_at(local)
    }

    fn surface_at(&self, t: Time) -> reelforge_core::Result<VideoSurface> {
        let (i, local) = map_concat_time(&self.ends, self.duration, t)?;
        self.clips[i].surface_at(local)
    }
}

/// Concatenate video clips; returns a trait object.
///
/// # Errors
///
/// Propagates [`ConcatVideo::new`] errors.
pub fn concatenate_video(clips: Vec<Arc<dyn VideoClip>>) -> Result<Arc<dyn VideoClip>> {
    Ok(Arc::new(ConcatVideo::new(clips)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8, VideoClip};

    #[test]
    fn concat_two_colors() {
        let a: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::RED,
            Duration::from_secs(1.0),
        ));
        let b: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::BLUE,
            Duration::from_secs(1.0),
        ));
        let cat = ConcatVideo::new(vec![a, b]).unwrap();
        assert!((cat.duration().as_secs() - 2.0).abs() < f64::EPSILON);
        let f0 = cat.frame_at(Time::from_secs(0.1)).unwrap();
        assert_eq!(&f0.data()[0..3], &[255, 0, 0]);
        let f1 = cat.frame_at(Time::from_secs(1.1)).unwrap();
        assert_eq!(&f1.data()[0..3], &[0, 0, 255]);
    }
}
