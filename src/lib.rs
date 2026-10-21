//! Shared core of the web server and the desktop client: addons, torrent engine and library.

pub mod addons;
pub mod engine;
pub mod library;
pub mod player;
pub mod server;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::task::JoinSet;

use addons::Addons;
use engine::Engine;
use library::Library;

pub struct Core {
    pub addons: Addons,
    pub engine: Engine,
    pub library: Library,
    /// Port of the local HTTP server that serves `/stream/...`.
    pub(crate) port: u16,
}

/// A catalog of an installed addon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogRef {
    pub addon: String,
    pub addon_name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    pub name: String,
    /// Can be shown without extra arguments (board / discover).
    pub board: bool,
    pub search: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayReq {
    pub info_hash: Option<String>,
    pub file_idx: Option<usize>,
    pub filename: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    /// Direct HTTP stream (non-torrent addons).
    pub url: Option<String>,
    #[serde(default)]
    pub title: String,
    /// Open in the external player rather than only returning the URL.
    #[serde(default)]
    pub external: bool,
}

impl Core {
    /// Loads the config from `~/.config/kinora/` and keeps torrent data in `cache_dir`.
    pub async fn new(port: u16, cache_dir: PathBuf) -> Result<Core> {
        let home = PathBuf::from(std::env::var("HOME").context("HOME not set")?);
        let config_dir = home.join(".config/kinora");
        let first_run = !config_dir.exists();
        std::fs::create_dir_all(&config_dir)?;
        // Copy the library and addons from the pre-rename config directory (left untouched).
        let legacy = home.join(".config/stremio-rs");
        if first_run && legacy.is_dir() {
            for f in ["library.json", "addons.json"] {
                if legacy.join(f).is_file() && std::fs::copy(legacy.join(f), config_dir.join(f)).is_ok() {
                    tracing::info!("imported {f} from {}", legacy.display());
                }
            }
        }

        let http = reqwest::Client::builder()
            .user_agent(concat!("kinora/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Core {
            addons: Addons::load(http, config_dir.join("addons.json")).await,
            engine: Engine::new(cache_dir).await?,
            library: Library::load(config_dir.join("library.json")),
            port,
        })
    }

    /// All catalogs of all addons, flagged by whether they can be shown on the board / searched.
    pub async fn catalogs(&self) -> Vec<CatalogRef> {
        let mut out = Vec::new();
        for a in self.addons.list().await {
            for c in &a.manifest.catalogs {
                out.push(CatalogRef {
                    addon: a.manifest.id.clone(),
                    addon_name: a.manifest.name.clone(),
                    kind: c.kind.clone(),
                    id: c.id.clone(),
                    name: c.name.clone().unwrap_or_else(|| c.id.clone()),
                    board: !c.requires_extra(),
                    search: c.supports_extra("search"),
                });
            }
        }
        out
    }

    pub async fn catalog(&self, addon: &str, kind: &str, id: &str, extra: &[(String, String)]) -> Result<Value> {
        let addons = self.addons.list().await;
        let a = addons.iter().find(|a| a.manifest.id == addon).context("unknown addon")?;
        a.get(&self.addons.http, "catalog", kind, id, extra).await
    }

    /// Searches every searchable catalog; one `{addon, name, metas}` row per catalog.
    pub async fn search(&self, q: &str) -> Vec<Value> {
        let mut set = JoinSet::new();
        for (order, a) in self.addons.list().await.into_iter().enumerate() {
            for c in a.manifest.catalogs.iter().filter(|c| c.supports_extra("search")).cloned() {
                let (a, http, q) = (a.clone(), self.addons.http.clone(), q.to_string());
                set.spawn(async move {
                    let extra = [("search".to_string(), q)];
                    let res = a.get(&http, "catalog", &c.kind, &c.id, &extra).await;
                    let title = format!("{} - {}", c.name.unwrap_or(c.id), c.kind);
                    (order, a.manifest.name, title, res)
                });
            }
        }
        let mut rows = Vec::new();
        while let Some(Ok((order, addon, title, res))) = set.join_next().await {
            match res {
                Ok(v) => rows.push((order, json!({ "addon": addon, "name": title, "metas": v["metas"] }))),
                Err(e) => tracing::warn!("search failed in {addon}: {e:#}"),
            }
        }
        rows.sort_by_key(|(o, _)| *o);
        rows.into_iter().map(|(_, r)| r).collect()
    }

    /// `{meta}` from the first addon that has metadata for this id.
    pub async fn meta(&self, kind: &str, id: &str) -> Result<Value> {
        for a in self.addons.list().await.iter().filter(|a| a.manifest.supports("meta", kind, id)) {
            match a.get(&self.addons.http, "meta", kind, id, &[]).await {
                Ok(v) if v["meta"].is_object() => return Ok(v),
                Ok(_) => {}
                Err(e) => tracing::warn!("meta failed in {}: {e:#}", a.manifest.name),
            }
        }
        anyhow::bail!("no addon returned metadata for {kind}/{id}")
    }

    /// Streams from every addon that provides them, each tagged with its addon name.
    pub async fn streams(&self, kind: &str, id: &str) -> Vec<Value> {
        let mut set = JoinSet::new();
        for (order, a) in self.addons.list().await.into_iter().enumerate() {
            if !a.manifest.supports("stream", kind, id) {
                continue;
            }
            let (http, kind, id) = (self.addons.http.clone(), kind.to_string(), id.to_string());
            set.spawn(async move { (order, a.manifest.name.clone(), a.get(&http, "stream", &kind, &id, &[]).await) });
        }
        let mut results = Vec::new();
        while let Some(Ok((order, addon, res))) = set.join_next().await {
            match res {
                Ok(v) => results.push((order, addon, v)),
                Err(e) => tracing::warn!("streams failed in {addon}: {e:#}"),
            }
        }
        results.sort_by_key(|(o, _, _)| *o);
        let mut out = Vec::new();
        for (_, addon, v) in results {
            for mut st in v["streams"].as_array().cloned().unwrap_or_default() {
                st["addon"] = json!(addon);
                out.push(st);
            }
        }
        out
    }

    /// Starts a stream (torrent or direct URL); returns its local URL and, for torrents, the engine info.
    pub async fn play(&self, req: PlayReq) -> Result<Value> {
        let mut res = json!({});
        let url = if let Some(url) = req.url {
            // The URL comes from an addon and may end up on a player's command line.
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                anyhow::bail!("unsupported stream URL: {url}");
            }
            url
        } else {
            let hash = req.info_hash.context("stream has neither infoHash nor url")?;
            let st = self.engine.start(&hash, req.file_idx, req.filename.as_deref(), &req.sources).await?;
            let url = format!(
                "http://127.0.0.1:{}/stream/{}/{}/{}",
                self.port,
                st.torrent_id,
                st.file_idx,
                urlencoding::encode(&st.file_name)
            );
            res = json!(st);
            url
        };
        res["url"] = json!(url);
        if req.external {
            res["player"] = json!(player::launch(&url, &req.title)?);
        }
        Ok(res)
    }

    pub fn status(&self, torrent_id: usize) -> Result<Value> {
        let t = self.engine.get(torrent_id).context("torrent not found")?;
        Ok(json!(t.stats()))
    }
}

/// Writes through a temporary file so a crash mid-write can't leave a truncated config.
pub(crate) fn write_atomic(path: &std::path::Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
