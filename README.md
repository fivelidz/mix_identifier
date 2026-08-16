# MixID — Shazam for DJ Mixes

Identify which songs play in a DJ mix, and at what time. Search a song and
find every mix that contains it. Pure Rust, works offline, ships as a CLI, a
web server, and an Android app.

```
[00:00] Kolter - Nirvana Edit     (1.00)
[00:58] Loofy - Last Night        (1.00)
[01:53] Sammy Virji - 925         (1.00)
[02:51] FISHER - OCEAN            (1.00)
```

## How it works

1. **Fingerprint** every track in your music library (Shazam-style spectral
   constellation: STFT → local-maxima peaks → anchor/target hash pairs).
   Pure Rust (`symphonia` decode + `rustfft`) — no C dependencies, compiles
   for Android.
2. **Fingerprint the mix**, then for every library track, vote on
   time-offsets (`mix_frame − track_frame`). A track playing at position T
   produces a massive cluster of agreeing offsets.
3. Score each offset independently by **IDF-weighted distinct-hash evidence**
   (hashes shared across many library tracks are discounted — house-music
   kick patterns etc.), extract contiguous time runs, trim boundaries
   adaptively.
4. Store everything in SQLite: search a song → every mix containing it, with
   in/out timestamps and confidence.

Verified end-to-end on real music (4-track house mix built with ffmpeg
crossfades): all 4 tracks detected, boundaries within seconds, decoy track
correctly rejected. Synthetic regression test: `scripts/verify_test.py`.

## Layout

| Path | What |
|---|---|
| `crates/mixid-core` | Fingerprinting, matching, SQLite store |
| `crates/mixid-cli` | `mixid` command-line tool |
| `crates/mixid-server` | axum web server + UI (`static/index.html`) |
| `app/` | Tauri 2 Android app (wraps mixid-core) |
| `scripts/make_test_audio.py` | Synthetic test-song/mix generator |
| `scripts/verify_test.py` | End-to-end regression test |

## CLI usage

```bash
cargo build --release

# Index your music library (mp3/flac/wav/ogg/m4a/aac)
./target/release/mixid --db mixid.db index ~/Music/DJ_Library

# Analyze a mix → tracklist with timestamps
./target/release/mixid --db mixid.db analyze ~/Mixes/set.mp3 --title "Friday set"

# Search: which mixes contain a song?
./target/release/mixid --db mixid.db search "Last Night"

# Browse
./target/release/mixid --db mixid.db mixes
./target/release/mixid --db mixid.db tracks
```

Filenames like `Artist - Title.ext` are split automatically.

## Web server

```bash
MIXID_DB=mixid.db ./target/release/mixid-server   # http://localhost:8900
```

UI tabs: **Analyze** (upload a mix → tracklist), **Search** (song → mixes),
**Mixes** (browse). REST API: `GET /api/mixes`, `/api/mixes/{id}`,
`/api/tracks/search?q=`, `POST /api/analyze` (multipart file or JSON path).

## Android app

```bash
cd app
ANDROID_HOME=~/Android/Sdk NDK_HOME=~/Android/Sdk/ndk/28.2.13676358 \
JAVA_HOME=/usr/lib/jvm/java-17-openjdk \
cargo tauri android build --apk --debug --target aarch64
```

APK: `app/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`

Install on the Redmi (HyperOS auto-denies plain `adb install`):
```bash
~/projects/phone_projects/camera_system/install_with_miui_dialog.sh <apk>
```

On-device: Library tab → pick your music folder → index; Analyze tab → pick
a mix file → tracklist. All local, no network.

## Tuning knobs (crates/mixid-core/src/matcher.rs)

| Constant | Meaning |
|---|---|
| `MIN_WEIGHTED` (60) | minimum IDF-weighted distinct hashes for a segment |
| `WEIGHTED_FULL_MATCH` (300) | evidence for confidence 1.0 |
| `MIN_DENSITY` (0.25) | matched-frame coverage over the segment span |
| `MAX_HASH_OCCURRENCES` (8) | per-hash occurrence cap (loop-heavy tracks) |
| `RUN_GAP_TOL` (~8s) | gap that splits a run (anti-stitching) |

## Known limitations

- Matches only what's in your indexed library (it's not the Shazam catalog).
- Extreme tempo/pitch-shifted edits match less reliably (offset drift).
- Very short snippets (<15s) of repetitive tracks can fall below the
  evidence threshold — the system is tuned for mix analysis, not
  identify-a-10s-clip.
- Confidence ≈ "how much unique fingerprint material aligned", not playback
  loudness.

## Development notes

The matcher design was calibrated empirically (see git history):
- distinct-hash evidence beats vote counts (occurrence multiplicity inflates ghosts)
- IDF weighting beats hard df-thresholds for small same-genre libraries
- per-offset scoring beats offset-cluster pooling (ghosts pool deceptively)
- 2048-point FFT (5.4Hz bins) separates nearby pitches that 1024 collapsed
- self-pair hashes (f1==f2) from sustained tones are ghost fuel — banned

Debug helpers: `cargo run -p mixid-core --example dbg` (hash sharing stats),
`--example dbg2 <mix> <track>` (offset cluster stats).
