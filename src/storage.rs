//! On-disk session storage: directory resolution, filenames, timestamps.
//!
//! Split so the pure parts (fields -> filename, env -> path, seconds ->
//! fields) are testable without touching the filesystem, and the one
//! function that does touch it takes a `&Path` so tests hand it a tempdir.

use crate::session::Timestamp;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const DEFAULT_SUBDIR: &str = "caleb";
pub const FILE_EXTENSION: &str = ".md";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResolveError {
    #[error("neither $XDG_DATA_HOME nor $HOME is set")]
    NoStorageDir,
}

/// Follows XDG: `$XDG_DATA_HOME/caleb`, else `$HOME/.local/share/caleb`.
/// An empty string counts as unset.
pub fn resolve_storage_dir(xdg: Option<&str>, home: Option<&str>) -> Result<PathBuf, ResolveError> {
    if let Some(x) = xdg.filter(|s| !s.is_empty()) {
        return Ok(Path::new(x).join(DEFAULT_SUBDIR));
    }
    if let Some(h) = home.filter(|s| !s.is_empty()) {
        return Ok(Path::new(h)
            .join(".local")
            .join("share")
            .join(DEFAULT_SUBDIR));
    }
    Err(ResolveError::NoStorageDir)
}

pub fn default_storage_dir() -> Result<PathBuf, ResolveError> {
    let xdg = std::env::var("XDG_DATA_HOME").ok();
    let home = std::env::var("HOME").ok();
    resolve_storage_dir(xdg.as_deref(), home.as_deref())
}

/// Convert UTC Unix seconds into wall-clock fields. Negative input clamps
/// to the epoch — the only sane fallback for a clock behind 1970.
///
/// Nothing in the binary calls this: `timestamp_now` goes through the local
/// zone instead. It stays because it is the deterministic, seconds-in /
/// fields-out core that the conversion tests can actually pin down.
#[allow(dead_code)]
pub fn timestamp_from_unix_seconds(secs: i64) -> Timestamp {
    let secs = secs.max(0);
    let dt = jiff::Timestamp::from_second(secs)
        .expect("in-range unix seconds")
        .to_zoned(jiff::tz::TimeZone::UTC)
        .datetime();
    from_civil(dt)
}

/// Local wall-clock now. jiff reads the system zone (`$TZ`, then
/// `/etc/localtime`) itself, in pure Rust — no hand-written TZif parsing and
/// no libc dependency.
pub fn timestamp_now() -> Timestamp {
    from_civil(jiff::Zoned::now().datetime())
}

fn from_civil(dt: jiff::civil::DateTime) -> Timestamp {
    Timestamp {
        year: dt.year() as u16,
        month: dt.month() as u8,
        day: dt.day() as u8,
        hour: dt.hour() as u8,
        minute: dt.minute() as u8,
    }
}

pub fn format_file_stem(ts: Timestamp) -> String {
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}",
        ts.year, ts.month, ts.day, ts.hour, ts.minute
    )
}

/// Find a name in `dir` that does not collide: `<stem><ext>`, then
/// `<stem>-2<ext>`, `<stem>-3<ext>`, ...
///
/// This is check-then-create. For a single-user local CLI that is fine; a
/// concurrent writer could race in between, but that is not caleb's model.
pub fn unique_filename(dir: &Path, stem: &str, ext: &str) -> std::io::Result<String> {
    let candidate = format!("{stem}{ext}");
    if !dir.join(&candidate).exists() {
        return Ok(candidate);
    }
    for n in 2u32.. {
        let candidate = format!("{stem}-{n}{ext}");
        if !dir.join(&candidate).exists() {
            return Ok(candidate);
        }
    }
    unreachable!("u32 range is effectively unbounded here")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_maps_to_1970() {
        let ts = timestamp_from_unix_seconds(0);
        assert_eq!(
            ts,
            Timestamp {
                year: 1970,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0
            }
        );
    }

    #[test]
    fn leap_day_2024() {
        // 1709209496 == 2024-02-29 12:24:56 UTC
        let ts = timestamp_from_unix_seconds(1709209496);
        assert_eq!(
            ts,
            Timestamp {
                year: 2024,
                month: 2,
                day: 29,
                hour: 12,
                minute: 24
            }
        );
    }

    #[test]
    fn negative_seconds_clamp_to_epoch() {
        let ts = timestamp_from_unix_seconds(-100);
        assert_eq!(ts.year, 1970);
        assert_eq!(ts.month, 1);
    }

    #[test]
    fn file_stem_is_zero_padded() {
        let ts = Timestamp {
            year: 2026,
            month: 5,
            day: 31,
            hour: 14,
            minute: 30,
        };
        assert_eq!(format_file_stem(ts), "2026-05-31_14-30");
        let ts = Timestamp {
            year: 2026,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
        };
        assert_eq!(format_file_stem(ts), "2026-01-02_03-04");
    }

    #[test]
    fn xdg_wins_when_set() {
        let p = resolve_storage_dir(Some("/custom/data"), None).unwrap();
        assert_eq!(p, PathBuf::from("/custom/data/caleb"));
    }

    #[test]
    fn falls_back_to_home_local_share() {
        let p = resolve_storage_dir(None, Some("/home/test")).unwrap();
        assert_eq!(p, PathBuf::from("/home/test/.local/share/caleb"));
    }

    #[test]
    fn empty_xdg_is_treated_as_unset() {
        let p = resolve_storage_dir(Some(""), Some("/home/test")).unwrap();
        assert_eq!(p, PathBuf::from("/home/test/.local/share/caleb"));
    }

    #[test]
    fn errors_when_nothing_is_set() {
        assert_eq!(
            resolve_storage_dir(None, None),
            Err(ResolveError::NoStorageDir)
        );
    }

    #[test]
    fn unique_filename_uses_base_name_in_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let got = unique_filename(dir.path(), "2026-05-31_14-30", ".md").unwrap();
        assert_eq!(got, "2026-05-31_14-30.md");
    }

    #[test]
    fn unique_filename_appends_suffixes_on_collision() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("stem.md"), "").unwrap();
        assert_eq!(
            unique_filename(dir.path(), "stem", ".md").unwrap(),
            "stem-2.md"
        );

        std::fs::write(dir.path().join("stem-2.md"), "").unwrap();
        assert_eq!(
            unique_filename(dir.path(), "stem", ".md").unwrap(),
            "stem-3.md"
        );
    }

    #[test]
    fn timestamp_now_is_plausible() {
        // Not asserting an exact value — just that the local clock produces
        // a sane, in-range timestamp rather than a default or a panic.
        let ts = timestamp_now();
        assert!(ts.year >= 2026);
        assert!((1..=12).contains(&ts.month));
        assert!((1..=31).contains(&ts.day));
        assert!(ts.hour <= 23);
        assert!(ts.minute <= 59);
    }
}
