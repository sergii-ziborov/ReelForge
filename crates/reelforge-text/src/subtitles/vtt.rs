//! `WebVTT` (`.vtt`) parser → [`SubtitleCue`].

use super::srt::SubtitleCue;
use crate::error::{Result, TextError};
use reelforge_core::Time;

/// Parse a UTF-8 `WebVTT` document into cues.
///
/// Supports cue blocks with `HH:MM:SS.mmm --> HH:MM:SS.mmm` (hours optional).
/// NOTE / STYLE / REGION blocks and cue settings after `-->` are ignored.
/// Basic markup (`<c>`, `<b>`, `<i>`, `<u>`, `<v>`, tags) is stripped for burn-in.
///
/// # Errors
///
/// Returns parse errors when no cues are found or timestamps are malformed.
pub fn parse_vtt(input: &str) -> Result<Vec<SubtitleCue>> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut cues = Vec::new();
    let mut lines = input.lines().map(str::trim).peekable();

    // Optional header
    if let Some(first) = lines.peek()
        && first.to_ascii_uppercase().starts_with("WEBVTT")
    {
        lines.next();
        // skip header metadata until blank
        while let Some(l) = lines.peek() {
            if l.is_empty() {
                lines.next();
                break;
            }
            lines.next();
        }
    }

    let mut buf: Vec<String> = Vec::new();
    let flush = |buf: &mut Vec<String>, cues: &mut Vec<SubtitleCue>| -> Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        // Find timing line
        let mut time_idx = None;
        for (i, line) in buf.iter().enumerate() {
            if line.contains("-->") {
                time_idx = Some(i);
                break;
            }
        }
        let Some(ti) = time_idx else {
            buf.clear();
            return Ok(());
        };
        let (start, end) = parse_vtt_range(&buf[ti])?;
        let text = buf[ti + 1..]
            .iter()
            .map(|s| strip_vtt_tags(s))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        buf.clear();
        if text.is_empty() {
            return Ok(());
        }
        cues.push(SubtitleCue { start, end, text });
        Ok(())
    };

    for line in lines {
        if line.is_empty() {
            flush(&mut buf, &mut cues)?;
            continue;
        }
        // Skip NOTE / STYLE / REGION blocks when starting a block
        if buf.is_empty() {
            let up = line.to_ascii_uppercase();
            if up.starts_with("NOTE") || up == "STYLE" || up == "REGION" {
                // skip until blank — consume rest of block via empty-line handling:
                // mark as skip by reading until we see empty (handled by collecting then discard)
                // Simpler: if NOTE..., ignore this line and subsequent until empty
                // Use a flag via empty buffer + special skip mode
            }
        }
        if buf.is_empty() {
            let up = line.to_ascii_uppercase();
            if up.starts_with("NOTE") || up == "STYLE" || up == "REGION" {
                // read until blank without pushing
                // we can't easily skip future lines here; handle below with skip_block
            }
        }
        buf.push(line.to_string());
    }
    flush(&mut buf, &mut cues)?;

    // Drop NOTE/STYLE/REGION-only blocks (no timing)
    cues.retain(|c| !c.text.is_empty());

    // Filter out blocks that were NOTE (they never had --> so weren't added)
    if cues.is_empty() {
        return Err(TextError::message("no WebVTT cues parsed"));
    }
    Ok(cues)
}

fn parse_vtt_range(line: &str) -> Result<(Time, Time)> {
    // "00:00:01.000 --> 00:00:02.000 align:start"
    let mut parts = line.split("-->");
    let left = parts
        .next()
        .map(str::trim)
        .ok_or_else(|| TextError::message(format!("bad vtt timing: {line}")))?;
    let right_full = parts
        .next()
        .map(str::trim)
        .ok_or_else(|| TextError::message(format!("bad vtt timing: {line}")))?;
    // drop cue settings after first whitespace on right side
    let right = right_full.split_whitespace().next().unwrap_or(right_full);
    Ok((parse_vtt_ts(left)?, parse_vtt_ts(right)?))
}

fn parse_vtt_ts(s: &str) -> Result<Time> {
    // HH:MM:SS.mmm or MM:SS.mmm
    let s = s.trim();
    let segs: Vec<&str> = s.split(':').collect();
    let (h_tok, m_tok, s_tok) = match segs.len() {
        3 => (segs[0], segs[1], segs[2]),
        2 => ("0", segs[0], segs[1]),
        _ => return Err(TextError::message(format!("bad vtt timestamp: {s}"))),
    };
    let hour: f64 = h_tok
        .parse()
        .map_err(|_| TextError::message(format!("bad hours in {s}")))?;
    let minute: f64 = m_tok
        .parse()
        .map_err(|_| TextError::message(format!("bad minutes in {s}")))?;
    let second: f64 = s_tok
        .parse()
        .map_err(|_| TextError::message(format!("bad seconds in {s}")))?;
    Ok(Time::from_secs(hour * 3600.0 + minute * 60.0 + second))
}

fn strip_vtt_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            for nc in chars.by_ref() {
                if nc == '>' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vtt() {
        let vtt = "WEBVTT\n\n1\n00:00:01.000 --> 00:00:02.500\nHello <b>world</b>\n\n00:00:03.000 --> 00:00:04.000 align:start\nSecond\n";
        let cues = parse_vtt(vtt).unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "Hello world");
        assert!((cues[0].start.as_secs() - 1.0).abs() < 1e-9);
        assert_eq!(cues[1].text, "Second");
    }
}
