//! Domain types for a coding session.
//!
//! Rust note: ava's Zig `Session` carried an `allocator` field and every
//! `Task` needed an explicit `deinit`. Here `String` and `Vec<Task>` own
//! their memory, and `Drop` frees it when a `Session` goes out of scope —
//! so there is nothing to free by hand and no allocator to thread through.

use crate::markdown;
use crate::storage;
use std::path::Path;
use thiserror::Error;

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
/// Rust note: `Option<Timestamp>` replaces Zig's `?Timestamp`. The compiler
/// forces every read to handle the `None` case, so a missing header cannot
/// silently become a zero date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

/// Which pane has focus. Movement and toggling apply to whichever this names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Active,
    Completed,
}

impl Pane {
    /// The opposite pane. Toggling moves a task from one to the other.
    pub fn other(self) -> Self {
        match self {
            Pane::Active => Pane::Completed,
            Pane::Completed => Pane::Active,
        }
    }
}

/// All in-memory state for an open session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Basename, not a full path. Reassigned when a session is resumed.
    pub filename: String,
    pub timestamp: Option<Timestamp>,
    pub active: Vec<Task>,
    pub completed: Vec<Task>,
    /// True when in-memory state diverges from what is on disk.
    pub dirty: bool,
}

/// Longest prefix of `s` that fits in `max` bytes without splitting a
/// character.
///
/// Rust note: ava sliced at exactly 150 bytes, which can cut a multi-byte
/// UTF-8 character in half and print mojibake. Rust's `&str` is guaranteed
/// valid UTF-8, so slicing mid-character would panic instead — the type
/// system forces us to get this right.
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

/// Rust note: `#[from]` generates the `From` impls that make `?` convert an
/// `io::Error` or a `ParseError` into a `LoadError` automatically. This is
/// the Rust counterpart to Zig's merged error sets (`A || B`).
#[derive(Debug, Error)]
pub enum LoadError {
    #[error("cannot read session file: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot parse session file: {0}")]
    Parse(#[from] markdown::ParseError),
}

#[derive(Debug, Error)]
pub enum SaveError {
    #[error("cannot write session file: {0}")]
    Io(#[from] std::io::Error),
}

/// Build an empty session with a unique filename in `dir`. Nothing is
/// written until the first `save`.
pub fn create_new(dir: &Path, ts: Timestamp) -> std::io::Result<Session> {
    let stem = storage::format_file_stem(ts);
    let filename = storage::unique_filename(dir, &stem, storage::FILE_EXTENSION)?;
    Ok(Session {
        filename,
        timestamp: Some(ts),
        active: Vec::new(),
        completed: Vec::new(),
        dirty: false,
    })
}

impl Session {
    pub fn tasks(&self, pane: Pane) -> &[Task] {
        match pane {
            Pane::Active => &self.active,
            Pane::Completed => &self.completed,
        }
    }

    pub fn tasks_mut(&mut self, pane: Pane) -> &mut Vec<Task> {
        match pane {
            Pane::Active => &mut self.active,
            Pane::Completed => &mut self.completed,
        }
    }

    /// Append a task to `pane`. Empty text is ignored; long text is capped.
    pub fn add(&mut self, pane: Pane, text: &str) {
        if text.is_empty() {
            return;
        }
        let text = truncate_on_char_boundary(text, MAX_TASK_BYTES).to_string();
        let done = pane == Pane::Completed;
        self.tasks_mut(pane).push(Task { text, done });
        self.dirty = true;
    }

    /// Remove the task at `index`. Out of range is a no-op.
    pub fn delete(&mut self, pane: Pane, index: usize) {
        let list = self.tasks_mut(pane);
        if index >= list.len() {
            return;
        }
        list.remove(index);
        self.dirty = true;
    }

    /// Move the task at `index` to the end of the other pane, flipping
    /// `done`.
    ///
    /// Rust note: ava's version returns an error and hand-rolls a rollback,
    /// because Zig's `append` can fail on allocation. `Vec::push` aborts on
    /// OOM rather than returning, so that entire failure branch disappears —
    /// this function cannot fail.
    pub fn toggle(&mut self, from: Pane, index: usize) {
        let src = self.tasks_mut(from);
        if index >= src.len() {
            return;
        }
        let mut task = src.remove(index);
        task.done = !task.done;
        self.tasks_mut(from.other()).push(task);
        self.dirty = true;
    }

    /// Swap two slots in the same pane. No-op if either index is out of
    /// range or they are equal.
    pub fn swap(&mut self, pane: Pane, a: usize, b: usize) {
        if a == b {
            return;
        }
        let list = self.tasks_mut(pane);
        if a >= list.len() || b >= list.len() {
            return;
        }
        list.swap(a, b);
        self.dirty = true;
    }

    /// Read and parse `filename` from `dir`.
    pub fn load(dir: &Path, filename: &str) -> Result<Session, LoadError> {
        let contents = std::fs::read_to_string(dir.join(filename))?;
        let parsed = markdown::parse(&contents)?;
        Ok(Session {
            filename: filename.to_string(),
            timestamp: parsed.timestamp,
            active: parsed.active,
            completed: parsed.completed,
            dirty: false,
        })
    }

    /// Serialize and overwrite the on-disk file, clearing `dirty`.
    pub fn save(&mut self, dir: &Path) -> Result<(), SaveError> {
        let data = markdown::serialize(self.timestamp, &self.active, &self.completed);
        std::fs::write(dir.join(&self.filename), data)?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_session() -> Session {
        Session {
            filename: "x.md".to_string(),
            timestamp: None,
            active: Vec::new(),
            completed: Vec::new(),
            dirty: false,
        }
    }

    #[test]
    fn add_then_delete_restores_count() {
        let mut s = empty_session();
        s.add(Pane::Active, "first");
        s.add(Pane::Active, "second");
        assert_eq!(s.active.len(), 2);
        assert!(s.dirty);

        s.delete(Pane::Active, 0);
        assert_eq!(s.active.len(), 1);
        assert_eq!(s.active[0].text, "second");
    }

    #[test]
    fn add_ignores_empty_text() {
        let mut s = empty_session();
        s.add(Pane::Active, "");
        assert!(s.active.is_empty());
        assert!(!s.dirty);
    }

    #[test]
    fn toggle_moves_task_between_panes_and_flips_done() {
        let mut s = empty_session();
        s.add(Pane::Active, "thing");
        s.dirty = false;

        s.toggle(Pane::Active, 0);
        assert!(s.active.is_empty());
        assert_eq!(s.completed.len(), 1);
        assert_eq!(s.completed[0].text, "thing");
        assert!(s.completed[0].done);
        assert!(s.dirty);

        s.toggle(Pane::Completed, 0);
        assert_eq!(s.active.len(), 1);
        assert!(s.completed.is_empty());
        assert!(!s.active[0].done);
    }

    #[test]
    fn toggle_out_of_range_is_a_noop() {
        let mut s = empty_session();
        s.toggle(Pane::Active, 5);
        assert!(s.active.is_empty());
        assert!(!s.dirty);
    }

    #[test]
    fn add_caps_text_at_max_bytes() {
        let mut s = empty_session();
        s.add(Pane::Active, &"x".repeat(200));
        assert_eq!(s.active[0].text.len(), MAX_TASK_BYTES);
    }

    #[test]
    fn add_truncates_on_char_boundary_not_mid_sequence() {
        let mut s = empty_session();
        // 149 ASCII bytes then a 2-byte char: cutting at 150 would split it.
        let text = format!("{}é", "x".repeat(MAX_TASK_BYTES - 1));
        s.add(Pane::Active, &text);
        assert_eq!(s.active[0].text.len(), MAX_TASK_BYTES - 1);
        assert!(s.active[0].text.is_char_boundary(s.active[0].text.len()));
    }

    #[test]
    fn swap_exchanges_two_slots() {
        let mut s = empty_session();
        s.add(Pane::Active, "first");
        s.add(Pane::Active, "second");
        s.swap(Pane::Active, 0, 1);
        assert_eq!(s.active[0].text, "second");
        assert_eq!(s.active[1].text, "first");
    }

    #[test]
    fn swap_out_of_range_or_equal_is_a_noop() {
        let mut s = empty_session();
        s.add(Pane::Active, "only");
        s.dirty = false;
        s.swap(Pane::Active, 0, 0);
        s.swap(Pane::Active, 0, 9);
        assert!(!s.dirty);
    }

    #[test]
    fn pane_other_flips() {
        assert_eq!(Pane::Active.other(), Pane::Completed);
        assert_eq!(Pane::Completed.other(), Pane::Active);
    }

    #[test]
    fn create_new_picks_a_name_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let ts = Timestamp {
            year: 2026,
            month: 5,
            day: 31,
            hour: 14,
            minute: 30,
        };
        let s = create_new(dir.path(), ts).unwrap();
        assert_eq!(s.filename, "2026-05-31_14-30.md");
        assert!(!s.dirty);
        // Nothing on disk until the first save.
        assert!(!dir.path().join(&s.filename).exists());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let ts = Timestamp {
            year: 2026,
            month: 5,
            day: 31,
            hour: 14,
            minute: 30,
        };
        let mut orig = create_new(dir.path(), ts).unwrap();
        orig.add(Pane::Active, "first");
        orig.add(Pane::Active, "second");
        orig.add(Pane::Completed, "done");
        orig.save(dir.path()).unwrap();
        assert!(!orig.dirty);

        let loaded = Session::load(dir.path(), &orig.filename).unwrap();
        assert_eq!(loaded.active.len(), 2);
        assert_eq!(loaded.completed.len(), 1);
        assert_eq!(loaded.active[0].text, "first");
        assert_eq!(loaded.completed[0].text, "done");
        assert_eq!(loaded.timestamp.unwrap().year, 2026);
        assert!(!loaded.dirty);
    }

    #[test]
    fn load_propagates_parse_errors() {
        let dir = tempfile::tempdir().unwrap();
        let bad = format!("- [ ] {}\n", "x".repeat(MAX_TASK_BYTES + 1));
        std::fs::write(dir.path().join("bad.md"), bad).unwrap();
        assert!(matches!(
            Session::load(dir.path(), "bad.md"),
            Err(LoadError::Parse(_))
        ));
    }

    #[test]
    fn load_missing_file_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            Session::load(dir.path(), "nope.md"),
            Err(LoadError::Io(_))
        ));
    }
}
