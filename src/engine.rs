//! Torrent streaming engine on top of librqbit (magnet → selected file → HTTP stream).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

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

/// Downloaded torrents are kept up to this size in total (`KINORA_CACHE_GB` overrides it).
pub const DEFAULT_CACHE_LIMIT: u64 = 2 << 30;

/// One torrent streams at a time. Its data stays on disk after playback, so replaying is
/// instant: `{dir}/{info_hash}/` holds the files and `{dir}/{info_hash}.torrent` the metadata
/// (no waiting for peers to send it). Least recently played torrents go once the cache is full.
pub struct Engine {
    session: Arc<Session>,
    dir: PathBuf,
    limit: u64,
    /// Session id and info hash of the torrent being streamed.
    current: Mutex<Option<(usize, String)>>,
}

#[derive(Serialize)]
pub struct Started {
    pub torrent_id: usize,
    pub file_idx: usize,
    pub file_name: String,
    pub file_len: u64,
}

impl Engine {
    pub async fn new(dir: PathBuf, limit: u64) -> Result<Self> {
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
                Session::new_with_opts(dir.clone(), opts(false)).await?
            }
        };
        evict(&dir, limit, None);
        Ok(Engine { session, dir, limit, current: Mutex::new(None) })
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
        // The hash names files on disk, so only accept the hex form.
        let hash = info_hash.to_lowercase();
        if hash.len() != 40 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            anyhow::bail!("invalid info hash: {info_hash}");
        }
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
            output_folder: Some(self.dir.join(&hash).to_string_lossy().into_owned()),
            ..Default::default()
        };
        let torrent_file = self.dir.join(format!("{hash}.torrent"));
        let add = match std::fs::read(&torrent_file) {
            Ok(bytes) => AddTorrent::from_bytes(bytes),
            Err(_) => AddTorrent::from_url(format!("magnet:?xt=urn:btih:{hash}")),
        };
        let add = self.session.add_torrent(add, Some(opts));
        let (id, handle) = match tokio::time::timeout(Duration::from_secs(90), add)
            .await
            .context("timed out fetching torrent metadata (no peers?)")??
        {
            AddTorrentResponse::Added(id, h) | AddTorrentResponse::AlreadyManaged(id, h) => (id, h),
            AddTorrentResponse::ListOnly(_) => unreachable!("list_only is not set"),
        };

        let prev = self.current.lock().await.replace((id, hash.clone()));
        if let Some((prev, _)) = prev.filter(|(p, _)| *p != id) {
            // Forget it but keep its data for next time.
            let _ = self.session.delete(TorrentIdOrHash::Id(prev), false).await;
        }
        handle.wait_until_initialized().await?;

        let (files, bytes) = handle.with_metadata(|m| {
            let files: Vec<(String, u64)> = m
                .file_infos
                .iter()
                .map(|f| (f.relative_filename.to_string_lossy().into_owned(), f.len))
                .collect();
            (files, m.torrent_bytes.clone())
        })?;
        if !torrent_file.exists()
            && let Err(e) = std::fs::write(&torrent_file, &bytes)
        {
            tracing::warn!("caching {}: {e}", torrent_file.display());
        }
        // Marks it as the most recently played.
        let _ = std::fs::File::options().write(true).open(&torrent_file).and_then(|f| f.set_modified(SystemTime::now()));
        evict(&self.dir, self.limit, Some(&hash));

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

    /// Pauses the current torrent. It stays loaded, so playing it again resumes immediately.
    pub async fn stop(&self) {
        let current = self.current.lock().await.clone();
        if let Some((id, _)) = current
            && let Some(handle) = self.get(id)
            && !handle.is_paused()
            && let Err(e) = self.session.pause(&handle).await
        {
            tracing::warn!("pausing torrent {id}: {e:#}");
        }
    }
}

/// Deletes the least recently played torrents until the cache fits `limit`. The torrent in use
/// (`keep`) and the most recent one are never deleted, even when larger than the limit.
fn evict(dir: &Path, limit: u64, keep: Option<&str>) {
    let mut entries = cache_entries(dir);
    entries.sort_by_key(|e| std::cmp::Reverse(e.used));
    let protected = |i: usize, e: &CacheEntry| i == 0 || keep == Some(e.hash.as_str());
    let mut total: u64 = entries.iter().enumerate().filter(|(i, e)| protected(*i, e)).map(|(_, e)| e.size).sum();
    for (i, e) in entries.iter().enumerate() {
        if protected(i, e) {
            continue;
        }
        if total + e.size <= limit {
            total += e.size;
            continue;
        }
        tracing::info!(hash = %e.hash, size = e.size, "evicting from cache");
        let _ = std::fs::remove_dir_all(dir.join(&e.hash));
        let _ = std::fs::remove_file(dir.join(format!("{}.torrent", e.hash)));
    }
}

struct CacheEntry {
    hash: String,
    /// Bytes on disk (pieces not downloaded don't count: the files are sparse).
    size: u64,
    used: SystemTime,
}

/// The torrents in the cache directory, by info hash.
fn cache_entries(dir: &Path) -> Vec<CacheEntry> {
    let mut out: Vec<CacheEntry> = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let hash = name.strip_suffix(".torrent").unwrap_or(&name).to_string();
        if hash.len() != 40 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        // Playing touches the .torrent; the folder's own time only counts without one.
        let is_torrent = name.ends_with(".torrent");
        let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let size = disk_usage(&entry.path());
        match out.iter_mut().find(|e| e.hash == hash) {
            Some(e) => {
                e.size += size;
                if is_torrent {
                    e.used = used;
                }
            }
            None => out.push(CacheEntry { hash, size, used }),
        }
    }
    out
}

fn disk_usage(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if meta.is_dir() {
        return std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| disk_usage(&e.path())).sum();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.blocks() * 512
    }
    #[cfg(not(unix))]
    meta.len()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction() {
        let dir = std::env::temp_dir().join(format!("kinora-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let hashes: Vec<String> = (0..4).map(|i| format!("{i}").repeat(40)).collect();
        let now = SystemTime::now();
        for (age, h) in hashes.iter().enumerate() {
            std::fs::create_dir_all(dir.join(h)).unwrap();
            std::fs::write(dir.join(h).join("video.mkv"), vec![1u8; 64 * 1024]).unwrap();
            let t = dir.join(format!("{h}.torrent"));
            std::fs::write(&t, b"d4:infode").unwrap();
            let f = std::fs::File::options().write(true).open(&t).unwrap();
            f.set_modified(now - Duration::from_secs(60 * age as u64)).unwrap();
        }
        let size = cache_entries(&dir)[0].size;
        let left = |dir: &Path| {
            let mut v: Vec<String> = cache_entries(dir).into_iter().map(|e| e.hash).collect();
            v.sort();
            v
        };
        // Room for two and a half entries, but the oldest one is in use.
        evict(&dir, size * 2 + size / 2, Some(&hashes[3]));
        assert_eq!(left(&dir), vec![hashes[0].clone(), hashes[3].clone()]);
        // A limit below one entry still keeps the most recent.
        evict(&dir, 1, None);
        assert_eq!(left(&dir), vec![hashes[0].clone()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
