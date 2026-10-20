//! SRT ⇄ WebVTT conversion and subtitle-to-text.

use crate::error::{CoreError, Result};
use crate::format::Format;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// Parse `hh:mm:ss,mmm`, `hh:mm:ss.mmm` or `mm:ss.mmm`.
fn parse_time(s: &str) -> Option<u64> {
    let s = s.trim();
    let (clock, frac) = s.rsplit_once([',', '.'])?;
    let ms: u64 = format!("{frac:0<3}")[..3].parse().ok()?;
    let parts: Vec<u64> = clock
        .split(':')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [h, m, s] => h * 3600 + m * 60 + s,
        [m, s] => m * 60 + s,
        _ => return None,
    };
    Some(secs * 1000 + ms)
}

fn fmt_time(ms: u64, sep: char) -> String {
    format!(
        "{:02}:{:02}:{:02}{}{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        sep,
        ms % 1000
    )
}

/// Parse SRT or WebVTT. Both use blocks separated by blank lines with a
/// `start --> end` timing line; anything before the timing line in a block
/// (an SRT index or a VTT cue id) is ignored.
pub fn parse(input: &str) -> Result<Vec<Cue>> {
    let text = input
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut cues = Vec::new();
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        let Some(pos) = lines.iter().position(|l| l.contains("-->")) else {
            continue;
        };
        let (a, b) = lines[pos].split_once("-->").unwrap();
        // VTT may append cue settings after the end time: "00:01.000 align:start"
        let b = b.split_whitespace().next().unwrap_or("");
        let (Some(start_ms), Some(end_ms)) = (parse_time(a), parse_time(b)) else {
            return Err(CoreError::InvalidInput(format!(
                "bad timing line: {}",
                lines[pos]
            )));
        };
        let body = lines[pos + 1..].join("\n");
        cues.push(Cue {
            start_ms,
            end_ms,
            text: body.trim_end().to_string(),
        });
    }
    if cues.is_empty() {
        return Err(CoreError::InvalidInput("no subtitle cues found".into()));
    }
    Ok(cues)
}

pub fn to_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (i, c) in cues.iter().enumerate() {
        out += &format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            fmt_time(c.start_ms, ','),
            fmt_time(c.end_ms, ','),
            c.text
        );
    }
    out
}

pub fn to_vtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for c in cues {
        out += &format!(
            "{} --> {}\n{}\n\n",
            fmt_time(c.start_ms, '.'),
            fmt_time(c.end_ms, '.'),
            c.text
        );
    }
    out
}

/// Dialogue only, with simple markup tags removed.
pub fn to_text(cues: &[Cue]) -> String {
    let mut out = String::new();
    for c in cues {
        let mut inside = false;
        for ch in c.text.chars() {
            match ch {
                '<' => inside = true,
                '>' if inside => inside = false,
                _ if !inside => out.push(ch),
                _ => {}
            }
        }
        out.push('\n');
    }
    out
}

pub fn convert(input: &str, to: Format) -> Result<String> {
    let cues = parse(input)?;
    Ok(match to {
        Format::Srt => to_srt(&cues),
        Format::Vtt => to_vtt(&cues),
        Format::Txt => to_text(&cues),
        other => {
            return Err(CoreError::InvalidInput(format!(
                "subtitles can't become {other}"
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = "1\r\n00:00:01,000 --> 00:00:02,500\r\nHello <i>there</i>\r\n\r\n2\r\n00:01:00,250 --> 01:00:00,000\r\nSecond\r\nline\r\n";
    const VTT: &str = "WEBVTT\n\nintro\n00:01.000 --> 00:02.500 align:start\nHello <i>there</i>\n\n00:01:00.250 --> 01:00:00.000\nSecond\nline\n";

    #[test]
    fn srt_and_vtt_parse_identically() {
        assert_eq!(parse(SRT).unwrap(), parse(VTT).unwrap());
    }

    #[test]
    fn round_trips() {
        let cues = parse(SRT).unwrap();
        assert_eq!(cues[1].start_ms, 60_250);
        assert_eq!(cues[1].end_ms, 3_600_000);
        assert_eq!(parse(&to_vtt(&cues)).unwrap(), cues);
        assert_eq!(parse(&to_srt(&cues)).unwrap(), cues);
        assert!(to_vtt(&cues).starts_with("WEBVTT"));
        assert!(to_srt(&cues).contains("00:00:01,000 --> 00:00:02,500"));
    }

    #[test]
    fn text_strips_tags() {
        assert_eq!(
            convert(SRT, Format::Txt).unwrap(),
            "Hello there\nSecond\nline\n"
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("just some words").is_err());
    }
}
