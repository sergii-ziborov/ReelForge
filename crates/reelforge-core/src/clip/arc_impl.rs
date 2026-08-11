//! [`Arc`] trait-object passthrough implementations.

use super::id::ClipId;
use super::traits::{AudioClip, VideoClip};
use crate::audio::{AudioBuffer, AudioFormat};
use crate::error::Result;
use crate::frame::{Frame, Mask};
use crate::layout::Size;
use crate::time::{Duration, Time};
use std::sync::Arc;

impl VideoClip for Arc<dyn VideoClip> {
    fn duration(&self) -> Duration {
        (**self).duration()
    }

    fn size(&self) -> Size {
        (**self).size()
    }

    fn fps(&self) -> Option<f64> {
        (**self).fps()
    }

    fn id(&self) -> Option<&ClipId> {
        (**self).id()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        (**self).frame_at(t)
    }

    fn mask_at(&self, t: Time) -> Result<Option<Mask>> {
        (**self).mask_at(t)
    }
}

impl AudioClip for Arc<dyn AudioClip> {
    fn duration(&self) -> Duration {
        (**self).duration()
    }

    fn format(&self) -> AudioFormat {
        (**self).format()
    }

    fn id(&self) -> Option<&ClipId> {
        (**self).id()
    }

    fn samples_at(&self, t: Time, frame_count: usize) -> Result<AudioBuffer> {
        (**self).samples_at(t, frame_count)
    }
}
