//! Minimal `ASS`/`SSA` (`.ass` / `.ssa`) dialogue parser.

use super::srt::SubtitleCue;
use crate::error::{Result, TextError};
use reelforge_core::Time;

/// Parse `ASS`/`SSA` dialogue events into plain-text cues for burn-in.
///
/// Reads `Dialogue:` lines (after an optional `[Events]` section). Override
/// style fields are accepted but ignored; ASS override tags (`{\\...}`) and
/// hard line breaks (`\\N`, `\\n`) are normalized.
///
/// # Errors
///
/// Returns an error when no dialogue lines can be parsed.
pub fn parse_ass(input: &str) -> Result<Vec<SubtitleCue>> {
    let mut cues = Vec::new();
    let mut in_events = false;
    let mut format_cols: Option<Vec<String>> = None;

    for raw in input.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') {
            in_events = line.eq_ignore_ascii_case("[Events]");
            continue;
        }
        if !in_events && !line.to_ascii_lowercase().starts_with("dialogue:") {
            // Some files omit section headers; still accept Dialogue: anywhere.
            if line.to_ascii_lowercase().starts_with("format:") && format_cols.is_none() {
                format_cols = Some(parse_format_line(line));
            }
            continue;
        }
        if line.to_ascii_lowercase().starts_with("format:") {
            format_cols = Some(parse_format_line(line));
            continue;
        }
        if !line.to_ascii_lowercase().starts_with("dialogue:") {
            continue;
        }
        if let Some(cue) = parse_dialogue_line(line, format_cols.as_deref())? {
            cues.push(cue);
        }
    }

    if cues.is_empty() {
        return Err(TextError::message("no ASS dialogue cues parsed"));
    }
    Ok(cues)
}

fn parse_format_line(line: &str) -> Vec<String> {
    let rest = line.split_once(':').map_or(line, |(_, r)| r);
    rest.split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .collect()
}

fn parse_dialogue_line(line: &str, format: Option<&[String]>) -> Result<Option<SubtitleCue>> {
    let rest = line
        .split_once(':')
        .map(|(_, r)| r.trim())
        .ok_or_else(|| TextError::message(format!("bad dialogue: {line}")))?;

    // Default ASS order: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
    // Text is last and may contain commas — take remaining after 9 commas if default.
    let (start_s, end_s, text) = if let Some(cols) = format {
        split_by_format(rest, cols)?
    } else {
        split_default_dialogue(rest)?
    };

    let start = parse_ass_time(start_s)?;
    let end = parse_ass_time(end_s)?;
    let text = normalize_ass_text(text);
    if text.is_empty() {
        return Ok(None);
    }
    Ok(Some(SubtitleCue { start, end, text }))
}

fn split_default_dialogue(rest: &str) -> Result<(&str, &str, &str)> {
    // Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text
    let mut parts = Vec::new();
    let mut start = 0usize;
    let bytes = rest.as_bytes();
    let mut commas = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b',' {
            parts.push(&rest[start..i]);
            start = i + 1;
            commas += 1;
            if commas == 9 {
                break;
            }
        }
    }
    if commas < 9 {
        return Err(TextError::message(format!(
            "ASS dialogue has fewer than 10 fields: {rest}"
        )));
    }
    let text = &rest[start..];
    // parts: 0 Layer, 1 Start, 2 End, ...
    Ok((parts[1].trim(), parts[2].trim(), text))
}

fn split_by_format<'a>(rest: &'a str, cols: &[String]) -> Result<(&'a str, &'a str, &'a str)> {
    let start_i = cols.iter().position(|c| c == "start");
    let end_i = cols.iter().position(|c| c == "end");
    let text_i = cols.iter().position(|c| c == "text");
    let (Some(si), Some(ei), Some(ti)) = (start_i, end_i, text_i) else {
        return split_default_dialogue(rest);
    };
    // Split into first `ti` fields + remainder text
    let mut fields = Vec::new();
    let mut start = 0usize;
    let mut count = 0usize;
    for (i, &b) in rest.as_bytes().iter().enumerate() {
        if b == b',' {
            fields.push(&rest[start..i]);
            start = i + 1;
            count += 1;
            if count == ti {
                break;
            }
        }
    }
    if fields.len() < ti {
        return Err(TextError::message("ASS format/dialogue mismatch"));
    }
    let text = &rest[start..];
    Ok((fields[si].trim(), fields[ei].trim(), text))
}

fn parse_ass_time(s: &str) -> Result<Time> {
    // H:MM:SS.cc  (centiseconds) or H:MM:SS.mm
    let s = s.trim();
    let segs: Vec<&str> = s.split(':').collect();
    if segs.len() != 3 {
        return Err(TextError::message(format!("bad ASS time: {s}")));
    }
    let h: f64 = segs[0]
        .parse()
        .map_err(|_| TextError::message(format!("bad ASS hours: {s}")))?;
    let m: f64 = segs[1]
        .parse()
        .map_err(|_| TextError::message(format!("bad ASS minutes: {s}")))?;
    let sec: f64 = segs[2]
        .parse()
        .map_err(|_| TextError::message(format!("bad ASS seconds: {s}")))?;
    Ok(Time::from_secs(h * 3600.0 + m * 60.0 + sec))
}

fn normalize_ass_text(s: &str) -> String {
    // strip {\...} overrides
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            for nc in chars.by_ref() {
                if nc == '}' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out.replace("\\N", "\n")
        .replace("\\n", "\n")
        .replace("\\h", " ")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dialogue() {
        let ass = r"
[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:02.50,Default,,0,0,0,,Hello{\b1}World\NLine2
";
        let cues = parse_ass(ass).unwrap();
        assert_eq!(cues.len(), 1);
        assert!((cues[0].start.as_secs() - 1.0).abs() < 1e-6);
        assert_eq!(cues[0].text, "HelloWorld\nLine2");
    }
}
