//! Text and title clips for `ReelForge`.
//!
//! Built-in bitmap face for portable titles, plus TrueType/OpenType via `fontdue`.

mod clip;
mod error;
mod font;
mod layout;
mod options;
mod raster;
mod subtitles;

pub use clip::{TextClip, text_clip};
pub use error::{Result, TextError};
pub use options::{BITMAP_FONT, TextClipOptions};
pub use subtitles::{
    BurnInOptions, SubtitleCue, burn_in_layers, parse_ass, parse_srt, parse_subtitles,
    parse_subtitles_path, parse_vtt,
};
