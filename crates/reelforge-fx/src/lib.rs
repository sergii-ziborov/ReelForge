//! Built-in video and audio effects for `ReelForge`.

mod audio_fade;
mod audio_norm;
mod blink;
mod color_fx;
mod crop;
mod crossfade;
mod even_size;
mod fade;
mod freeze;
mod gain;
mod gamma;
mod identity;
mod loop_fx;
mod lum_contrast;
mod margin;
mod mask_color;
mod mirror;
mod raster;
mod resize;
mod rotate;
mod scale;
mod scroll;
mod slide;
mod speed;
mod time_mirror;
mod time_sym;

pub use audio_fade::{AudioFadeIn, AudioFadeOut};
pub use audio_norm::AudioNormalize;
pub use blink::Blink;
pub use color_fx::{BlackAndWhite, InvertColors, MultiplyColor};
pub use crop::Crop;
pub use crossfade::{CrossFadeIn, CrossFadeOut};
pub use even_size::EvenSize;
pub use fade::{FadeIn, FadeOut};
pub use freeze::Freeze;
pub use gain::{VolumeGain, validate_gain};
pub use gamma::GammaCorrection;
pub use identity::Identity;
pub use loop_fx::Loop;
pub use lum_contrast::LumContrast;
pub use margin::Margin;
pub use mask_color::MaskColor;
pub use mirror::{MirrorX, MirrorY};
pub use resize::Resize;
pub use rotate::Rotate;
pub use scale::{ResizeFilter, resize_bilinear};
pub use scroll::Scroll;
pub use slide::{SlideIn, SlideOut, SlideSide};
pub use speed::Speed;
pub use time_mirror::TimeMirror;
pub use time_sym::TimeSymmetrize;

use reelforge_core::{AudioEffect, VideoEffect};
use std::sync::Arc;

/// Type-erased video effect handle.
pub type DynVideoEffect = Arc<dyn VideoEffect>;

/// Type-erased audio effect handle.
pub type DynAudioEffect = Arc<dyn AudioEffect>;
