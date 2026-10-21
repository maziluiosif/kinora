//! Local library and "continue watching", persisted to a JSON file.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibItem {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub poster: Option<String>,
    /// Explicitly added to the library (vs. only watched).
    #[serde(default)]
    pub saved: bool,
    #[serde(default)]
    pub last_video: Option<String>,
    #[serde(default)]
    pub last_watched: u64,
    /// Seconds into `last_video` where playback stopped.
    #[serde(default)]
    pub time_offset: f64,
    #[serde(default)]
    pub duration: f64,
    /// Videos played to (almost) the end.
    #[serde(default)]
    pub watched: Vec<String>,
    /// The stream `last_video` was played from (an app-defined key), to replay the same one.
    #[serde(default)]
    pub last_stream: Option<String>,
    /// The subtitles last picked in the player.
    #[serde(default)]
    pub subtitle: Option<SubtitlePref>,
}

/// Subtitles picked for `video`. The same video gets the same track back; other videos of the
/// title (the next episode) get the same language.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubtitlePref {
    pub video: String,
    /// Language code; empty when subtitles were turned off.
    #[serde(default)]
    pub lang: String,
    /// Stream index of an embedded track.
    #[serde(default)]
    pub embedded: Option<usize>,
    /// URL of an online subtitle.
    #[serde(default)]
    pub url: Option<String>,
}

impl LibItem {
    /// Fraction of `last_video` already played.
    pub fn progress(&self) -> f32 {
        if self.duration > 0.0 { (self.time_offset / self.duration).clamp(0.0, 1.0) as f32 } else { 0.0 }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LibUpdate {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub poster: Option<String>,
    pub saved: Option<bool>,
    pub video: Option<String>,
    /// Key of the stream `video` is played from.
    #[serde(default)]
    pub stream: Option<String>,
}

pub struct Library {
    path: PathBuf,
    items: Mutex<Vec<LibItem>>,
}

impl Library {
    pub fn load(path: PathBuf) -> Self {
        let items = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Library { path, items: Mutex::new(items) }
    }

    pub fn items(&self) -> Vec<LibItem> {
        self.items.lock().unwrap().clone()
    }

    pub fn update(&self, u: LibUpdate) -> Result<()> {
        let mut items = self.items.lock().unwrap();
        let idx = match items.iter().position(|i| i.id == u.id) {
            Some(i) => i,
            None => {
                items.push(LibItem {
                    id: u.id.clone(),
                    kind: u.kind.clone(),
                    name: u.name.clone(),
                    poster: None,
                    saved: false,
                    last_video: None,
                    last_watched: 0,
                    time_offset: 0.0,
                    duration: 0.0,
                    watched: Vec::new(),
                    last_stream: None,
                    subtitle: None,
                });
                items.len() - 1
            }
        };
        let item = &mut items[idx];
        item.name = u.name;
        item.poster = u.poster.or(item.poster.take());
        if let Some(saved) = u.saved {
            item.saved = saved;
        }
        if let Some(video) = u.video {
            if item.last_video.as_ref() != Some(&video) {
                item.time_offset = 0.0;
                item.duration = 0.0;
            }
            item.last_video = Some(video);
            item.last_stream = u.stream;
            item.last_watched = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        }
        Self::save(&self.path, &mut items)
    }

    /// Records the playback position in `video`; past 90% it counts as watched.
    pub fn progress(&self, id: &str, video: &str, time_offset: f64, duration: f64) -> Result<()> {
        let mut items = self.items.lock().unwrap();
        let Some(item) = items.iter_mut().find(|i| i.id == id) else { return Ok(()) };
        item.last_video = Some(video.to_string());
        item.time_offset = time_offset;
        item.duration = duration;
        item.last_watched = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        if duration > 0.0 && time_offset / duration > 0.9 && !item.watched.iter().any(|w| w == video) {
            item.watched.push(video.to_string());
        }
        Self::save(&self.path, &mut items)
    }

    pub fn set_subtitle(&self, id: &str, pref: SubtitlePref) -> Result<()> {
        let mut items = self.items.lock().unwrap();
        let Some(item) = items.iter_mut().find(|i| i.id == id) else { return Ok(()) };
        item.subtitle = Some(pref);
        Self::save(&self.path, &mut items)
    }

    /// Removes the item from "continue watching" (it stays in the library if saved).
    pub fn dismiss(&self, id: &str) -> Result<()> {
        let mut items = self.items.lock().unwrap();
        if let Some(item) = items.iter_mut().find(|i| i.id == id) {
            item.last_video = None;
            item.time_offset = 0.0;
            item.duration = 0.0;
        }
        Self::save(&self.path, &mut items)
    }

    fn save(path: &std::path::Path, items: &mut Vec<LibItem>) -> Result<()> {
        items.retain(|i| i.saved || i.last_video.is_some());
        crate::write_atomic(path, &serde_json::to_vec_pretty(&*items)?)
    }
}
