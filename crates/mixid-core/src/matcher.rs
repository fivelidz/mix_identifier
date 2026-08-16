//! Offset-voting matcher (dejavu-style): find where a track's fingerprint
//! occurs inside a mix's fingerprint.
//!
//! Scoring: hashes are weighted by library-level rarity (IDF = 1/df, computed
//! by the caller). A segment's score is the sum of weights of the DISTINCT
//! hashes aligned in a contiguous time run. True matches are dominated by
//! track-unique hashes (weight ~1); same-genre ghost matches align mostly
//! genre-generic hashes (low weight). Calibrated on real music:
//! true >= ~250 weighted, ghosts <= ~60 weighted.

use crate::fingerprint::{Fingerprint, FRAME_S};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct MatchSegment {
    pub track_id: i64,
    /// seconds into the mix where the matched span begins
    pub t_start: f64,
    /// seconds into the mix where the matched span ends
    pub t_end: f64,
    pub confidence: f64,
}

/// Per-hash occurrence cap in the track; kept occurrences are stride-sampled
/// across the track so late sections of loop-heavy tracks stay visible.
const MAX_HASH_OCCURRENCES: usize = 8;
/// Skip mix hashes occurring more than this many times (loops, silence).
const MAX_MIX_HASH_OCCURRENCES: u32 = 64;
/// A run must accumulate at least this much IDF-weighted distinct evidence.
const MIN_WEIGHTED: f64 = 60.0;
/// Matched frames must cover at least this fraction of the segment span.
const MIN_DENSITY: f64 = 0.25;
/// Weighted evidence of a solid full match -> confidence 1.0.
const WEIGHTED_FULL_MATCH: f64 = 300.0;
/// Report at most this many segments per track.
const MAX_SEGMENTS_PER_TRACK: usize = 3;
/// Score at most this many candidate offsets per track (by weighted evidence).
const MAX_OFFSETS_SCORED: usize = 8;
/// Matched times separated by more than this many frames belong to different
/// runs (a true segment is contiguous; ghosts stitch across song changes).
const RUN_GAP_TOL: i64 = 172; // ~8 s in frames

pub fn match_fingerprints(
    mix: &Fingerprint,
    track_id: i64,
    track: &Fingerprint,
    weights: &HashMap<u32, f64>,
    min_confidence: f64,
    min_duration_s: f64,
) -> Vec<MatchSegment> {
    if track.hashes.is_empty() || mix.hashes.is_empty() {
        return vec![];
    }

    // track hash -> stride-sampled times
    let mut tmap_all: HashMap<u32, Vec<u32>> = HashMap::with_capacity(track.hashes.len());
    for &(h, t) in &track.hashes {
        tmap_all.entry(h).or_default().push(t);
    }
    let tmap: HashMap<u32, Vec<u32>> = tmap_all
        .into_iter()
        .map(|(h, mut ts)| {
            if ts.len() > MAX_HASH_OCCURRENCES {
                let stride = ts.len() as f64 / MAX_HASH_OCCURRENCES as f64;
                ts = (0..MAX_HASH_OCCURRENCES)
                    .map(|i| ts[(i as f64 * stride) as usize])
                    .collect();
            }
            (h, ts)
        })
        .collect();
    let hash_weight = |h: u32| weights.get(&h).copied().unwrap_or(1.0);

    // Mix-side occurrence counts (skip ultra-common mix hashes).
    let mut mix_occ: HashMap<u32, u32> = HashMap::new();
    for &(h, _) in &mix.hashes {
        *mix_occ.entry(h).or_insert(0) += 1;
    }

    // Vote on offset = mix_frame - track_frame; collect the DISTINCT hashes
    // contributing to each offset.
    let mut votes: HashMap<i64, HashSet<u32>> = HashMap::new();
    for &(h, tm) in &mix.hashes {
        if mix_occ.get(&h).copied().unwrap_or(0) > MAX_MIX_HASH_OCCURRENCES {
            continue;
        }
        if let Some(ts) = tmap.get(&h) {
            for &tt in ts {
                votes.entry(tm as i64 - tt as i64).or_default().insert(h);
            }
        }
    }
    if votes.is_empty() {
        return vec![];
    }

    // Score each offset INDEPENDENTLY. A true match is dense at its offset
    // (for repetitive tracks: dense at EACH of its repeat offsets); a ghost
    // is sparse at every offset. Pooling adjacent offsets (cluster merging)
    // lets ghosts stitch deceptively dense runs — so we don't pool.
    let mut offs: Vec<(i64, f64, HashSet<u32>)> = votes
        .into_iter()
        .map(|(o, s)| {
            let w: f64 = s.iter().map(|&h| hash_weight(h)).sum();
            (o, w, s)
        })
        .collect();
    offs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut out: Vec<MatchSegment> = Vec::new();
    'offsets: for (core, w, _) in offs.into_iter().take(MAX_OFFSETS_SCORED) {
        if w < MIN_WEIGHTED {
            break; // sorted — nothing below can pass
        }
        // Matched (mix_frame, hash) pairs at this offset.
        let mut pairs: Vec<(i64, u32)> = Vec::new();
        for &(h, tm) in &mix.hashes {
            if mix_occ.get(&h).copied().unwrap_or(0) > MAX_MIX_HASH_OCCURRENCES {
                continue;
            }
            if let Some(ts) = tmap.get(&h) {
                if ts.iter().any(|&tt| tm as i64 - tt as i64 == core) {
                    pairs.push((tm as i64, h));
                }
            }
        }
        if pairs.len() < 2 {
            continue;
        }
        pairs.sort_by_key(|(t, _)| *t);

        // Split into contiguous runs at RUN_GAP_TOL gaps; score each run.
        let mut run_start = 0usize;
        for idx in 1..=pairs.len() {
            let split = idx == pairs.len() || pairs[idx].0 - pairs[idx - 1].0 > RUN_GAP_TOL;
            if !split {
                continue;
            }
            let run = &pairs[run_start..idx];
            run_start = idx;
            if let Some(seg) = score_run(run, track_id, weights, min_confidence, min_duration_s) {
                out.push(seg);
                if out.len() >= MAX_SEGMENTS_PER_TRACK {
                    break 'offsets;
                }
            }
        }
    }
    out.sort_by(|a, b| {
        a.t_start
            .partial_cmp(&b.t_start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Score one contiguous run of matched (mix_frame, hash) pairs.
fn score_run(
    run: &[(i64, u32)],
    track_id: i64,
    weights: &HashMap<u32, f64>,
    min_confidence: f64,
    min_duration_s: f64,
) -> Option<MatchSegment> {
    let hash_weight = |h: u32| weights.get(&h).copied().unwrap_or(1.0);

    // unique matched frames
    let mut times: Vec<i64> = run.iter().map(|&(t, _)| t).collect();
    times.dedup();

    // Adaptive edge trim: bucket matched frames into ~2s windows; trim edges
    // while a bucket holds < 25% of the run's PEAK bucket count. Adapts to
    // each match's own quality — dense true matches keep their fade-out
    // tails, while sparse bleed into a neighbouring song gets cut.
    let bucket_frames = (2.0 / FRAME_S) as i64;
    let t0 = times[0];
    let nb = ((times[times.len() - 1] - t0) / bucket_frames + 1) as usize;
    let mut counts = vec![0usize; nb];
    for &t in &times {
        counts[((t - t0) / bucket_frames) as usize] += 1;
    }
    let peak = *counts.iter().max().unwrap_or(&1) as f64;
    let thresh = 0.25 * peak;
    let mut s_b = 0usize;
    while s_b + 1 < nb && (counts[s_b] as f64) < thresh {
        s_b += 1;
    }
    let mut e_b = nb - 1;
    while e_b > s_b + 1 && (counts[e_b] as f64) < thresh {
        e_b -= 1;
    }
    let lo = t0 + s_b as i64 * bucket_frames;
    let hi = t0 + (e_b as i64 + 1) * bucket_frames;
    let s = times.partition_point(|&t| t < lo);
    let e = times.partition_point(|&t| t < hi).saturating_sub(1);
    if e <= s + 1 {
        return None;
    }

    let t_start = times[s] as f64 * FRAME_S;
    let t_end = times[e] as f64 * FRAME_S;
    let duration = t_end - t_start;
    if duration < min_duration_s {
        return None;
    }

    let span_frames = (times[e] - times[s]).max(1) as f64;
    let matched_in_span = times[s..=e].len() as f64;
    let density = matched_in_span / span_frames;
    if density < MIN_DENSITY {
        return None;
    }

    // IDF-weighted DISTINCT evidence within the trimmed span (each hash
    // counted exactly once — occurrence multiplicity must not inflate score).
    let distinct_in_span: HashSet<u32> = run
        .iter()
        .filter(|&&(t, _)| t >= times[s] && t <= times[e])
        .map(|&(_, h)| h)
        .collect();
    let weighted: f64 = distinct_in_span.iter().map(|&h| hash_weight(h)).sum();
    if weighted < MIN_WEIGHTED {
        return None;
    }
    let confidence = (weighted / WEIGHTED_FULL_MATCH).min(1.0);
    if confidence < min_confidence {
        return None;
    }
    Some(MatchSegment {
        track_id,
        t_start,
        t_end,
        confidence,
    })
}
