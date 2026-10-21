//! HTTP API for the web UI and the `/stream/{torrent}/{file}/{name}` endpoint used by players.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::sync::Arc;

use anyhow::Context;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::middleware::{self, Next};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::library::LibUpdate;
use crate::{Core, PlayReq, engine};

type S = State<Arc<Core>>;

struct ApiError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::warn!("{:#}", self.0);
        let body = Json(json!({ "error": format!("{:#}", self.0) }));
        (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

pub fn router(core: Arc<Core>) -> Router {
    let port = core.port;
    Router::new()
        .route("/", get(|| async { Html(include_str!("../static/index.html")) }))
        .route("/api/addons", get(list_addons).post(install_addon))
        .route("/api/addons/{id}", axum::routing::delete(uninstall_addon))
        .route("/api/catalogs", get(catalogs))
        .route("/api/catalog/{addon}/{kind}/{id}", get(catalog))
        .route("/api/search", get(search))
        .route("/api/meta/{kind}/{id}", get(meta))
        .route("/api/streams/{kind}/{id}", get(streams))
        .route("/api/library", get(library).post(update_library))
        .route("/api/play", post(play))
        .route("/api/stop", post(stop))
        .route("/api/status/{tid}", get(status))
        .route("/stream/{tid}/{fid}/{name}", get(stream_file))
        .layer(middleware::from_fn(move |req: Request, next: Next| local_only(port, req, next)))
        .with_state(core)
}

/// Rejects requests whose `Host` isn't this server, so a web page can't reach the API through
/// DNS rebinding (e.g. `evil.example` resolving to 127.0.0.1).
async fn local_only(port: u16, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
    let allowed = ["127.0.0.1", "localhost", "[::1]"].iter().any(|h| host == *h || host == format!("{h}:{port}"));
    if allowed { next.run(req).await } else { (StatusCode::FORBIDDEN, "forbidden host").into_response() }
}

/// `bytes=start-end`, `bytes=start-` or `bytes=-suffix` as a half-open `[start, end)`.
fn parse_range(v: &str, len: u64) -> Option<(u64, u64)> {
    let (start, end) = v.strip_prefix("bytes=")?.split(',').next()?.trim().split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?.min(len);
        return Some((len - suffix, len));
    }
    let start = start.parse::<u64>().ok()?;
    let end = match end {
        "" => len,
        e => e.parse::<u64>().ok()?.saturating_add(1).min(len),
    };
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-", 100), Some((0, 100)));
        assert_eq!(parse_range("bytes=10-19", 100), Some((10, 20)));
        assert_eq!(parse_range("bytes=90-500", 100), Some((90, 100)));
        assert_eq!(parse_range("bytes=-30", 100), Some((70, 100)));
        assert_eq!(parse_range("bytes=-300", 100), Some((0, 100)));
        assert_eq!(parse_range("bytes=5-9, 20-30", 100), Some((5, 10)));
        assert_eq!(parse_range("items=0-1", 100), None);
    }
}

async fn list_addons(State(s): S) -> Json<Value> {
    Json(json!(s.addons.list().await))
}

#[derive(Deserialize)]
struct InstallReq {
    url: String,
}

async fn install_addon(State(s): S, Json(req): Json<InstallReq>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.addons.install(&req.url).await?)))
}

async fn uninstall_addon(State(s): S, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    s.addons.uninstall(&id).await?;
    Ok(Json(json!({})))
}

async fn catalogs(State(s): S) -> Json<Value> {
    Json(json!(s.catalogs().await))
}

async fn catalog(
    State(s): S,
    Path((addon, kind, id)): Path<(String, String, String)>,
    Query(extra): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let extra: Vec<_> = extra.into_iter().filter(|(_, v)| !v.is_empty()).collect();
    Ok(Json(s.catalog(&addon, &kind, &id, &extra).await?))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

async fn search(State(s): S, Query(SearchQuery { q }): Query<SearchQuery>) -> Json<Value> {
    Json(json!(s.search(&q).await))
}

async fn meta(State(s): S, Path((kind, id)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    Ok(Json(s.meta(&kind, &id).await?))
}

async fn streams(State(s): S, Path((kind, id)): Path<(String, String)>) -> Json<Value> {
    Json(json!({ "streams": s.streams(&kind, &id).await }))
}

async fn library(State(s): S) -> Json<Value> {
    Json(json!(s.library.items()))
}

async fn update_library(State(s): S, Json(u): Json<LibUpdate>) -> ApiResult<Json<Value>> {
    s.library.update(u)?;
    Ok(Json(json!({})))
}

async fn play(State(s): S, Json(req): Json<PlayReq>) -> ApiResult<Json<Value>> {
    Ok(Json(s.play(req).await?))
}

async fn stop(State(s): S) -> Json<Value> {
    s.engine.stop().await;
    Json(json!({}))
}

async fn status(State(s): S, Path(tid): Path<usize>) -> ApiResult<Json<Value>> {
    Ok(Json(s.status(tid)?))
}

/// HTTP range streaming of a torrent file; pieces are prioritized around the read position.
async fn stream_file(
    State(s): S,
    Path((tid, fid, name)): Path<(usize, usize, String)>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let t = s.engine.get(tid).context("torrent not found")?;
    let mut stream = t.stream(fid).await?;
    let len = stream.len();

    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_range(v, len));

    let mut out = HeaderMap::new();
    out.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    out.insert(header::CONTENT_TYPE, HeaderValue::from_static(engine::mime_type(&name)));

    let (status, body) = match range {
        Some((start, end)) => {
            if start >= len || end <= start {
                let mut h = HeaderMap::new();
                h.insert(header::CONTENT_RANGE, format!("bytes */{len}").parse()?);
                return Ok((StatusCode::RANGE_NOT_SATISFIABLE, h).into_response());
            }
            stream.seek(SeekFrom::Start(start)).await?;
            out.insert(header::CONTENT_LENGTH, (end - start).into());
            out.insert(header::CONTENT_RANGE, format!("bytes {start}-{}/{len}", end - 1).parse()?);
            let body = Body::from_stream(ReaderStream::with_capacity(stream.take(end - start), 65536));
            (StatusCode::PARTIAL_CONTENT, body)
        }
        None => {
            out.insert(header::CONTENT_LENGTH, len.into());
            (StatusCode::OK, Body::from_stream(ReaderStream::with_capacity(stream, 65536)))
        }
    };
    Ok((status, out, body).into_response())
}
