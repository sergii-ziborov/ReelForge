//! Subtitle helpers: SRT parse and burn-in layers.

mod burn;
mod srt;

pub use burn::{BurnInOptions, burn_in_layers};
pub use srt::{SubtitleCue, parse_srt};
