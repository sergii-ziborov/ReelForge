//! Subtitle helpers: `SRT` / `WebVTT` / `ASS` parse and burn-in layers.

mod ass;
mod burn;
mod srt;
mod vtt;

pub use ass::parse_ass;
pub use burn::{BurnInOptions, burn_in_layers};
pub use srt::{SubtitleCue, parse_srt};
pub use vtt::parse_vtt;

use crate::error::{Result, TextError};
use std::path::Path;

/// Parse subtitles by file extension (`.srt`, `.vtt`, `.ass`, `.ssa`).
///
/// # Errors
///
/// Returns I/O or parse errors.
pub fn parse_subtitles_path(path: impl AsRef<Path>) -> Result<Vec<SubtitleCue>> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|e| {
        TextError::message(format!("read subtitles {}: {e}", path.display()))
    })?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "srt" => parse_srt(&text),
        "vtt" => parse_vtt(&text),
        "ass" | "ssa" => parse_ass(&text),
        other => Err(TextError::message(format!(
            "unsupported subtitle extension .{other} (use srt, vtt, ass, ssa)"
        ))),
    }
}

/// Parse subtitles from a string, auto-detecting format when possible.
///
/// Detection: `WEBVTT` header → VTT; `[Script Info]` / `Dialogue:` → ASS; else SRT.
///
/// # Errors
///
/// Returns parse errors from the chosen parser.
pub fn parse_subtitles(input: &str) -> Result<Vec<SubtitleCue>> {
    let trimmed = input.trim_start_matches('\u{feff}').trim_start();
    let head = trimmed.chars().take(64).collect::<String>().to_ascii_uppercase();
    if head.starts_with("WEBVTT") {
        return parse_vtt(input);
    }
    if head.contains("[SCRIPT INFO]")
        || head.contains("[V4+ STYLES]")
        || head.contains("[EVENTS]")
        || trimmed.lines().any(|l| l.trim().to_ascii_lowercase().starts_with("dialogue:"))
    {
        return parse_ass(input);
    }
    parse_srt(input)
}
