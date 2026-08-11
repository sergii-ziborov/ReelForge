//! Identity effect (passes the clip through unchanged).

use reelforge_core::{AudioClip, AudioEffect, Result, VideoClip, VideoEffect};
use std::sync::Arc;

/// No-op effect used for pipeline wiring tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct Identity;

impl VideoEffect for Identity {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(clip)
    }
}

impl AudioEffect for Identity {
    fn apply(&self, clip: Arc<dyn AudioClip>) -> Result<Arc<dyn AudioClip>> {
        Ok(clip)
    }
}
