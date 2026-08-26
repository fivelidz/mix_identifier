//! Tracklist export: CUE sheets, CSV, and extended M3U.
//!
//! All formatters take the mix title plus the ordered detection list and
//! return a String suitable for writing to a file (no trailing newline
//! conventions beyond what each format expects).

use crate::DetectionRow;

/// Seconds → CUE `MM:SS:FF` (75 frames/second, the Red Book standard).
fn cue_time(s: f64) -> String {
    let s = s.max(0.0);
    let total_frames = (s * 75.0).round() as u64;
    let frames = total_frames % 75;
    let secs = (total_frames / 75) % 60;
    let mins = total_frames / 75 / 60;
    format!("{mins:02}:{secs:02}:{frames:02}")
}

fn cue_escape(s: &str) -> String {
    s.replace('"', "'")
}

/// Standard CUE sheet: one AUDIO track per detection, INDEX 01 at t_start.
/// Most players (foobar2000, rekordbox, VLC, burners) can split a mix with it.
pub fn export_cue(mix_title: &str, mix_file: &str, detections: &[DetectionRow]) -> String {
    let mut out = String::new();
    out.push_str(&format!("TITLE \"{}\"\n", cue_escape(mix_title)));
    out.push_str(&format!("FILE \"{}\" MP3\n", cue_escape(mix_file)));
    for (i, d) in detections.iter().enumerate() {
        let performer = if d.artist.is_empty() { "" } else { &d.artist };
        out.push_str(&format!("  TRACK {:02} AUDIO\n", i + 1));
        out.push_str(&format!("    TITLE \"{}\"\n", cue_escape(&d.title)));
        out.push_str(&format!("    PERFORMER \"{}\"\n", cue_escape(performer)));
        out.push_str(&format!("    INDEX 01 {}\n", cue_time(d.t_start)));
    }
    out
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// CSV: start,end,confidence,artist,title (seconds, RFC-4180 quoting).
pub fn export_csv(mix_title: &str, detections: &[DetectionRow]) -> String {
    let mut out = String::from("mix,start_s,end_s,confidence,artist,title\n");
    for d in detections {
        out.push_str(&format!(
            "{},{:.1},{:.1},{:.3},{},{}\n",
            csv_escape(mix_title),
            d.t_start,
            d.t_end,
            d.confidence,
            csv_escape(&d.artist),
            csv_escape(&d.title),
        ));
    }
    out
}

/// Extended M3U: `#EXTINF:<duration>,Artist - Title` per detection. Useful as
/// a plain "what played" playlist / setlist.
pub fn export_m3u(mix_title: &str, detections: &[DetectionRow]) -> String {
    let mut out = String::from("#EXTM3U\n");
    out.push_str(&format!("#Playlist: {}\n", mix_title));
    for d in detections {
        let dur = (d.t_end - d.t_start).max(0.0);
        let label = if d.artist.is_empty() {
            d.title.clone()
        } else {
            format!("{} - {}", d.artist, d.title)
        };
        out.push_str(&format!("#EXTINF:{:.0},{}\n", dur, label));
        // No real per-track file exists; emit the timestamped label as the
        // entry so the list is still readable in any player.
        out.push_str(&format!(
            "{} — {} [{}]\n",
            label,
            mix_title,
            cue_time(d.t_start)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(track_id: i64, title: &str, artist: &str, t_start: f64, t_end: f64) -> DetectionRow {
        DetectionRow {
            track_id,
            title: title.into(),
            artist: artist.into(),
            t_start,
            t_end,
            confidence: 0.99,
        }
    }

    #[test]
    fn cue_time_formats() {
        assert_eq!(cue_time(0.0), "00:00:00");
        assert_eq!(cue_time(61.5), "01:01:38"); // 61.5s * 75 = 4612.5 → 4613 → 1:01:38
        assert_eq!(cue_time(3661.0), "61:01:00"); // over an hour: MM keeps counting
    }

    #[test]
    fn cue_sheet_shape() {
        let dets = vec![
            det(1, "Nirvana Edit", "Kolter", 0.0, 58.0),
            det(2, "Last Night", "Loofy", 58.0, 113.0),
        ];
        let cue = export_cue("Friday set", "friday.mp3", &dets);
        assert!(cue.contains("TITLE \"Friday set\""));
        assert!(cue.contains("FILE \"friday.mp3\" MP3"));
        assert!(cue.contains("TRACK 01 AUDIO"));
        assert!(cue.contains("TITLE \"Nirvana Edit\""));
        assert!(cue.contains("PERFORMER \"Kolter\""));
        assert!(cue.contains("INDEX 01 00:00:00"));
        assert!(cue.contains("INDEX 01 00:58:00"));
        assert!(cue.contains("TRACK 02 AUDIO"));
    }

    #[test]
    fn cue_escapes_quotes() {
        let dets = vec![det(1, "Say \"Hi\"", "DJ", 0.0, 10.0)];
        let cue = export_cue("t", "t.mp3", &dets);
        assert!(cue.contains("'Hi'"));
        assert!(!cue.contains("\\\""));
    }

    #[test]
    fn csv_quotes_commas() {
        let dets = vec![det(1, "Love, Again", "A,B", 0.0, 60.0)];
        let csv = export_csv("My, Mix", &dets);
        assert!(csv.starts_with("mix,start_s,end_s,confidence,artist,title\n"));
        assert!(csv.contains("\"My, Mix\""));
        assert!(csv.contains("\"A,B\",\"Love, Again\""));
    }

    #[test]
    fn m3u_shape() {
        let dets = vec![det(1, "Nine", "Sammy Virji", 0.0, 60.0)];
        let m3u = export_m3u("Set", &dets);
        assert!(m3u.starts_with("#EXTM3U\n"));
        assert!(m3u.contains("#Playlist: Set\n"));
        assert!(m3u.contains("#EXTINF:60,Sammy Virji - Nine\n"));
    }
}
