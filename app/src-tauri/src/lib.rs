use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use mixid_core::{split_artist_title, AnalysisResult, DetectionRow, MixRow, TrackInMix, TrackRow};

// ---------------------------------------------------------------------------
// State: DB path resolved once (app_data_dir()/mixid.db), connection kept
// lazily-opened behind a Mutex. All commands go through `with_db`.
// ---------------------------------------------------------------------------

struct DbState {
    path: PathBuf,
    db: Mutex<Option<mixid_core::Db>>,
}

impl DbState {
    fn with_db<T>(
        &self,
        f: impl FnOnce(&mut mixid_core::Db) -> anyhow::Result<T>,
    ) -> Result<T, String> {
        let mut guard = self.db.lock().unwrap();
        if guard.is_none() {
            let db = mixid_core::Db::open(&self.path)
                .map_err(|e| format!("failed to open database at {}: {e}", self.path.display()))?;
            *guard = Some(db);
        }
        f(guard.as_mut().unwrap()).map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Serializable payloads of our own (mixid-core rows are already Serialize)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct MixDetail {
    mix: MixRow,
    tracklist: Vec<DetectionRow>,
}

#[derive(Serialize)]
struct SearchResult {
    track: TrackRow,
    mixes: Vec<TrackInMix>,
}

#[derive(Serialize, Clone)]
struct IndexProgress {
    done: usize,
    total: usize,
    file: String,
}

#[derive(Serialize)]
struct IndexResult {
    indexed: usize,
    failed: Vec<String>,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn list_mixes(state: State<DbState>) -> Result<Vec<MixRow>, String> {
    state.with_db(|db| db.mixes())
}

#[tauri::command]
fn get_mix(id: i64, state: State<DbState>) -> Result<MixDetail, String> {
    state.with_db(|db| {
        let mix = db
            .mixes()?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| anyhow::anyhow!("mix {id} not found"))?;
        let tracklist = db.mix_tracklist(id)?;
        Ok(MixDetail { mix, tracklist })
    })
}

#[tauri::command]
fn search_tracks(q: String, state: State<DbState>) -> Result<Vec<SearchResult>, String> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    state.with_db(|db| {
        let tracks = db.search_tracks(q)?;
        let mut out = Vec::with_capacity(tracks.len());
        for t in tracks {
            let mixes = db.mixes_containing_track(t.id)?;
            out.push(SearchResult { track: t, mixes });
        }
        Ok(out)
    })
}

/// Analyze a mix file against the indexed library. Sync command is fine —
/// Tauri runs commands off the main thread; the frontend shows a spinner.
#[tauri::command]
fn analyze_path(
    path: String,
    title: Option<String>,
    state: State<DbState>,
) -> Result<AnalysisResult, String> {
    let p = Path::new(&path);
    if !p.is_file() {
        return Err(format!("not a file: {path}"));
    }
    state.with_db(|db| mixid_core::analyze_mix(db, p, title.as_deref(), 0.35, 8.0))
}

// ---------------------------------------------------------------------------
// Library indexing: walk a folder for audio files, fingerprint each, add to DB.
// ---------------------------------------------------------------------------

const AUDIO_EXTS: &[&str] = &[
    "mp3", "wav", "flac", "m4a", "ogg", "oga", "opus", "aiff", "aif", "aac", "wma", "mp4",
    "webm",
];

fn is_audio(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn collect_audio(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_audio(&path, out)?;
        } else if is_audio(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Split a filename stem into (artist, title) — shared logic in mixid-core
/// (handles the "NN - Artist - Title" DJ-pool convention).
fn split_stem(stem: &str) -> (String, String) {
    split_artist_title(stem)
}

#[tauri::command]
fn index_folder(
    app: AppHandle,
    path: String,
    state: State<DbState>,
) -> Result<IndexResult, String> {
    let root = Path::new(&path);
    if !root.is_dir() {
        return Err(format!("not a directory: {path}"));
    }

    let mut files = Vec::new();
    collect_audio(root, &mut files).map_err(|e| format!("walking {path}: {e}"))?;
    let total = files.len();
    let mut indexed = 0usize;
    let mut failed: Vec<String> = Vec::new();

    for (i, f) in files.into_iter().enumerate() {
        let _ = app.emit(
            "index-progress",
            IndexProgress {
                done: i,
                total,
                file: f.to_string_lossy().into_owned(),
            },
        );
        let stem = f
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();
        let (artist, title) = split_stem(&stem);
        match mixid_core::fingerprint_file(&f) {
            Ok((fp, duration)) => {
                let path_str = f.display().to_string();
                let added = state.with_db(|db| db.add_track(&title, &artist, &path_str, duration, &fp));
                match added {
                    Ok(_) => indexed += 1,
                    Err(e) => failed.push(format!("{}: {e}", f.display())),
                }
            }
            Err(e) => failed.push(format!("{}: {e}", f.display())),
        }
    }

    let _ = app.emit(
        "index-progress",
        IndexProgress {
            done: total,
            total,
            file: String::new(),
        },
    );
    Ok(IndexResult { indexed, failed })
}

// ---------------------------------------------------------------------------
// App entry
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db_path = dir.join("mixid.db");
            app.manage(DbState {
                path: db_path,
                db: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_mixes,
            get_mix,
            search_tracks,
            analyze_path,
            index_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running MixID");
}
