//! Typed views of the addon JSON the desktop UI renders. Everything is optional: addons vary a lot.

use serde::{Deserialize, Deserializer};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaPreview {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    pub poster: Option<String>,
    pub background: Option<String>,
    pub logo: Option<String>,
    pub description: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub release_info: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub imdb_rating: Option<String>,
    #[serde(default, deserialize_with = "strings")]
    pub genres: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchRow {
    pub addon: String,
    pub name: String,
    #[serde(default, deserialize_with = "metas")]
    pub metas: Vec<MetaPreview>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    #[serde(default)]
    pub name: String,
    pub poster: Option<String>,
    pub background: Option<String>,
    pub logo: Option<String>,
    pub description: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub release_info: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub runtime: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub imdb_rating: Option<String>,
    #[serde(default, deserialize_with = "strings")]
    pub genres: Vec<String>,
    #[serde(default, deserialize_with = "strings")]
    pub cast: Vec<String>,
    #[serde(default, deserialize_with = "strings")]
    pub director: Vec<String>,
    #[serde(default)]
    pub videos: Vec<Video>,
    #[serde(default)]
    pub trailer_streams: Vec<Trailer>,
    #[serde(default)]
    pub behavior_hints: MetaHints,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaHints {
    pub default_video_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trailer {
    pub yt_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Video {
    pub id: String,
    #[serde(default, deserialize_with = "number")]
    pub season: u32,
    #[serde(default, deserialize_with = "number")]
    pub episode: u32,
    pub name: Option<String>,
    pub title: Option<String>,
    pub thumbnail: Option<String>,
    pub released: Option<String>,
    pub overview: Option<String>,
    pub description: Option<String>,
}

impl Video {
    pub fn label(&self) -> String {
        let name = self.name.as_deref().or(self.title.as_deref()).unwrap_or_default();
        if name.is_empty() { format!("Episode {}", self.episode) } else { name.to_string() }
    }

    pub fn overview(&self) -> &str {
        self.overview.as_deref().or(self.description.as_deref()).unwrap_or_default()
    }

    /// "S1E4", or "" for specials without numbers.
    pub fn code(&self) -> String {
        if self.season == 0 && self.episode == 0 { String::new() } else { format!("S{}E{}", self.season, self.episode) }
    }

    /// Release date (YYYY-MM-DD), if any.
    pub fn date(&self) -> &str {
        self.released.as_deref().and_then(|d| d.get(..10)).unwrap_or_default()
    }
}

/// Resolution class of a stream, used for its badge and the quick filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Uhd,
    FullHd,
    Hd,
    Sd,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Uhd => "4K",
            Tier::FullHd => "1080p",
            Tier::Hd => "720p",
            Tier::Sd => "SD",
        }
    }
}

/// What the stream list shows, parsed out of an addon's free-form name and title.
pub struct StreamInfo {
    pub tier: Tier,
    /// Short quality tag, e.g. "1080p" or "4k HDR".
    pub quality: String,
    /// First line of the title, usually the file or release name.
    pub release: String,
    /// The remaining lines (languages, notes) without the stats.
    pub notes: String,
    pub seeders: Option<u32>,
    pub size: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    #[serde(default)]
    pub addon: String,
    pub name: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub info_hash: Option<String>,
    pub file_idx: Option<usize>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub behavior_hints: StreamHints,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StreamHints {
    pub filename: Option<String>,
}

impl Stream {
    /// Quality column, e.g. "1080p" (Torrentio prefixes its own name).
    pub fn quality(&self) -> String {
        let name = self.name.as_deref().unwrap_or_default();
        let q = name.strip_prefix("Torrentio").unwrap_or(name).trim();
        if q.is_empty() { self.addon.clone() } else { q.to_string() }
    }

    /// Description with flag emojis turned into country codes (egui's fonts have no flags).
    pub fn text(&self) -> String {
        let text = self.title.as_deref().or(self.description.as_deref()).or(self.url.as_deref());
        text.unwrap_or_default()
            .chars()
            .map(|c| match c as u32 {
                0x1F1E6..=0x1F1FF => char::from(b'A' + (c as u32 - 0x1F1E6) as u8),
                _ => c,
            })
            .collect()
    }

    pub fn info(&self) -> StreamInfo {
        let quality = self.quality();
        let q = format!("{} {}", quality, self.title.as_deref().unwrap_or_default()).to_lowercase();
        let tier = if q.contains("2160") || q.contains("4k") || q.contains("uhd") {
            Tier::Uhd
        } else if q.contains("1080") {
            Tier::FullHd
        } else if q.contains("720") {
            Tier::Hd
        } else {
            Tier::Sd
        };
        let (mut seeders, mut size, mut source) = (None, None, None);
        let mut lines = Vec::new();
        for line in self.text().lines().map(str::trim).filter(|l| !l.is_empty()) {
            let mut stats = false;
            if let Some(rest) = after(line, '👤') {
                seeders = rest.split_whitespace().next().and_then(|n| n.parse().ok());
                stats = true;
            }
            if let Some(rest) = after(line, '💾') {
                size = Some(rest.to_string()).filter(|s| !s.is_empty());
                stats = true;
            }
            if let Some(rest) = after(line, '⚙') {
                source = Some(rest.to_string()).filter(|s| !s.is_empty());
                stats = true;
            }
            if !stats {
                lines.push(line.to_string());
            }
        }
        let release = if lines.is_empty() { String::new() } else { lines.remove(0) };
        StreamInfo { tier, quality, release, notes: lines.join(" · "), seeders, size, source }
    }

    /// Identifies the stream across lists: torrent and file, or the URL.
    pub fn key(&self) -> String {
        match (&self.info_hash, &self.url) {
            (Some(h), _) => format!("{}:{}", h.to_lowercase(), self.file_idx.map_or(String::new(), |i| i.to_string())),
            (None, Some(u)) => u.clone(),
            _ => self.addon.clone() + self.name.as_deref().unwrap_or_default() + self.title.as_deref().unwrap_or_default(),
        }
    }

    /// Lowercased text the stream filter matches against.
    pub fn haystack(&self) -> String {
        [self.name.as_deref(), self.title.as_deref(), self.description.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }
}

/// The text after `marker` in `line`, up to the next stats marker.
fn after(line: &str, marker: char) -> Option<&str> {
    let rest = &line[line.find(marker)? + marker.len_utf8()..];
    let end = rest.find(['👤', '💾', '⚙']).unwrap_or(rest.len());
    Some(rest[..end].trim_start_matches('\u{fe0f}').trim())
}

/// Accepts strings or numbers (`"2019"`, `2019`, `7.5`).
fn lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) if !s.is_empty() => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn number<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n.as_u64().unwrap_or(0) as u32,
        Value::String(s) => s.parse().unwrap_or(0),
        _ => 0,
    })
}

/// Accepts a string or a list of strings.
fn strings<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => vec![s],
        Value::Array(a) => a.into_iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        _ => Vec::new(),
    })
}

/// Skips malformed items instead of failing the whole list.
fn metas<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<MetaPreview>, D::Error> {
    Ok(parse_metas(Value::deserialize(d)?))
}

pub fn parse_metas(v: Value) -> Vec<MetaPreview> {
    match v {
        Value::Array(a) => a.into_iter().filter_map(|m| serde_json::from_value(m).ok()).collect(),
        _ => Vec::new(),
    }
}

pub fn parse_streams(v: Vec<Value>) -> Vec<Stream> {
    v.into_iter().filter_map(|s| serde_json::from_value(s).ok()).collect()
}
