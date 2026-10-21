//! Client for the Stremio addon protocol: manifests, catalogs, meta and streams.
//! Spec: https://github.com/Stremio/stremio-addon-sdk/blob/master/docs/protocol.md

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use tokio::task::JoinSet;

pub const CINEMETA: &str = "https://v3-cinemeta.strem.io/manifest.json";
/// Torrentio with Romanian results first and CAM/SCR/3D releases filtered out.
pub const TORRENTIO: &str =
    "https://torrentio.strem.fun/language=romanian%7Cqualityfilter=cam,scr,threed/manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Resource {
    Name(String),
    Full {
        name: String,
        #[serde(default)]
        types: Option<Vec<String>>,
        #[serde(default, rename = "idPrefixes")]
        id_prefixes: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtraProp {
    pub name: String,
    #[serde(default, rename = "isRequired")]
    pub is_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub extra: Vec<ExtraProp>,
    // Legacy manifest fields.
    #[serde(default, rename = "extraRequired")]
    pub extra_required: Vec<String>,
    #[serde(default, rename = "extraSupported")]
    pub extra_supported: Vec<String>,
}

impl Catalog {
    pub fn supports_extra(&self, name: &str) -> bool {
        self.extra.iter().any(|e| e.name == name) || self.extra_supported.iter().any(|e| e == name)
    }

    pub fn requires_extra(&self) -> bool {
        self.extra.iter().any(|e| e.is_required) || !self.extra_required.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub resources: Vec<Resource>,
    #[serde(default)]
    pub types: Vec<String>,
    #[serde(default)]
    pub catalogs: Vec<Catalog>,
    #[serde(default, rename = "idPrefixes")]
    pub id_prefixes: Option<Vec<String>>,
}

impl Manifest {
    pub fn supports(&self, resource: &str, kind: &str, id: &str) -> bool {
        self.resources.iter().any(|r| {
            let (name, types, prefixes) = match r {
                Resource::Name(n) => (n.as_str(), None, None),
                Resource::Full { name, types, id_prefixes } => {
                    (name.as_str(), types.as_ref(), id_prefixes.as_ref())
                }
            };
            name == resource
                && types.unwrap_or(&self.types).iter().any(|t| t == kind)
                && prefixes
                    .or(self.id_prefixes.as_ref())
                    .is_none_or(|p| p.iter().any(|p| id.starts_with(p.as_str())))
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Addon {
    pub url: String,
    pub manifest: Manifest,
}

impl Addon {
    pub async fn fetch(http: &reqwest::Client, url: &str) -> Result<Addon> {
        let url = url.trim().replacen("stremio://", "https://", 1);
        if !url.ends_with("/manifest.json") {
            bail!("addon URL must end with /manifest.json");
        }
        let manifest = http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json::<Manifest>()
            .await
            .with_context(|| format!("invalid manifest at {url}"))?;
        Ok(Addon { url, manifest })
    }

    fn base(&self) -> &str {
        self.url.strip_suffix("/manifest.json").unwrap_or(&self.url)
    }

    /// GET `{base}/{resource}/{type}/{id}[/{extra}].json`
    pub async fn get(
        &self,
        http: &reqwest::Client,
        resource: &str,
        kind: &str,
        id: &str,
        extra: &[(String, String)],
    ) -> Result<Value> {
        let mut url = format!("{}/{resource}/{kind}/{}", self.base(), urlencoding::encode(id));
        if !extra.is_empty() {
            let extra = extra
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url.push('/');
            url.push_str(&extra);
        }
        url.push_str(".json");
        tracing::debug!(%url, "addon request");
        Ok(http.get(&url).send().await?.error_for_status()?.json().await?)
    }
}

/// The installed addons, persisted as a list of manifest URLs.
pub struct Addons {
    pub http: reqwest::Client,
    path: PathBuf,
    list: RwLock<Vec<Addon>>,
    /// Installed addons whose manifest failed to load; kept so saving doesn't uninstall them.
    unavailable: std::sync::Mutex<Vec<String>>,
}

impl Addons {
    pub async fn load(http: reqwest::Client, path: PathBuf) -> Self {
        let urls: Vec<String> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| vec![CINEMETA.into(), TORRENTIO.into()]);
        let mut set = JoinSet::new();
        for (order, url) in urls.into_iter().enumerate() {
            let http = http.clone();
            set.spawn(async move { (order, Addon::fetch(&http, &url).await, url) });
        }
        let mut loaded = Vec::new();
        let mut unavailable = Vec::new();
        while let Some(Ok((order, res, url))) = set.join_next().await {
            match res {
                Ok(a) => {
                    tracing::info!("loaded addon {} {}", a.manifest.name, a.manifest.version);
                    loaded.push((order, a));
                }
                Err(e) => {
                    tracing::warn!("failed to load addon {url}: {e:#}");
                    unavailable.push(url);
                }
            }
        }
        loaded.sort_by_key(|(o, _)| *o);
        let list = loaded.into_iter().map(|(_, a)| a).collect();
        Addons { http, path, list: RwLock::new(list), unavailable: std::sync::Mutex::new(unavailable) }
    }

    pub async fn list(&self) -> Vec<Addon> {
        self.list.read().await.clone()
    }

    pub async fn install(&self, url: &str) -> Result<Addon> {
        let addon = Addon::fetch(&self.http, url).await?;
        let mut list = self.list.write().await;
        list.retain(|a| a.manifest.id != addon.manifest.id);
        list.push(addon.clone());
        self.unavailable.lock().unwrap().retain(|u| *u != addon.url);
        self.save(&list)?;
        Ok(addon)
    }

    pub async fn uninstall(&self, id: &str) -> Result<()> {
        let mut list = self.list.write().await;
        list.retain(|a| a.manifest.id != id);
        self.save(&list)
    }

    fn save(&self, list: &[Addon]) -> Result<()> {
        let unavailable = self.unavailable.lock().unwrap();
        let urls: Vec<&str> = list.iter().map(|a| a.url.as_str()).chain(unavailable.iter().map(String::as_str)).collect();
        crate::write_atomic(&self.path, &serde_json::to_vec_pretty(&urls)?)
    }
}
