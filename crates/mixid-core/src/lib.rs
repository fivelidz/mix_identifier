//! MixID core — audio fingerprinting and mix analysis.
//!
//! Pipeline: decode (symphonia) -> mono -> resample 11.025kHz -> STFT (rustfft)
//! -> constellation peaks -> anchor/target hash pairs (Shazam-style).
//! Matching: offset voting (dejavu-style) between mix and track fingerprints.

pub mod db;
pub mod fingerprint;
pub mod matcher;

use anyhow::Result;
use serde::Serialize;
use std::path::Path;

pub use db::Db;
pub use fingerprint::{fingerprint_file, Fingerprint};

#[derive(Clone, Debug, Serialize)]
pub struct TrackRow {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub duration: f64,
    pub mix_count: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct MixRow {
    pub id: i64,
    pub title: String,
    pub duration: f64,
    pub added_at: String,
    pub track_count: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DetectionRow {
    pub track_id: i64,
    pub title: String,
    pub artist: String,
    pub t_start: f64,
    pub t_end: f64,
    pub confidence: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrackInMix {
    pub mix_id: i64,
    pub mix_title: String,
    pub t_start: f64,
    pub t_end: f64,
    pub confidence: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnalysisResult {
    pub mix_id: i64,
    pub title: String,
    pub duration: f64,
    pub detections: Vec<DetectionRow>,
}

/// Fingerprint `mix_path`, match it against every track in the DB, store the
/// mix + its detections, and return the joined tracklist.
pub fn analyze_mix(
    db: &mut Db,
    mix_path: &Path,
    title: Option<&str>,
    min_confidence: f64,
    min_duration_s: f64,
) -> Result<AnalysisResult> {
    let (mix_fp, duration) = fingerprint_file(mix_path)?;

    let track_fps = db.all_track_fingerprints()?;

    // Library-level rarity (IDF): weight each hash by 1/document-frequency.
    // Hashes shared across many tracks (kick loops, genre-generic patterns)
    // contribute little; a track's unique content contributes fully. This
    // separates true matches from same-genre ghost matches and scales as the
    // library grows.
    let mut df: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for (_, tfp) in &track_fps {
        let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for &(h, _) in &tfp.hashes {
            seen.insert(h);
        }
        for h in seen {
            *df.entry(h).or_insert(0) += 1;
        }
    }
    let weights: std::collections::HashMap<u32, f64> =
        df.into_iter().map(|(h, n)| (h, 1.0 / n as f64)).collect();

    let mut segments: Vec<matcher::MatchSegment> = Vec::new();
    for (track_id, tfp) in &track_fps {
        segments.extend(matcher::match_fingerprints(
            &mix_fp,
            *track_id,
            tfp,
            &weights,
            min_confidence,
            min_duration_s,
        ));
    }
    segments.sort_by(|a, b| {
        a.t_start
            .partial_cmp(&b.t_start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Deduplicate overlapping segments of the same track (keep higher confidence).
    let mut kept: Vec<matcher::MatchSegment> = Vec::new();
    for seg in segments {
        let overlap = kept
            .iter()
            .any(|k| k.track_id == seg.track_id && seg.t_start < k.t_end && k.t_start < seg.t_end);
        if !overlap {
            kept.push(seg);
        }
    }

    let title_s = title.map(|s| s.to_string()).unwrap_or_else(|| {
        mix_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled Mix".into())
    });
    let path_s = mix_path.display().to_string();

    let mix_id = db.add_mix(&title_s, &path_s, duration)?;
    db.clear_detections(mix_id)?;
    for seg in &kept {
        db.add_detection(mix_id, seg.track_id, seg.t_start, seg.t_end, seg.confidence)?;
    }

    let detections = db.mix_tracklist(mix_id)?;
    Ok(AnalysisResult {
        mix_id,
        title: title_s,
        duration,
        detections,
    })
}
