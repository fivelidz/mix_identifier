use mixid_core::fingerprint::fingerprint_file;
use std::collections::HashMap;
use std::path::Path;

fn main() {
    let a = fingerprint_file(Path::new("test_data/songs/songA.wav")).unwrap().0;
    let c = fingerprint_file(Path::new("test_data/songs/songB.wav")).unwrap().0;
    let e = fingerprint_file(Path::new("test_data/songs/songE.wav")).unwrap().0;

    let mut ma: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(h, t) in &a.hashes { ma.entry(h).or_default().push(t); }
    let mut mc: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(h, t) in &c.hashes { mc.entry(h).or_default().push(t); }
    let mut me: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(h, t) in &e.hashes { me.entry(h).or_default().push(t); }

    println!("A: {} hashes, {} distinct", a.hashes.len(), ma.len());
    println!("C: {} hashes, {} distinct", c.hashes.len(), mc.len());
    println!("E: {} hashes, {} distinct", e.hashes.len(), me.len());

    let shared_ac: usize = ma.keys().filter(|k| mc.contains_key(k)).count();
    let shared_ae: usize = ma.keys().filter(|k| me.contains_key(k)).count();
    println!("shared A∩C distinct hashes: {} ({:.1}% of A)", shared_ac, 100.0 * shared_ac as f64 / ma.len() as f64);
    println!("shared A∩E distinct hashes: {} ({:.1}% of A)", shared_ae, 100.0 * shared_ae as f64 / ma.len() as f64);

    // decode top shared hashes
    let mut shared: Vec<(u32, usize)> = ma.iter()
        .filter(|(k, _)| mc.contains_key(k))
        .map(|(k, v)| (*k, v.len().min(mc[k].len())))
        .collect();
    shared.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!("\ntop shared A∩C hashes (f1, f2, dt, min_occ):");
    for (h, n) in shared.iter().take(15) {
        let f1 = h >> 15;
        let f2 = (h >> 6) & 0x1FF;
        let dt = h & 0x3F;
        println!("  f1={:3} ({:5.0} Hz)  f2={:3} ({:5.0} Hz)  dt={:2}  occ={}", f1, f1 as f64 * 10.766, f2, f2 as f64 * 10.766, dt, n);
    }

    // dt distribution of A's hashes
    let mut dts = [0usize; 64];
    for &(h, _) in &a.hashes { dts[(h & 0x3F) as usize] += 1; }
    println!("\nA dt distribution (dt: count):");
    for (dt, n) in dts.iter().enumerate() {
        if *n > 0 { println!("  dt={}: {}", dt, n); }
    }

    // cluster analysis: ghost (E vs A) and true (A vs mix)
    let mix = fingerprint_file(Path::new("test_data/mix.wav")).unwrap().0;
    analyze_clusters("GHOST E-vs-A", &a, &e);
    analyze_clusters("TRUE  A-vs-mix", &mix, &a);
}

fn analyze_clusters(label: &str, mix: &mixid_core::Fingerprint, track: &mixid_core::Fingerprint) {
    use std::collections::{HashMap, HashSet};
    let mut tmap: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(h, t) in &track.hashes {
        let ts = tmap.entry(h).or_default();
        if ts.len() < 8 { ts.push(t); }
    }
    let mut mix_occ: HashMap<u32, u32> = HashMap::new();
    for &(h, _) in &mix.hashes { *mix_occ.entry(h).or_insert(0) += 1; }
    // votes: offset -> (votes, distinct hashes)
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
    let mut v: Vec<(i64, u32, usize)> = votes.into_iter().map(|(o, (n, s))| (o, n, s.len())).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    println!("\n{label}: top offset clusters (offset, votes, distinct):");
    for (o, n, d) in v.iter().take(8) {
        println!("  offset={:6} votes={:5} distinct={:4}", o, n, d);
    }
    let track_distinct = tmap.len();
    println!("{label}: track distinct hashes (capped): {}", track_distinct);
}
