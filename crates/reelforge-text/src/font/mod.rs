//! Font backends: built-in bitmap and TrueType via `fontdue`.

mod bitmap;
mod face;

pub use face::{FontFace, GlyphCoverage, load_face};
