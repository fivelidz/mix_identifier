//! mixid-server — web server for "mixid", the Shazam-for-DJ-mixes app.
//!
//! # Run
//!
//! From the workspace root:
//!
//! ```text
//! cargo run --release -p mixid-server
//! ```
//!
//! # Configuration (environment variables)
//!
//! * `PORT` — listen port (default `8900`)
//! * `MIXID_DB` — SQLite database path (default `mixid.db`, created if missing)
//!
//! The web UI is served from `<repo>/static/index.html` (resolved relative to
//! this crate's manifest dir at compile time, then the current dir; if neither
//! is readable, a copy embedded at compile time is served). Uploaded files are
//! stored under `./uploads/`.
//!
//! # Endpoints
//!
//! | Method | Path                | Body / Query                    | Response |
//! |--------|---------------------|---------------------------------|----------|
//! | GET    | `/`                 | —                               | the web UI (HTML) |
//! | GET    | `/api/health`       | —                               | `{"ok":true}` |
//! | GET    | `/api/mixes`        | —                               | `{"mixes":[MixRow]}` |
//! | GET    | `/api/mixes/{id}`   | —                               | `{"mix":MixRow,"tracklist":[DetectionRow]}` or 404 |
//! | GET    | `/api/tracks`       | —                               | `{"tracks":[TrackRow]}` |
//! | GET    | `/api/tracks/search`| `?q=`                           | `{"results":[{...TrackRow,"mixes":[TrackInMix]}]}` |
//! | POST   | `/api/analyze`      | multipart `file`[,`title`] or JSON `{"path","title"}` | `AnalysisResult` |
//!
//! Analyses are serialized through a single DB lock and run on the blocking
//! thread pool (`tokio::task::spawn_blocking`), so long FFT work never stalls
//! the async runtime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{FromRequest, Multipart, Path as AxPath, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tower_http::services::ServeDir;

use mixid_core::{analyze_mix, Db};

/// Shared application state: the SQLite handle behind an async mutex.
/// Analyses take the lock for the whole (blocking) call, so they serialize.
struct AppState {
    db: Mutex<Db>,
}

type SharedState = Arc<AppState>;

/// Where uploaded files land (relative to the current working directory).
const UPLOAD_DIR: &str = "uploads";

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run())
}

async fn run() -> anyhow::Result<()> {
    let db_path = std::env::var("MIXID_DB").unwrap_or_else(|_| "mixid.db".to_string());
    let db = Db::open(Path::new(&db_path))?;
    std::fs::create_dir_all(UPLOAD_DIR)?;
    let state = Arc::new(AppState { db: Mutex::new(db) });

    // static/ lives at the repo root, two levels above this crate.
    let static_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../static");

    let app = Router::new()
        .route("/", get(root))
        .route("/api/health", get(health))
        .route("/api/mixes", get(list_mixes))
        .route("/api/mixes/{id}", get(get_mix))
        .route("/api/tracks", get(list_tracks))
        .route("/api/tracks/search", get(search_tracks))
        .route("/api/analyze", post(analyze))
        .fallback_service(ServeDir::new(static_dir))
        .with_state(state);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8900);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    eprintln!("mixid-server listening on http://127.0.0.1:{port} (db: {db_path})");
    axum::serve(listener, app).await?;
    Ok(())
}

/* ------------------------------------------------------------------ */
/* helpers                                                             */
/* ------------------------------------------------------------------ */

fn error_response(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn internal_error(e: impl std::fmt::Display) -> Response {
    error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn not_found() -> Response {
    error_response(StatusCode::NOT_FOUND, "not found")
}

/* ------------------------------------------------------------------ */
/* static UI                                                           */
/* ------------------------------------------------------------------ */

async fn root() -> Response {
    // Prefer the on-disk file (picks up edits without a rebuild); fall back
    // to the copy embedded at compile time so the binary is self-sufficient.
    let compiled = concat!(env!("CARGO_MANIFEST_DIR"), "/../../static/index.html");
    for candidate in [compiled, "static/index.html"] {
        if let Ok(html) = tokio::fs::read_to_string(candidate).await {
            return Html(html).into_response();
        }
    }
    Html(include_str!("../../../static/index.html")).into_response()
}

/* ------------------------------------------------------------------ */
/* read-only JSON endpoints                                            */
/* ------------------------------------------------------------------ */

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

async fn list_mixes(State(state): State<SharedState>) -> Response {
    let db = state.db.lock().await;
    match db.mixes() {
        Ok(mixes) => Json(json!({ "mixes": mixes })).into_response(),
        Err(e) => internal_error(e),
    }
}

async fn get_mix(State(state): State<SharedState>, AxPath(id): AxPath<i64>) -> Response {
    let db = state.db.lock().await;
    let mix = match db.mixes() {
        Ok(mixes) => mixes.into_iter().find(|m| m.id == id),
        Err(e) => return internal_error(e),
    };
    let Some(mix) = mix else {
        return not_found();
    };
    match db.mix_tracklist(id) {
        Ok(tracklist) => Json(json!({ "mix": mix, "tracklist": tracklist })).into_response(),
        Err(e) => internal_error(e),
    }
}

async fn list_tracks(State(state): State<SharedState>) -> Response {
    let db = state.db.lock().await;
    match db.tracks() {
        Ok(tracks) => Json(json!({ "tracks": tracks })).into_response(),
        Err(e) => internal_error(e),
    }
}

async fn search_tracks(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let q = params.get("q").map(|s| s.trim()).unwrap_or("");
    if q.is_empty() {
        return Json(json!({ "results": [] })).into_response();
    }
    let db = state.db.lock().await;
    let tracks = match db.search_tracks(q) {
        Ok(t) => t,
        Err(e) => return internal_error(e),
    };
    let mut results = Vec::with_capacity(tracks.len());
    for track in tracks {
        let mixes = match db.mixes_containing_track(track.id) {
            Ok(m) => m,
            Err(e) => return internal_error(e),
        };
        // {**TrackRow, "mixes": [TrackInMix]}
        let mut value = serde_json::to_value(&track).unwrap_or_else(|_| json!({}));
        if let Some(obj) = value.as_object_mut() {
            obj.insert("mixes".to_string(), json!(mixes));
        }
        results.push(value);
    }
    Json(json!({ "results": results })).into_response()
}

/* ------------------------------------------------------------------ */
/* POST /api/analyze                                                   */
/* ------------------------------------------------------------------ */

#[derive(Deserialize, Default)]
struct AnalyzeBody {
    path: Option<String>,
    title: Option<String>,
}

async fn analyze(State(state): State<SharedState>, req: Request) -> Response {
    let content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let (mix_path, title) = if content_type.starts_with("multipart/form-data") {
        let mut multipart = match Multipart::from_request(req, &()).await {
            Ok(m) => m,
            Err(e) => return error_response(StatusCode::BAD_REQUEST, e.to_string()),
        };
        match save_upload(&mut multipart).await {
            Ok(v) => v,
            Err(msg) => return error_response(StatusCode::BAD_REQUEST, msg),
        }
    } else {
        let body: AnalyzeBody = match axum::Json::from_request(req, &()).await {
            Ok(axum::Json(b)) => b,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!("expected multipart (field 'file') or JSON {{'path': ...}}: {e}"),
                );
            }
        };
        let Some(path) = body
            .path
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
        else {
            return error_response(StatusCode::BAD_REQUEST, "JSON body must include 'path'");
        };
        if !Path::new(&path).is_file() {
            return error_response(
                StatusCode::NOT_FOUND,
                format!("file not found on server: {path}"),
            );
        }
        (PathBuf::from(path), clean_title(body.title))
    };

    // Heavy CPU work: run on the blocking pool. The DB lock is held for the
    // whole analysis, which serializes concurrent analyses (by design).
    let state = Arc::clone(&state);
    let result = tokio::task::spawn_blocking(move || {
        let mut db = state.db.blocking_lock();
        analyze_mix(&mut db, &mix_path, title.as_deref(), 0.35, 8.0)
    })
    .await;

    match result {
        Ok(Ok(analysis)) => Json(analysis).into_response(),
        Ok(Err(e)) => internal_error(e), // analyze_mix failed
        Err(e) => internal_error(e),     // task join failure / panic
    }
}

/// Consume a multipart body: save the `file` field under `uploads/`, read the
/// optional `title` field. Returns the saved path and the cleaned title.
async fn save_upload(multipart: &mut Multipart) -> Result<(PathBuf, Option<String>), String> {
    let mut saved: Option<PathBuf> = None;
    let mut title: Option<String> = None;

    while let Some(mut field) = multipart.next_field().await.map_err(|e| e.to_string())? {
        match field.name().unwrap_or("") {
            "file" => {
                let filename = field.file_name().unwrap_or("upload").to_string();
                tokio::fs::create_dir_all(UPLOAD_DIR)
                    .await
                    .map_err(|e| e.to_string())?;
                let millis = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                let dest = Path::new(UPLOAD_DIR).join(format!("{millis}_{}", sanitize(&filename)));

                let mut file = tokio::fs::File::create(&dest)
                    .await
                    .map_err(|e| e.to_string())?;
                // Stream chunk-by-chunk: mixes can be hundreds of MB.
                while let Some(chunk) = field.chunk().await.map_err(|e| e.to_string())? {
                    file.write_all(&chunk).await.map_err(|e| e.to_string())?;
                }
                file.flush().await.map_err(|e| e.to_string())?;
                saved = Some(dest);
            }
            "title" => {
                let text = field.text().await.map_err(|e| e.to_string())?;
                title = clean_title(Some(text));
            }
            _ => {
                // Drain unknown fields so the stream stays parseable.
                let _ = field.bytes().await;
            }
        }
    }

    saved
        .map(|p| (p, title))
        .ok_or_else(|| "multipart body must include a 'file' field".to_string())
}

/// Trim a title; empty string becomes `None`.
fn clean_title(raw: Option<String>) -> Option<String> {
    let t = raw?.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// Make a client-supplied filename safe to store: strip any directory
/// components, keep only `[A-Za-z0-9._-]`, no leading dots, cap at 120 chars.
fn sanitize(name: &str) -> String {
    let base = name.replace('\\', "/");
    let base = base.rsplit('/').next().unwrap_or("");
    let mut out: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    out = out.trim_start_matches('.').to_string();
    if out.is_empty() {
        out.push_str("upload");
    }
    out.truncate(120);
    out
}
