//! External subtitles: the addon `subtitles` resource, download and SRT/WebVTT parsing.

use serde::{Deserialize, Serialize};

/// Used when no installed addon provides subtitles.
pub const OPENSUBTITLES: &str = "https://opensubtitles-v3.strem.io/manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleTrack {
    #[serde(default)]
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub addon: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

pub fn is_romanian(lang: &str) -> bool {
    matches!(lang.to_lowercase().as_str(), "ron" | "rum" | "ro" | "romanian" | "română")
}

/// Display name for the ISO 639 codes addons use.
pub fn lang_name(code: &str) -> String {
    let name = match code.to_lowercase().as_str() {
        "ron" | "rum" | "ro" => "Romanian",
        "eng" | "en" => "English",
        "spa" | "es" => "Spanish",
        "fre" | "fra" | "fr" => "French",
        "ger" | "deu" | "de" => "German",
        "ita" | "it" => "Italian",
        "por" | "pt" => "Portuguese",
        "pob" => "Portuguese (BR)",
        "hun" | "hu" => "Hungarian",
        "rus" | "ru" => "Russian",
        "ukr" | "uk" => "Ukrainian",
        "bul" | "bg" => "Bulgarian",
        "srp" | "scc" | "sr" => "Serbian",
        "hrv" | "hr" => "Croatian",
        "slv" | "sl" => "Slovenian",
        "cze" | "ces" | "cs" => "Czech",
        "slo" | "slk" | "sk" => "Slovak",
        "pol" | "pl" => "Polish",
        "ell" | "gre" | "el" => "Greek",
        "tur" | "tr" => "Turkish",
        "dut" | "nld" | "nl" => "Dutch",
        "swe" | "sv" => "Swedish",
        "nor" | "no" => "Norwegian",
        "dan" | "da" => "Danish",
        "fin" | "fi" => "Finnish",
        "heb" | "he" => "Hebrew",
        "ara" | "ar" => "Arabic",
        "chi" | "zho" | "zh" => "Chinese",
        "jpn" | "ja" => "Japanese",
        "kor" | "ko" => "Korean",
        _ => return code.to_uppercase(),
    };
    name.to_string()
}

/// Romanian first, then English, then the rest; the addon's order otherwise.
pub fn sort_tracks(tracks: &mut [SubtitleTrack]) {
    tracks.sort_by_key(|t| match t.lang.to_lowercase().as_str() {
        l if is_romanian(l) => 0,
        "eng" | "en" => 1,
        _ => 2,
    });
}

/// UTF-8 when valid, otherwise Windows-1250 (the usual legacy encoding of Romanian subtitles).
pub fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1250.decode(bytes).0.into_owned(),
    }
}

/// Parses SRT or WebVTT. Styling tags (`<i>`, `{\an8}`) are dropped.
pub fn parse(text: &str) -> Vec<Cue> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = Vec::new();
    for block in text.split("\n\n") {
        let mut lines = block.lines().skip_while(|l| !l.contains("-->"));
        let Some(timing) = lines.next() else { continue };
        let Some((start, end)) = timing.split_once("-->") else { continue };
        // WebVTT puts cue settings after the end time.
        let end = end.split_whitespace().next().unwrap_or_default();
        let (Some(start), Some(end)) = (parse_time(start.trim()), parse_time(end)) else { continue };
        let body: Vec<String> = lines.map(strip_tags).filter(|l| !l.trim().is_empty()).collect();
        if !body.is_empty() && end > start {
            cues.push(Cue { start, end, text: body.join("\n") });
        }
    }
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    cues
}

/// `hh:mm:ss,mmm`, `hh:mm:ss.mmm` or `mm:ss.mmm`.
fn parse_time(s: &str) -> Option<f64> {
    let s = s.replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<f64>().ok()?, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        [m, s] => (0.0, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        _ => return None,
    };
    Some(h * 3600.0 + m * 60.0 + sec)
}

fn strip_tags(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut skip = None;
    for c in line.chars() {
        match (skip, c) {
            (None, '<') => skip = Some('>'),
            (None, '{') => skip = Some('}'),
            (Some(close), c) if c == close => skip = None,
            (Some(_), _) => {}
            (None, c) => out.push(c),
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt() {
        let s = "1\r\n00:01:51,524 --> 00:01:53,567\r\n<i>Domnule</i> Dufresne,\r\n{\\an8}descrie-mi\r\n\r\n2\r\n00:01:53,610 --> 00:01:58,615\r\nAl doilea\r\n";
        let cues = parse(s);
        assert_eq!(cues.len(), 2);
        assert!((cues[0].start - 111.524).abs() < 1e-9);
        assert_eq!(cues[0].text, "Domnule Dufresne,\ndescrie-mi");
        assert!((cues[1].end - 118.615).abs() < 1e-9);
    }

    #[test]
    fn vtt() {
        let s = "WEBVTT\n\n00:05.000 --> 00:07.500 align:start\nHello\n\nnote\n01:00:00.000 --> 01:00:01.000\nLate";
        let cues = parse(s);
        assert_eq!(cues, vec![
            Cue { start: 5.0, end: 7.5, text: "Hello".into() },
            Cue { start: 3600.0, end: 3601.0, text: "Late".into() },
        ]);
    }

    #[test]
    fn cp1250() {
        // "ţară" in Windows-1250.
        assert_eq!(decode(&[0xFE, 0x61, 0x72, 0xE3]), "ţară");
        assert_eq!(decode("\u{feff}șa".as_bytes()), "șa");
    }
}
