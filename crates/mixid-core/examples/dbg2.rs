use mixid_core::fingerprint::fingerprint_file;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Usage: dbg2 <mix_file> <track_file> — print cluster stats for the pair.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mix = fingerprint_file(Path::new(&args[1])).unwrap().0;
    let track = fingerprint_file(Path::new(&args[2])).unwrap().0;
    println!("mix: {} hashes", mix.hashes.len());
    println!("track: {} hashes, {} distinct", track.hashes.len(), {
        let s: HashSet<u32> = track.hashes.iter().map(|&(h, _)| h).collect();
        s.len()
    });

    let mut tmap: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(h, t) in &track.hashes {
        let ts = tmap.entry(h).or_default();
        if ts.len() < 8 { ts.push(t); }
    }
    let mut mix_occ: HashMap<u32, u32> = HashMap::new();
    for &(h, _) in &mix.hashes { *mix_occ.entry(h).or_insert(0) += 1; }

    let mut votes: HashMap<i64, (u32, HashSet<u32>)> = HashMap::new();
    for &(h, tm) in &mix.hashes {
        if mix_occ[&h] > 64 { continue; }
        if let Some(ts) = tmap.get(&h) {
            for &tt in ts {
                let e = votes.entry(tm as i64 - tt as i64).or_insert((0, HashSet::new()));
                e.0 += 1;
                e.1.insert(h);
            }
        }
    }
    let mut v: Vec<(i64, u32, usize, Vec<i64>)> = votes
        .into_iter()
        .map(|(o, (n, s))| (o, n, s.len(), Vec::new()))
        .collect();
    // recompute matched times for top clusters
    v.sort_by(|a, b| b.2.cmp(&a.2));
    v.truncate(5);
    for (o, votes_n, distinct, _) in v.iter_mut() {
        let mut times: Vec<i64> = Vec::new();
        for &(h, tm) in &mix.hashes {
            if mix_occ[&h] > 64 { continue; }
            if let Some(ts) = tmap.get(&h) {
                if ts.iter().any(|&tt| tm as i64 - tt as i64 == *o) {
                    times.push(tm as i64);
                }
            }
        }
        times.sort_unstable();
        times.dedup();
        let span = times.last().unwrap_or(&0) - times.first().unwrap_or(&0);
        let density = if span > 0 { times.len() as f64 / span as f64 } else { 0.0 };
        println!("offset={:6} votes={:5} distinct={:5} matched_frames={:5} span={}f density={:.2}",
            o, votes_n, distinct, times.len(), span, density);
    }
}
