//! MixID CLI — index a music library, analyze mixes, search.
//!
//! Examples:
//!   mixid index ~/Music
//!   mixid analyze ~/Mixes/set.wav
//!   mixid search "song name"
//!   mixid mixes

use anyhow::Result;
use clap::{Parser, Subcommand};
use mixid_core::{analyze_mix, fingerprint_file, split_artist_title, Db};
use std::path::{Path, PathBuf};

const AUDIO_EXTS: &[&str] = &[
    "mp3", "flac", "wav", "ogg", "oga", "m4a", "aac", "aiff", "aif",
];

fn default_db() -> String {
    std::env::var("MIXID_DB").unwrap_or_else(|_| "mixid.db".to_string())
}

#[derive(Parser)]
#[command(name = "mixid", about = "Shazam for DJ mixes")]
struct Cli {
    /// Path to the SQLite database
    #[arg(long, global = true, default_value_t = default_db())]
    db: String,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Fingerprint every audio file in a directory into the library
    Index { dir: PathBuf },
    /// Identify the tracks in a mix file, with timestamps
    Analyze {
        file: PathBuf,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Search tracks by name; show which mixes contain them
    Search { query: String },
    /// List analyzed mixes
    Mixes,
    /// List indexed tracks
    Tracks,
    /// Delete a mix (and its detections) by id
    Delete {
        #[command(subcommand)]
        target: DeleteTarget,
    },
    /// Export a mix tracklist as CUE / CSV / M3U to stdout
    Export {
        mix_id: i64,
        #[arg(long, default_value = "cue")]
        format: String,
    },
}

#[derive(Subcommand)]
enum DeleteTarget {
    /// Delete a mix by id (see `mixid mixes`)
    Mix { id: i64 },
    /// Delete a library track by id (see `mixid tracks`)
    Track { id: i64 },
}

fn fmt_time(s: f64) -> String {
    let m = (s / 60.0).floor() as u64;
    let sec = (s - m as f64 * 60.0).round() as u64;
    format!("{:02}:{:02}", m, sec)
}

fn collect_audio(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_audio(&p, out);
        } else if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
            if AUDIO_EXTS.contains(&ext.to_ascii_lowercase().as_str()) {
                out.push(p);
            }
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Index { dir } => {
            let mut files = Vec::new();
            collect_audio(&dir, &mut files);
            files.sort();
            if files.is_empty() {
                println!("No audio files found under {}", dir.display());
                return Ok(());
            }
            let mut db = Db::open(&cli.db)?;
            let total = files.len();
            let mut failed: Vec<String> = Vec::new();
            for (i, f) in files.iter().enumerate() {
                let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown");
                let (artist, title) = split_artist_title(stem);
                print!("[{}/{}] {} — {} ... ", i + 1, total, artist, title);
                use std::io::Write;
                let _ = std::io::stdout().flush();
                match fingerprint_file(f) {
                    Ok((fp, dur)) => {
                        db.add_track(&title, &artist, &f.display().to_string(), dur, &fp)?;
                        println!("ok ({}s, {} hashes)", dur as u64, fp.hashes.len());
                    }
                    Err(e) => {
                        println!("FAILED: {e:#}");
                        failed.push(f.display().to_string());
                    }
                }
            }
            println!(
                "\nIndexed {} / {} files into {}",
                total - failed.len(),
                total,
                cli.db
            );
            if !failed.is_empty() {
                println!("Failed files:");
                for f in &failed {
                    println!("  {f}");
                }
            }
        }
        Cmd::Analyze { file, title, json } => {
            let mut db = Db::open(&cli.db)?;
            let result = analyze_mix(&mut db, &file, title.as_deref(), 0.35, 8.0)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!(
                    "{} ({}s) — {} tracks detected:",
                    result.title,
                    result.duration as u64,
                    result.detections.len()
                );
                for d in &result.detections {
                    let artist = if d.artist.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", d.artist)
                    };
                    println!(
                        "[{}] {}{} ({}–{}, {:.2})",
                        fmt_time(d.t_start),
                        d.title,
                        artist,
                        fmt_time(d.t_start),
                        fmt_time(d.t_end),
                        d.confidence
                    );
                }
            }
        }
        Cmd::Search { query } => {
            let db = Db::open(&cli.db)?;
            let tracks = db.search_tracks(&query)?;
            if tracks.is_empty() {
                println!("No tracks match {query:?}");
                return Ok(());
            }
            for t in &tracks {
                println!(
                    "{} — {} ({}s, in {} mix(es))",
                    t.title, t.artist, t.duration as u64, t.mix_count
                );
                for m in db.mixes_containing_track(t.id)? {
                    println!(
                        "    in \"{}\" at {}–{} ({:.2})",
                        m.mix_title,
                        fmt_time(m.t_start),
                        fmt_time(m.t_end),
                        m.confidence
                    );
                }
            }
        }
        Cmd::Mixes => {
            let db = Db::open(&cli.db)?;
            for m in db.mixes()? {
                println!(
                    "#{} {} ({}s, {} tracks, added {})",
                    m.id, m.title, m.duration as u64, m.track_count, m.added_at
                );
            }
        }
        Cmd::Tracks => {
            let db = Db::open(&cli.db)?;
            for t in db.tracks()? {
                println!(
                    "#{} {} — {} ({}s, {} hashes in {} mixes)",
                    t.id,
                    t.title,
                    t.artist,
                    t.duration as u64,
                    t.mix_count.max(0),
                    t.mix_count
                );
            }
        }
        Cmd::Delete { target } => {
            let mut db = Db::open(&cli.db)?;
            match target {
                DeleteTarget::Mix { id } => {
                    if db.delete_mix(id)? {
                        println!("Deleted mix #{id}");
                    } else {
                        println!("No mix #{id}");
                    }
                }
                DeleteTarget::Track { id } => {
                    if db.delete_track(id)? {
                        println!("Deleted track #{id}");
                    } else {
                        println!("No track #{id}");
                    }
                }
            }
        }
        Cmd::Export { mix_id, format } => {
            let db = Db::open(&cli.db)?;
            let mix = db
                .get_mix(mix_id)?
                .ok_or_else(|| anyhow::anyhow!("no mix #{mix_id} (see `mixid mixes`)"))?;
            let tracklist = db.mix_tracklist(mix_id)?;
            let file_name = std::path::Path::new(&mix.path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| mix.title.clone());
            let out = match format.to_ascii_lowercase().as_str() {
                "cue" => mixid_core::export::export_cue(&mix.title, &file_name, &tracklist),
                "csv" => mixid_core::export::export_csv(&mix.title, &tracklist),
                "m3u" => mixid_core::export::export_m3u(&mix.title, &tracklist),
                other => {
                    return Err(anyhow::anyhow!(
                        "unknown format {other:?} (use cue, csv or m3u)"
                    ))
                }
            };
            print!("{out}");
        }
    }
    Ok(())
}
