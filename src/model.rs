//! Plain data types for a coding session — no I/O, no terminal, no
//! dependencies on the rest of the crate.
//!
//! Keeping these here is what lets `markdown` and `storage` stay leaf modules:
//! before this split, both had to reach back into `session` for `Timestamp`,
//! which made the dependency graph cyclic.

use std::fmt;

/// Tasks are short notes, capped so rendering never needs to wrap.
pub const MAX_TASK_BYTES: usize = 150;

/// A single task. `text` is owned outright — no lifetime, no manual free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub text: String,
    pub done: bool,
}

/// Wall-clock time at minute granularity — all we need for filenames and
/// the markdown header.
///
/// Rust note: an absent timestamp is `Option<Timestamp>`, never a sentinel
/// value. The compiler forces every read to handle the `None` case, so a
/// missing header cannot silently become a zero date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

impl fmt::Display for Timestamp {
    /// The header format used inside session files and in the app header:
    /// `2026-05-31 14:30`.
    ///
    /// Note this is deliberately *not* the filename format — see
    /// [`crate::storage::format_file_stem`], which separates date from time
    /// with an underscore so the name stays shell-friendly.
    ///
    /// ```
    /// # use caleb::model::Timestamp;
    /// let ts = Timestamp { year: 2026, month: 5, day: 31, hour: 14, minute: 30 };
    /// assert_eq!(ts.to_string(), "2026-05-31 14:30");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }
}

/// Which pane has focus. Movement and toggling apply to whichever this names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Active,
    Completed,
}

impl Pane {
    /// The opposite pane. Toggling moves a task from one to the other.
    #[must_use]
    pub fn other(self) -> Self {
        match self {
            Pane::Active => Pane::Completed,
            Pane::Completed => Pane::Active,
        }
    }
}

/// Longest prefix of `s` that fits in `max` bytes without splitting a
/// character.
///
/// Rust note: slicing at exactly `max` bytes would cut a multi-byte UTF-8
/// character in half. Rust's `&str` is guaranteed valid UTF-8, so that slice
/// panics rather than printing mojibake — the type system forces us to walk
/// back to a boundary.
///
/// ```
/// # use caleb::model::truncate_on_char_boundary;
/// assert_eq!(truncate_on_char_boundary("hello", 10), "hello");
/// // 'é' is two bytes, so a three-byte budget drops it rather than splitting.
/// assert_eq!(truncate_on_char_boundary("abé", 3), "ab");
/// ```
pub fn truncate_on_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_other_flips() {
        assert_eq!(Pane::Active.other(), Pane::Completed);
        assert_eq!(Pane::Completed.other(), Pane::Active);
    }

    #[test]
    fn timestamp_displays_in_header_format() {
        let ts = Timestamp {
            year: 2026,
            month: 5,
            day: 31,
            hour: 14,
            minute: 30,
        };
        assert_eq!(ts.to_string(), "2026-05-31 14:30");
    }

    #[test]
    fn timestamp_display_zero_pads() {
        let ts = Timestamp {
            year: 999,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
        };
        assert_eq!(ts.to_string(), "0999-01-02 03:04");
    }

    #[test]
    fn truncate_leaves_short_strings_alone() {
        assert_eq!(truncate_on_char_boundary("abc", 10), "abc");
        assert_eq!(truncate_on_char_boundary("abc", 3), "abc");
    }

    #[test]
    fn truncate_never_splits_a_multibyte_char() {
        // 'é' is 2 bytes, so a cap of 2 must drop it entirely rather than
        // slicing it in half.
        assert_eq!(truncate_on_char_boundary("aé", 2), "a");
        assert_eq!(truncate_on_char_boundary("aé", 3), "aé");
        assert_eq!(truncate_on_char_boundary("é", 1), "");
    }
}
