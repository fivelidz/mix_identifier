//! Self-contained integration tests for mixid-core.
//!
//! No external audio files, no Python, no network. Songs are synthesized in
//! memory and written as minimal PCM s16le RIFF/WAVE files via `std::fs`
//! (`hound` is not a workspace dependency, so we hand-roll the ~30-line
//! header writer; symphonia's `wav` + `pcm` features decode it fine).
//!
//! Audio design notes: the fingerprinter bans self-pairs (f1 == f2) from
//! sustained tones, so a steady sine yields almost no usable hashes. Each
//! synthetic song is therefore a random-walk pentatonic melody (two octaves)
//! where every note is built from 3 harmonics with a fast attack + exponential
//! decay, plus a short seeded noise burst at each onset (percussion). This
//! gives the STFT constellation rich, time-varying peaks. Distinct seeds and
//! base frequencies keep the songs' hash sets well separated.
//!
//! The mix is a plain concatenation of 12 s from each of Alpha/Beta/Gamma at
//! known timestamps (0 s, 12 s, 24 s). Delta is indexed but never spliced in —
//! the decoy.

use mixid_core::fingerprint::{fingerprint_samples, FRAME_S, TARGET_SR};
use mixid_core::{analyze_mix, fingerprint_file, AnalysisResult, Db};
use std::f32::consts::PI;
use std::io::Write;
use std::path::{Path, PathBuf};

const SR: u32 = 44100;
const SONG_SECS: f64 = 30.0;
const SEG_SECS: usize = 12;

/// (title, synth seed, base frequency Hz). Delta is the decoy.
const SONGS: &[(&str, u64, f32)] = &[
    ("Alpha", 0x00A1_0CE1, 196.0),
    ("Beta", 0x00BE_EF70, 247.0),
    ("Gamma", 0x00CA_FE90, 311.0),
    ("Delta", 0x00D0_D0D0, 370.0),
];

// ---------------------------------------------------------------- helpers

/// Tiny xorshift64 PRNG — deterministic per seed, no deps.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn f01(&mut self) -> f32 {
        self.next_u64() as f32 / u64::MAX as f32
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// Synthesize a "song": random-walk pentatonic melody over two octaves, each
/// note = 3 harmonics (1.0 / 0.5 / 0.25) with ~5 ms attack and exponential
/// decay, plus a 6 ms seeded noise burst at every note onset.
fn synth_song(seed: u64, base_hz: f32, sr: u32, duration_s: f64) -> Vec<f32> {
    const PENTA: [f32; 5] = [0.0, 3.0, 5.0, 7.0, 10.0]; // minor pentatonic
    let n = (sr as f64 * duration_s) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Rng(seed | 1);
    let mut deg: usize = rng.below(10) as usize;
    let mut t = 0usize;
    let click_len = (0.006 * sr as f64) as usize;
    while t < n {
        // 160..260 ms per note -> ~4-6 STFT frames per note at TARGET_SR
        let note_len = ((0.16 + rng.f01() as f64 * 0.10) * sr as f64) as usize;
        let step = rng.below(5) as i32 - 2; // -2..=+2
        deg = ((deg as i32 + step).rem_euclid(10)) as usize;
        let semis = PENTA[deg % 5] + 12.0 * (deg / 5) as f32;
        let f0 = base_hz * (2.0f32).powf(semis / 12.0);
        let tau = note_len as f32 * 0.45;
        for i in 0..note_len {
            let idx = t + i;
            if idx >= n {
                break;
            }
            let ts = i as f32;
            let attack = if ts < 200.0 { ts / 200.0 } else { 1.0 };
            let env = attack * (-ts / tau).exp();
            let mut s = 0.0f32;
            for (h, a) in [(1.0f32, 1.0f32), (2.0, 0.5), (3.0, 0.25)] {
                s += a * (2.0 * PI * f0 * h * ts / sr as f32).sin();
            }
            out[idx] += s * env * 0.22;
        }
        // percussive onset: decaying noise burst, unique per seed
        for i in 0..click_len.min(n - t) {
            let decay = 1.0 - i as f32 / click_len as f32;
            out[t + i] += (rng.f01() * 2.0 - 1.0) * 0.35 * decay;
        }
        t += note_len;
    }
    out
}

/// Minimal RIFF/WAVE writer: PCM s16le, mono. No dependencies.
fn write_wav(path: &Path, samples: &[f32], sr: u32) -> std::io::Result<()> {
    let mut data = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        data.extend_from_slice(&v.to_le_bytes());
    }
    let mut f = std::fs::File::create(path)?;
    let byte_rate = sr * 2; // mono * 2 bytes
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data.len() as u32).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?; // fmt chunk size
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&1u16.to_le_bytes())?; // mono
    f.write_all(&sr.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?; // block align
    f.write_all(&16u16.to_le_bytes())?; // bits per sample
    f.write_all(b"data")?;
    f.write_all(&(data.len() as u32).to_le_bytes())?;
    f.write_all(&data)?;
    Ok(())
}

/// Temp fixture: 4 song wavs + one mix wav. Removed (best effort) on drop.
struct Fixture {
    dir: PathBuf,
    songs: Vec<(&'static str, PathBuf)>,
    /// Alpha[0..12] + Beta[0..12] + Gamma[0..12], plain concatenation.
    mix: PathBuf,
}

impl Fixture {
    fn song(&self, title: &str) -> &Path {
        &self.songs.iter().find(|(t, _)| *t == title).unwrap().1
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn build_fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("mixid_it_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut songs = Vec::new();
    let mut bank: Vec<(&'static str, Vec<f32>)> = Vec::new();
    for &(title, seed, base) in SONGS {
        let samples = synth_song(seed, base, SR, SONG_SECS);
        let path = dir.join(format!("{title}.wav"));
        write_wav(&path, &samples, SR).unwrap();
        songs.push((title, path));
        bank.push((title, samples));
    }
    let seg = SEG_SECS * SR as usize;
    let mut mix_samples = Vec::with_capacity(seg * 3);
    for title in ["Alpha", "Beta", "Gamma"] {
        let s = &bank.iter().find(|(t, _)| *t == title).unwrap().1;
        mix_samples.extend_from_slice(&s[..seg]);
    }
    let mix = dir.join("the_mix.wav");
    write_wav(&mix, &mix_samples, SR).unwrap();
    Fixture { dir, songs, mix }
}

/// Index all four songs (decoy included) and analyze the mix with the exact
/// parameters the CLI uses (min_confidence 0.35, min_duration 8.0).
fn run_analysis(name: &str) -> (Fixture, AnalysisResult) {
    let fx = build_fixture(name);
    let db_path = std::env::temp_dir().join(format!("mixid_test_{name}_{}.db", std::process::id()));
    let result = {
        let mut db = Db::open(&db_path).unwrap();
        for (title, path) in &fx.songs {
            let (fp, dur) = fingerprint_file(path).unwrap();
            db.add_track(title, "Test Artist", &path.display().to_string(), dur, &fp)
                .unwrap();
        }
        analyze_mix(&mut db, &fx.mix, Some("Integration Mix"), 0.35, 8.0).unwrap()
    };
    let _ = std::fs::remove_file(&db_path); // best effort
    (fx, result)
}

// ------------------------------------------------------------------ tests

#[test]
fn test_fingerprint_basic() {
    let fx = build_fixture("fp_basic");
    let (fp, dur) = fingerprint_file(fx.song("Alpha")).unwrap();
    assert!((dur - SONG_SECS).abs() < 0.05, "unexpected duration: {dur}");
    assert!(
        fp.hashes.len() > 1000,
        "too few hashes: {}",
        fp.hashes.len()
    );
    // every anchor time must fall inside the song
    let max_t = fp.hashes.iter().map(|&(_, t)| t).max().unwrap();
    assert!(max_t as f64 * FRAME_S < dur, "anchor beyond duration");
}

#[test]
fn test_fingerprint_samples_direct() {
    // The samples-level API consumes raw f32 at TARGET_SR directly.
    let s = synth_song(0x5EED_1234, 220.0, TARGET_SR, 5.0);
    let fp = fingerprint_samples(&s);
    assert!(!fp.hashes.is_empty(), "no hashes from direct samples");
}

#[test]
fn test_analyze_mix_detects_all() {
    let (_fx, res) = run_analysis("detect");
    assert!(
        (res.duration - 36.0).abs() < 0.1,
        "mix duration: {}",
        res.duration
    );
    let titles: Vec<&str> = res.detections.iter().map(|d| d.title.as_str()).collect();
    assert_eq!(
        titles.len(),
        3,
        "expected exactly 3 detections, got {titles:?}"
    );
    for (title, expect_start) in [("Alpha", 0.0), ("Beta", 12.0), ("Gamma", 24.0)] {
        let d = res
            .detections
            .iter()
            .find(|d| d.title == title)
            .unwrap_or_else(|| panic!("{title} not detected; got {titles:?}"));
        assert!(
            (d.t_start - expect_start).abs() <= 3.0,
            "{title}: t_start {} vs expected {expect_start}",
            d.t_start
        );
        assert!(d.confidence >= 0.35, "{title}: confidence {}", d.confidence);
        assert!(
            d.t_end - d.t_start >= 8.0,
            "{title}: span {}",
            d.t_end - d.t_start
        );
    }
}

#[test]
fn test_decoy_rejected() {
    let (_fx, res) = run_analysis("decoy");
    let decoys: Vec<_> = res
        .detections
        .iter()
        .filter(|d| d.title == "Delta")
        .collect();
    assert!(decoys.is_empty(), "decoy song was detected: {decoys:#?}");
}

#[test]
fn test_db_roundtrip() {
    let fx = build_fixture("db_rt");
    let db_path = std::env::temp_dir().join(format!("mixid_test_db_{}.db", std::process::id()));
    {
        let mut db = Db::open(&db_path).unwrap();

        // add_track for two songs
        let mut ids = Vec::new();
        for (title, path) in fx.songs.iter().take(2) {
            let (fp, dur) = fingerprint_file(path).unwrap();
            let id = db
                .add_track(title, "Artist X", &path.display().to_string(), dur, &fp)
                .unwrap();
            ids.push(id);
        }
        assert_eq!(db.tracks().unwrap().len(), 2);

        // re-adding the same path upserts (keeps id), does not duplicate
        let (fp, dur) = fingerprint_file(fx.song("Alpha")).unwrap();
        let again = db
            .add_track(
                "Alpha",
                "Artist X",
                &fx.song("Alpha").display().to_string(),
                dur,
                &fp,
            )
            .unwrap();
        assert_eq!(again, ids[0]);
        assert_eq!(db.tracks().unwrap().len(), 2);

        // search_tracks
        let found = db.search_tracks("Beta").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Beta");
        assert_eq!(found[0].id, ids[1]);
        assert!(db.search_tracks("No Such Song").unwrap().is_empty());

        // all_track_fingerprints round-trips the hashes
        let fps = db.all_track_fingerprints().unwrap();
        assert_eq!(fps.len(), 2);
        assert!(fps.iter().all(|(_, f)| f.hashes.len() > 1000));

        // mix + detections
        let mix_id = db.add_mix("RT Mix", "/tmp/mixid_rt_mix.wav", 36.0).unwrap();
        db.clear_detections(mix_id).unwrap();
        db.add_detection(mix_id, ids[0], 0.0, 12.0, 0.9).unwrap();
        db.add_detection(mix_id, ids[1], 12.0, 24.0, 0.8).unwrap();

        let tracklist = db.mix_tracklist(mix_id).unwrap();
        assert_eq!(tracklist.len(), 2);
        assert_eq!(tracklist[0].title, "Alpha");
        assert_eq!(tracklist[1].track_id, ids[1]);

        let in_mix = db.mixes_containing_track(ids[1]).unwrap();
        assert_eq!(in_mix.len(), 1);
        assert_eq!(in_mix[0].mix_title, "RT Mix");
        assert!((in_mix[0].t_start - 12.0).abs() < 1e-9);
        assert!((in_mix[0].confidence - 0.8).abs() < 1e-9);

        let mixes = db.mixes().unwrap();
        assert_eq!(mixes.len(), 1);
        assert_eq!(mixes[0].track_count, 2);
    } // Db dropped -> connection closed
    let _ = std::fs::remove_file(&db_path); // best effort cleanup
}

#[test]
fn test_get_and_delete() {
    let fx = build_fixture("del");
    let db_path = std::env::temp_dir().join(format!("mixid_test_del_{}.db", std::process::id()));
    {
        let mut db = Db::open(&db_path).unwrap();
        let mut ids = Vec::new();
        for (title, path) in fx.songs.iter().take(2) {
            let (fp, dur) = fingerprint_file(path).unwrap();
            ids.push(
                db.add_track(title, "Artist X", &path.display().to_string(), dur, &fp)
                    .unwrap(),
            );
        }
        let mix_id = db
            .add_mix("Del Mix", "/tmp/mixid_del_mix.wav", 36.0)
            .unwrap();
        db.add_detection(mix_id, ids[0], 0.0, 12.0, 0.9).unwrap();
        db.add_detection(mix_id, ids[1], 12.0, 24.0, 0.8).unwrap();

        // get_mix: by id, with path and detection count
        let m = db.get_mix(mix_id).unwrap().expect("mix exists");
        assert_eq!(m.title, "Del Mix");
        assert_eq!(m.path, "/tmp/mixid_del_mix.wav");
        assert_eq!(m.track_count, 2);
        assert!(db.get_mix(9999).unwrap().is_none());

        // delete_track cascades its detections but keeps the mix
        assert!(db.delete_track(ids[0]).unwrap());
        assert!(!db.delete_track(ids[0]).unwrap()); // already gone
        assert_eq!(db.tracks().unwrap().len(), 1);
        assert_eq!(db.mix_tracklist(mix_id).unwrap().len(), 1);
        assert_eq!(db.get_mix(mix_id).unwrap().unwrap().track_count, 1);

        // delete_mix removes it and its remaining detections
        assert!(db.delete_mix(mix_id).unwrap());
        assert!(!db.delete_mix(mix_id).unwrap()); // already gone
        assert!(db.get_mix(mix_id).unwrap().is_none());
        assert!(db.mixes().unwrap().is_empty());
        // the surviving track's fingerprint is untouched
        assert_eq!(db.all_track_fingerprints().unwrap().len(), 1);
    }
    let _ = std::fs::remove_file(&db_path); // best effort cleanup
}
