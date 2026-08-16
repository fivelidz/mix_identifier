//! SQLite persistence for tracks, mixes, hashes and detections.

use crate::fingerprint::Fingerprint;
use crate::{DetectionRow, MixRow, TrackInMix, TrackRow};
use anyhow::Result;
use rusqlite::{params, Connection};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS tracks(
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    artist TEXT NOT NULL DEFAULT '',
    path TEXT NOT NULL UNIQUE,
    duration REAL NOT NULL
);
CREATE TABLE IF NOT EXISTS mixes(
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    path TEXT NOT NULL UNIQUE,
    duration REAL NOT NULL,
    added_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS hashes(
    hash INTEGER NOT NULL,
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    t INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_hashes_track ON hashes(track_id);
CREATE TABLE IF NOT EXISTS detections(
    id INTEGER PRIMARY KEY,
    mix_id INTEGER NOT NULL REFERENCES mixes(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    t_start REAL NOT NULL,
    t_end REAL NOT NULL,
    confidence REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_det_mix ON detections(mix_id);
CREATE INDEX IF NOT EXISTS idx_det_track ON detections(track_id);
";

pub struct Db(Connection);

impl Db {
    pub fn open<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db(conn))
    }

    /// Upsert a track by path (keeps its id), replacing its hashes.
    pub fn add_track(
        &mut self,
        title: &str,
        artist: &str,
        path: &str,
        duration_s: f64,
        fp: &Fingerprint,
    ) -> Result<i64> {
        let conn = &mut self.0;
        let tx = conn.transaction()?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM tracks WHERE path = ?1",
                params![path],
                |r| r.get(0),
            )
            .ok();
        let id = match existing {
            Some(id) => {
                tx.execute(
                    "UPDATE tracks SET title=?1, artist=?2, duration=?3 WHERE id=?4",
                    params![title, artist, duration_s, id],
                )?;
                tx.execute("DELETE FROM hashes WHERE track_id = ?1", params![id])?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO tracks(title, artist, path, duration) VALUES (?1,?2,?3,?4)",
                    params![title, artist, path, duration_s],
                )?;
                tx.last_insert_rowid()
            }
        };
        {
            let mut stmt = tx.prepare("INSERT INTO hashes(hash, track_id, t) VALUES (?1,?2,?3)")?;
            for &(h, t) in &fp.hashes {
                stmt.execute(params![h as i64, id, t as i64])?;
            }
        }
        tx.commit()?;
        Ok(id)
    }

    pub fn tracks(&self) -> Result<Vec<TrackRow>> {
        let mut stmt = self.0.prepare(
            "SELECT t.id, t.title, t.artist, t.duration, COUNT(d.id)
             FROM tracks t LEFT JOIN detections d ON d.track_id = t.id
             GROUP BY t.id ORDER BY t.title",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TrackRow {
                id: r.get(0)?,
                title: r.get(1)?,
                artist: r.get(2)?,
                duration: r.get(3)?,
                mix_count: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// All (track_id, fingerprint) pairs — used by analyze_mix.
    pub fn all_track_fingerprints(&self) -> Result<Vec<(i64, Fingerprint)>> {
        let ids: Vec<i64> = {
            let mut stmt = self.0.prepare("SELECT id FROM tracks")?;
            let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let mut stmt = self
                .0
                .prepare("SELECT hash, t FROM hashes WHERE track_id = ?1")?;
            let rows = stmt.query_map(params![id], |r| {
                Ok((r.get::<_, i64>(0)? as u32, r.get::<_, i64>(1)? as u32))
            })?;
            let hashes = rows.collect::<std::result::Result<Vec<_>, _>>()?;
            out.push((id, Fingerprint { hashes }));
        }
        Ok(out)
    }

    /// Upsert a mix by path (keeps its id).
    pub fn add_mix(&mut self, title: &str, path: &str, duration_s: f64) -> Result<i64> {
        let existing: Option<i64> = self
            .0
            .query_row("SELECT id FROM mixes WHERE path = ?1", params![path], |r| {
                r.get(0)
            })
            .ok();
        Ok(match existing {
            Some(id) => {
                self.0.execute(
                    "UPDATE mixes SET title=?1, duration=?2 WHERE id=?3",
                    params![title, duration_s, id],
                )?;
                id
            }
            None => {
                self.0.execute(
                    "INSERT INTO mixes(title, path, duration) VALUES (?1,?2,?3)",
                    params![title, path, duration_s],
                )?;
                self.0.last_insert_rowid()
            }
        })
    }

    pub fn clear_detections(&mut self, mix_id: i64) -> Result<()> {
        self.0
            .execute("DELETE FROM detections WHERE mix_id = ?1", params![mix_id])?;
        Ok(())
    }

    pub fn add_detection(
        &mut self,
        mix_id: i64,
        track_id: i64,
        t_start: f64,
        t_end: f64,
        confidence: f64,
    ) -> Result<()> {
        self.0.execute(
            "INSERT INTO detections(mix_id, track_id, t_start, t_end, confidence) VALUES (?1,?2,?3,?4,?5)",
            params![mix_id, track_id, t_start, t_end, confidence],
        )?;
        Ok(())
    }

    pub fn mixes(&self) -> Result<Vec<MixRow>> {
        let mut stmt = self.0.prepare(
            "SELECT m.id, m.title, m.duration, m.added_at, COUNT(d.id)
             FROM mixes m LEFT JOIN detections d ON d.mix_id = m.id
             GROUP BY m.id ORDER BY m.id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(MixRow {
                id: r.get(0)?,
                title: r.get(1)?,
                duration: r.get(2)?,
                added_at: r.get(3)?,
                track_count: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn mix_tracklist(&self, mix_id: i64) -> Result<Vec<DetectionRow>> {
        let mut stmt = self.0.prepare(
            "SELECT d.track_id, t.title, t.artist, d.t_start, d.t_end, d.confidence
             FROM detections d JOIN tracks t ON t.id = d.track_id
             WHERE d.mix_id = ?1 ORDER BY d.t_start",
        )?;
        let rows = stmt.query_map(params![mix_id], |r| {
            Ok(DetectionRow {
                track_id: r.get(0)?,
                title: r.get(1)?,
                artist: r.get(2)?,
                t_start: r.get(3)?,
                t_end: r.get(4)?,
                confidence: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn search_tracks(&self, q: &str) -> Result<Vec<TrackRow>> {
        let like = format!("%{}%", q);
        let mut stmt = self.0.prepare(
            "SELECT t.id, t.title, t.artist, t.duration, COUNT(d.id)
             FROM tracks t LEFT JOIN detections d ON d.track_id = t.id
             WHERE t.title LIKE ?1 OR t.artist LIKE ?1
             GROUP BY t.id ORDER BY t.title",
        )?;
        let rows = stmt.query_map(params![like], |r| {
            Ok(TrackRow {
                id: r.get(0)?,
                title: r.get(1)?,
                artist: r.get(2)?,
                duration: r.get(3)?,
                mix_count: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn mixes_containing_track(&self, track_id: i64) -> Result<Vec<TrackInMix>> {
        let mut stmt = self.0.prepare(
            "SELECT m.id, m.title, d.t_start, d.t_end, d.confidence
             FROM detections d JOIN mixes m ON m.id = d.mix_id
             WHERE d.track_id = ?1 ORDER BY d.t_start",
        )?;
        let rows = stmt.query_map(params![track_id], |r| {
            Ok(TrackInMix {
                mix_id: r.get(0)?,
                mix_title: r.get(1)?,
                t_start: r.get(2)?,
                t_end: r.get(3)?,
                confidence: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}
