//! Torrent streaming engine on top of librqbit (magnet → selected file → HTTP stream).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use librqbit::api::TorrentIdOrHash;
use librqbit::dht::DhtPersistenceConfig;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, DhtSessionConfig, ManagedTorrent, Session, SessionOptions,
};
use serde::Serialize;
use tokio::sync::Mutex;

const DEFAULT_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://explodie.org:6969/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
];

const VIDEO_EXTS: &[&str] = &["mkv", "mp4", "avi", "m4v", "webm", "mov", "ts", "wmv"];

pub struct Engine {
    session: Arc<Session>,
    /// Only one torrent is kept at a time; the previous one is deleted on switch.
    current: Mutex<Option<usize>>,
}

#[derive(Serialize)]
pub struct Started {
    pub torrent_id: usize,
    pub file_idx: usize,
    pub file_name: String,
    pub file_len: u64,
}

impl Engine {
    pub async fn new(dir: PathBuf) -> Result<Self> {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        // librqbit's default DHT state file is shared by every app using it and pins the UDP port,
        // so a second instance (web + desktop, or another rqbit app) failed to start. Keep our own
        // file next to the cache, and fall back to a fresh DHT on a random port if it's taken.
        let dht_file = dir.with_extension("dht.json");
        let opts = |persist: bool| SessionOptions {
            dht: Some(DhtSessionConfig {
                persistence: persist.then(|| DhtPersistenceConfig {
                    config_filename: Some(dht_file.clone()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let session = match Session::new_with_opts(dir.clone(), opts(true)).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("persistent DHT unavailable ({e:#}), starting without it");
                Session::new_with_opts(dir, opts(false)).await?
            }
        };
        Ok(Engine { session, current: Mutex::new(None) })
    }

    pub fn get(&self, id: usize) -> Option<Arc<ManagedTorrent>> {
        self.session.get(TorrentIdOrHash::Id(id))
    }

    pub async fn start(
        &self,
        info_hash: &str,
        file_idx: Option<usize>,
        filename: Option<&str>,
        sources: &[String],
    ) -> Result<Started> {
        let trackers = sources
            .iter()
            .filter_map(|s| s.strip_prefix("tracker:"))
            .chain(DEFAULT_TRACKERS.iter().copied())
            .map(String::from)
            .collect();
        let opts = AddTorrentOptions {
            paused: true,
            overwrite: true,
            trackers: Some(trackers),
            ..Default::default()
        };
        let magnet = format!("magnet:?xt=urn:btih:{}", info_hash.to_lowercase());
        let add = self.session.add_torrent(AddTorrent::from_url(magnet), Some(opts));
        let (id, handle) = match tokio::time::timeout(Duration::from_secs(90), add)
            .await
            .context("timed out fetching torrent metadata (no peers?)")??
        {
            AddTorrentResponse::Added(id, h) | AddTorrentResponse::AlreadyManaged(id, h) => (id, h),
            AddTorrentResponse::ListOnly(_) => unreachable!("list_only is not set"),
        };

        let prev = self.current.lock().await.replace(id);
        if let Some(prev) = prev.filter(|p| *p != id) {
            let _ = self.session.delete(TorrentIdOrHash::Id(prev), true).await;
        }
        handle.wait_until_initialized().await?;

        let files: Vec<(String, u64)> = handle.with_metadata(|m| {
            m.file_infos
                .iter()
                .map(|f| (f.relative_filename.to_string_lossy().into_owned(), f.len))
                .collect()
        })?;
        let fid = pick_file(&files, file_idx, filename).context("torrent has no files")?;
        self.session.update_only_files(&handle, &HashSet::from([fid])).await?;
        if handle.is_paused() {
            self.session.unpause(&handle).await?;
        }

        let (path, file_len) = &files[fid];
        let file_name = path.rsplit('/').next().unwrap_or(path).to_string();
        tracing::info!(torrent_id = id, file_idx = fid, %file_name, "streaming");
        Ok(Started { torrent_id: id, file_idx: fid, file_name, file_len: *file_len })
    }

    pub async fn stop(&self) {
        if let Some(id) = self.current.lock().await.take() {
            let _ = self.session.delete(TorrentIdOrHash::Id(id), true).await;
        }
    }
}

fn is_video(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| VIDEO_EXTS.contains(&ext.to_lowercase().as_str()))
}

fn pick_file(files: &[(String, u64)], idx: Option<usize>, filename: Option<&str>) -> Option<usize> {
    if let Some(i) = idx.filter(|i| *i < files.len()) {
        return Some(i);
    }
    if let Some(name) = filename
        && let Some(i) = files.iter().position(|(p, _)| p.ends_with(name))
    {
        return Some(i);
    }
    let largest = |videos_only: bool| {
        files
            .iter()
            .enumerate()
            .filter(|(_, (p, _))| !videos_only || is_video(p))
            .max_by_key(|(_, (_, len))| *len)
            .map(|(i, _)| i)
    };
    largest(true).or_else(|| largest(false))
}

pub fn mime_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("mkv") => "video/x-matroska",
        Some("webm") => "video/webm",
        Some("avi") => "video/x-msvideo",
        Some("mov") => "video/quicktime",
        Some("ts") => "video/mp2t",
        _ => "application/octet-stream",
    }
}
