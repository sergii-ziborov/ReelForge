//! Built-in video and audio effects for `ReelForge`.

mod color_fx;
mod crop;
mod crossfade;
mod even_size;
mod fade;
mod freeze;
mod gain;
mod identity;
mod loop_fx;
mod margin;
mod mirror;
mod raster;
mod resize;
mod rotate;
mod speed;
mod time_mirror;

pub use color_fx::{BlackAndWhite, InvertColors, MultiplyColor};
pub use crop::Crop;
pub use crossfade::{CrossFadeIn, CrossFadeOut};
pub use even_size::EvenSize;
pub use fade::{FadeIn, FadeOut};
pub use freeze::Freeze;
pub use gain::{VolumeGain, validate_gain};
pub use identity::Identity;
pub use loop_fx::Loop;
pub use margin::Margin;
pub use mirror::{MirrorX, MirrorY};
pub use resize::Resize;
pub use rotate::Rotate;
pub use speed::Speed;
pub use time_mirror::TimeMirror;

use reelforge_core::{AudioEffect, VideoEffect};
use std::sync::Arc;

/// Type-erased video effect handle.
pub type DynVideoEffect = Arc<dyn VideoEffect>;

/// Type-erased audio effect handle.
pub type DynAudioEffect = Arc<dyn AudioEffect>;
