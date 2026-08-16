"""
mixid — web UI server ("Shazam for DJ mixes").

Launch (from the repo root):

    uvicorn server:app --host 127.0.0.1 --port 8900

(or simply `python3 server.py`).

Environment:
    MIXID_DB   path to the sqlite database (default: "mixid.db" next to this file;
               an absolute MIXID_DB overrides the location entirely)

NOTE on python-multipart: REQUIRED for the multipart upload branch of
POST /api/analyze (`await request.form()` raises without it). At dev time it
IS installed (python-multipart 0.0.32, verified via `python3 -c "import multipart"`).
If it ever goes missing, the JSON {"path": ...} branch of /api/analyze still works.

The core library (mixid/) is built by another agent; this file only imports
`mixid.db` and `mixid.analyzer` per the agreed API.
"""

from __future__ import annotations

import os
import re
import shutil
import sqlite3
import threading
import time
from pathlib import Path
from typing import Any

from fastapi import FastAPI, HTTPException, Request
from fastapi.concurrency import run_in_threadpool
from fastapi.responses import FileResponse
from fastapi.staticfiles import StaticFiles

try:
    from mixid import analyzer
    from mixid import db as mixdb
except ImportError as exc:  # core lib not built yet — fail loudly, not mysteriously
    raise SystemExit(
        "mixid core library not found: server.py requires the `mixid` package "
        "(built separately). Start the server from the repo root once mixid/ exists."
    ) from exc

ROOT = Path(__file__).resolve().parent
DB_PATH = ROOT / os.environ.get("MIXID_DB", "mixid.db")
UPLOAD_DIR = ROOT / "uploads"
STATIC_DIR = ROOT / "static"

app = FastAPI(title="mixid", docs_url=None, redoc_url=None)

if STATIC_DIR.is_dir():
    app.mount("/static", StaticFiles(directory=str(STATIC_DIR)), name="static")

# ---------------------------------------------------------------------------
# Database plumbing
#
# One global connection shared across request threads, with a lock serialising
# write ops (analyses) as per spec. If mixid.db.connect() happens NOT to create
# the connection with check_same_thread=False, we detect that at startup and
# transparently fall back to one connection per thread. Handlers never care:
# they just call db().
# ---------------------------------------------------------------------------

_DB_LOCK = threading.RLock()  # guards write ops on the shared connection
_ANALYZE_LOCK = threading.Lock()  # analyses are CPU-heavy: one at a time
_GLOBAL_CONN: sqlite3.Connection | None = None
_TLS = threading.local()


def _new_conn() -> sqlite3.Connection:
    conn = mixdb.connect(str(DB_PATH))
    try:  # be tolerant of concurrent readers while a long analysis commits
        conn.execute("PRAGMA busy_timeout = 5000")
    except sqlite3.Error:
        pass
    return conn


def _cross_thread_ok(conn: sqlite3.Connection) -> bool:
    """Probe whether `conn` may be used from a non-creating thread."""
    errs: list[BaseException] = []

    def probe() -> None:
        try:
            conn.execute("SELECT 1 FROM sqlite_master LIMIT 1").fetchone()
        except BaseException as e:  # noqa: BLE001 — deliberately broad
            errs.append(e)

    t = threading.Thread(target=probe, daemon=True)
    t.start()
    t.join()
    return not errs


def db() -> sqlite3.Connection:
    if _GLOBAL_CONN is not None:
        return _GLOBAL_CONN
    conn = getattr(_TLS, "conn", None)
    if conn is None:
        conn = _TLS.conn = _new_conn()
    return conn


def _init_db() -> None:
    global _GLOBAL_CONN
    conn = _new_conn()
    if _cross_thread_ok(conn):
        _GLOBAL_CONN = conn  # single shared conn; writes serialised by _ANALYZE_LOCK
    else:
        try:
            conn.close()
        except sqlite3.Error:
            pass
        # per-thread connections; sqlite file locking + busy_timeout keep it safe


_init_db()


def _d(x: Any) -> Any:
    """sqlite3.Row (or anything dict-like) -> plain dict."""
    return dict(x) if hasattr(x, "keys") else x


# ---------------------------------------------------------------------------
# API
# ---------------------------------------------------------------------------


@app.get("/api/health")
def health() -> dict:
    return {"ok": True}


@app.get("/api/mixes")
def api_mixes() -> dict:
    return {"mixes": mixdb.list_mixes(db())}


@app.get("/api/mixes/{mix_id}")
def api_mix(mix_id: int) -> dict:
    conn = db()
    mix = mixdb.get_mix(conn, mix_id)
    if mix is None:
        raise HTTPException(status_code=404, detail=f"mix {mix_id} not found")
    return {"mix": _d(mix), "tracklist": mixdb.get_mix_tracklist(conn, mix_id)}


@app.get("/api/tracks")
def api_tracks() -> dict:
    return {"tracks": mixdb.list_tracks(db())}


@app.get("/api/tracks/search")
def api_track_search(q: str = "") -> dict:
    q = q.strip()
    if not q:
        return {"results": []}
    conn = db()
    results = []
    for track in mixdb.search_tracks(conn, q):
        track = _d(track)
        track["mixes"] = mixdb.mixes_containing_track(conn, track["id"])
        results.append(track)
    return {"results": results}


_SAFE_NAME = re.compile(r"[^A-Za-z0-9._-]+")


def _safe_name(name: str | None) -> str:
    name = os.path.basename((name or "").replace("\\", "/"))
    name = _SAFE_NAME.sub("_", name).strip("._") or "upload"
    return name[:120]


def _save_upload(src_file: Any, dest: Path) -> None:
    with dest.open("wb") as out:
        shutil.copyfileobj(src_file, out)  # streams; no full-file RAM spike


@app.post("/api/analyze")
async def api_analyze(request: Request) -> dict:
    ctype = request.headers.get("content-type", "")
    title: str | None = None
    mix_path: str

    if ctype.startswith("multipart/form-data"):
        # Requires python-multipart (installed at dev time, v0.0.32 — see docstring).
        form = await request.form()
        upload = form.get("file")
        if upload is None or isinstance(upload, str):
            raise HTTPException(
                status_code=400, detail="multipart body must include a 'file' field"
            )
        raw_title = form.get("title")
        if isinstance(raw_title, str) and raw_title.strip():
            title = raw_title.strip()
        UPLOAD_DIR.mkdir(parents=True, exist_ok=True)
        dest = UPLOAD_DIR / f"{int(time.time() * 1000)}_{_safe_name(upload.filename)}"
        await run_in_threadpool(_save_upload, upload.file, dest)
        mix_path = str(dest)
    else:
        try:
            body = await request.json()
        except Exception:
            raise HTTPException(
                status_code=400,
                detail="expected multipart (field 'file') or JSON {'path': ..., 'title': ...}",
            )
        mix_path = str(body.get("path") or "").strip()
        raw_title = body.get("title")
        if isinstance(raw_title, str) and raw_title.strip():
            title = raw_title.strip()
        if not mix_path:
            raise HTTPException(status_code=400, detail="JSON body must include 'path'")
        if not Path(mix_path).is_file():
            raise HTTPException(
                status_code=404, detail=f"file not found on server: {mix_path}"
            )

    def work() -> dict:
        with _ANALYZE_LOCK:  # one CPU-heavy analysis at a time
            return analyzer.analyze_mix(db(), mix_path, title=title)

    try:
        return await run_in_threadpool(work)
    except HTTPException:
        raise
    except Exception as e:  # noqa: BLE001 — surface core-lib failures to the client
        raise HTTPException(status_code=500, detail=f"analysis failed: {e}")


# ---------------------------------------------------------------------------
# Static UI
# ---------------------------------------------------------------------------


@app.get("/")
def index() -> FileResponse:
    return FileResponse(STATIC_DIR / "index.html")


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(app, host="127.0.0.1", port=8900)
