//! Minimal `SubRip` (`.srt`) parser.

use crate::error::{Result, TextError};
use reelforge_core::{Duration, Time};

/// One subtitle cue.
#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleCue {
    /// Inclusive start.
    pub start: Time,
    /// Exclusive end (duration-friendly).
    pub end: Time,
    /// Cue text (may contain newlines).
    pub text: String,
}

impl SubtitleCue {
    /// Active duration of the cue.
    #[must_use]
    pub fn duration(&self) -> Duration {
        Duration::from_secs((self.end.as_secs() - self.start.as_secs()).max(0.0))
    }
}

/// Parse a UTF-8 SRT document into cues.
///
/// # Errors
///
/// Returns parse errors for malformed timestamps or empty documents.
pub fn parse_srt(input: &str) -> Result<Vec<SubtitleCue>> {
    let mut cues = Vec::new();
    let blocks: Vec<&str> = input
        .split("\n\n")
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .collect();

    for block in blocks {
        let lines: Vec<&str> = block.lines().map(str::trim).collect();
        if lines.len() < 2 {
            continue;
        }
        // Optional index line: skip if pure digits.
        let (time_line, text_lines) = if lines[0].chars().all(|c| c.is_ascii_digit()) {
            if lines.len() < 3 {
                continue;
            }
            (lines[1], &lines[2..])
        } else {
            (lines[0], &lines[1..])
        };
        let (start, end) = parse_time_range(time_line)?;
        let text = text_lines.join("\n");
        if text.is_empty() {
            continue;
        }
        cues.push(SubtitleCue { start, end, text });
    }

    if cues.is_empty() {
        return Err(TextError::message("no subtitle cues parsed"));
    }
    Ok(cues)
}

fn parse_time_range(line: &str) -> Result<(Time, Time)> {
    let parts: Vec<&str> = line.split("-->").map(str::trim).collect();
    if parts.len() != 2 {
        return Err(TextError::message(format!("bad srt timing line: {line}")));
    }
    Ok((parse_ts(parts[0])?, parse_ts(parts[1])?))
}

fn parse_ts(s: &str) -> Result<Time> {
    // HH:MM:SS,mmm or HH:MM:SS.mmm
    let s = s.replace(',', ".");
    let segs: Vec<&str> = s.split(':').collect();
    if segs.len() != 3 {
        return Err(TextError::message(format!("bad timestamp: {s}")));
    }
    let h: f64 = segs[0]
        .parse()
        .map_err(|_| TextError::message(format!("bad hours in {s}")))?;
    let m: f64 = segs[1]
        .parse()
        .map_err(|_| TextError::message(format!("bad minutes in {s}")))?;
    let sec: f64 = segs[2]
        .parse()
        .map_err(|_| TextError::message(format!("bad seconds in {s}")))?;
    Ok(Time::from_secs(h * 3600.0 + m * 60.0 + sec))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_cues() {
        let srt = "\
1
00:00:01,000 --> 00:00:02,500
Hello

2
00:00:03,000 --> 00:00:04,000
World
";
        let cues = parse_srt(srt).unwrap();
        assert_eq!(cues.len(), 2);
        assert!((cues[0].start.as_secs() - 1.0).abs() < 1e-9);
        assert_eq!(cues[1].text, "World");
    }
}
