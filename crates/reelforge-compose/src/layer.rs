//! Layer descriptors for composite video.

use reelforge_core::{Duration, Position, Time, VideoClip};
use std::sync::Arc;

/// One layer in a [`crate::CompositeVideo`].
#[derive(Clone)]
pub struct CompositeLayer {
    /// Source clip for this layer.
    pub clip: Arc<dyn VideoClip>,
    /// Placement on the composite canvas.
    pub position: Position,
    /// Start time of this layer on the composite timeline.
    pub start: Time,
    /// Draw order: lower values under higher values.
    pub layer_index: i32,
    /// Constant opacity multiplier in `0.0..=1.0` (default `1.0`).
    pub opacity: f32,
}

impl CompositeLayer {
    /// Layer at the origin starting at `t = 0`, full opacity, `layer_index = 0`.
    #[must_use]
    pub fn new(clip: Arc<dyn VideoClip>) -> Self {
        Self {
            clip,
            position: Position::default(),
            start: Time::ZERO,
            layer_index: 0,
            opacity: 1.0,
        }
    }

    /// Set placement.
    #[must_use]
    pub fn with_position(mut self, position: Position) -> Self {
        self.position = position;
        self
    }

    /// Set start time on the composite timeline.
    #[must_use]
    pub fn with_start(mut self, start: Time) -> Self {
        self.start = start;
        self
    }

    /// Set layer order (higher draws on top).
    #[must_use]
    pub fn with_layer_index(mut self, layer_index: i32) -> Self {
        self.layer_index = layer_index;
        self
    }

    /// Set constant opacity.
    #[must_use]
    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity.clamp(0.0, 1.0);
        self
    }

    /// End time of this layer on the composite timeline (`start + duration`).
    #[must_use]
    pub fn end_time(&self) -> Time {
        self.start + self.clip.duration()
    }

    /// Whether composite time `t` falls inside this layer's active range.
    #[must_use]
    pub fn active_at(&self, t: Time) -> bool {
        let local = t.as_secs() - self.start.as_secs();
        local >= 0.0 && local < self.clip.duration().as_secs()
    }

    /// Map composite time to local clip time.
    #[must_use]
    pub fn local_time(&self, t: Time) -> Time {
        Time::from_secs(t.as_secs() - self.start.as_secs())
    }

    /// Contribution of this layer to composite duration.
    #[must_use]
    pub fn contributes_duration(&self) -> Duration {
        Duration::from_secs(self.start.as_secs() + self.clip.duration().as_secs())
    }
}
