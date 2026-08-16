//! Filename → (artist, title) parsing, shared by CLI, server and app.
//!
//! Convention: `Artist - Title.ext`. DJ-pool rips often prefix a track
//! number (`03 - Artist - Title.ext`); when the leading segment is purely
//! numeric AND at least one more separator follows, the number is dropped
//! so it doesn't end up as the "artist".

/// Split a filename stem into `(artist, title)`.
///
/// ```
/// use mixid_core::split_artist_title;
/// assert_eq!(split_artist_title("Kolter - Nirvana Edit"), ("Kolter".into(), "Nirvana Edit".into()));
/// assert_eq!(split_artist_title("03 - Sammy Virji - 925"), ("Sammy Virji".into(), "925".into()));
/// assert_eq!(split_artist_title("925"), ("".into(), "925".into()));
/// ```
pub fn split_artist_title(stem: &str) -> (String, String) {
    let parts: Vec<&str> = stem.split(" - ").collect();
    let parts = if parts.len() >= 3
        && !parts[0].trim().is_empty()
        && parts[0].trim().chars().all(|c| c.is_ascii_digit())
    {
        &parts[1..]
    } else {
        &parts[..]
    };
    if parts.len() >= 2 && !parts[0].trim().is_empty() {
        (
            parts[0].trim().to_string(),
            parts[1..].join(" - ").trim().to_string(),
        )
    } else {
        (String::new(), stem.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_convention() {
        assert_eq!(
            split_artist_title("Kolter - Nirvana Edit"),
            ("Kolter".to_string(), "Nirvana Edit".to_string())
        );
    }

    #[test]
    fn numbered_dj_pool() {
        assert_eq!(
            split_artist_title("03 - Sammy Viriji - 925"),
            ("Sammy Viriji".to_string(), "925".to_string())
        );
        assert_eq!(
            split_artist_title("12 - FISHER - OCEAN"),
            ("FISHER".to_string(), "OCEAN".to_string())
        );
    }

    #[test]
    fn numeric_artist_is_kept_when_only_one_separator() {
        // "925 - Remix" is a literal artist name choice, not a track number
        assert_eq!(
            split_artist_title("925 - Remix"),
            ("925".to_string(), "Remix".to_string())
        );
    }

    #[test]
    fn no_separator() {
        assert_eq!(
            split_artist_title("untitled"),
            (String::new(), "untitled".to_string())
        );
    }

    #[test]
    fn extra_whitespace_trimmed() {
        assert_eq!(
            split_artist_title("  Loofy  -   Last Night "),
            ("Loofy".to_string(), "Last Night".to_string())
        );
    }

    #[test]
    fn three_separators_no_number() {
        // No numeric prefix: keep first split (title may legitimately contain " - ")
        assert_eq!(
            split_artist_title("A - B - C"),
            ("A".to_string(), "B - C".to_string())
        );
    }
}
