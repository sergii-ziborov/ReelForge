//! Layering, concatenation, and mixing for `ReelForge` clips.

mod audio_concat;
mod audio_mix;
mod blit;
mod composite;
mod concat;
mod layer;
mod timeline;

pub use audio_concat::{ConcatAudio, concatenate_audio};
pub use audio_mix::{MixAudio, MixTrack, mix_audio, mix_audio_clips};
pub use composite::{CompositeVideo, composite_video, composite_video_with_background};
pub use concat::{ConcatVideo, concatenate_video};
pub use layer::CompositeLayer;

use reelforge_core::CoreError;

/// Errors specific to composition (wrapping core where useful).
#[derive(Debug, thiserror::Error)]
pub enum ComposeError {
    /// Core model failure.
    #[error(transparent)]
    Core(#[from] CoreError),

    /// Empty clip list or incompatible inputs.
    #[error("compose: {0}")]
    Message(String),
}

/// Result alias for compose operations.
pub type Result<T> = std::result::Result<T, ComposeError>;
