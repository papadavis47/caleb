//! An open session and its persistence: load, save, and resume.
//!
//! The plain data types live in [`crate::model`]; this module is what turns
//! them into files on disk and back.
//!
//! Rust note: `String` and `Vec<Task>` own their memory, and `Drop` frees it
//! when a `Session` goes out of scope — so there is nothing to free by hand
//! and no allocator to thread through the API.

use crate::markdown;
use crate::model::{MAX_TASK_BYTES, Pane, Task, Timestamp, truncate_on_char_boundary};
use crate::storage;
use std::path::Path;
use thiserror::Error;

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

/// Rust note: `#[from]` generates the `From` impls that make `?` convert an
/// `io::Error` or a `ParseError` into a `LoadError` automatically, so callers
/// see one typed error instead of two.
#[derive(Debug, Error)]
pub enum LoadError {
    #[error("cannot read session file: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot parse session file: {0}")]
    Parse(#[from] markdown::ParseError),
}

/// Failure to write a session back to disk.
#[derive(Debug, Error)]
pub enum SaveError {
    #[error("cannot write session file: {0}")]
    Io(#[from] std::io::Error),
}

/// Failure to resume a previously saved session. Each variant names the file
/// it was working on, because by the time this surfaces the user has already
/// forgotten which one they picked.
#[derive(Debug, Error)]
pub enum ResumeError {
    #[error("cannot pick a filename for the resumed session: {source}")]
    Pick {
        #[source]
        source: std::io::Error,
    },
    #[error("cannot rename '{from}' to '{to}': {source}")]
    Rename {
        from: String,
        to: String,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot load session '{name}': {source}")]
    Load {
        name: String,
        #[source]
        source: LoadError,
    },
    #[error("cannot rewrite the header of '{name}': {source}")]
    Rewrite {
        name: String,
        #[source]
        source: SaveError,
    },
}

/// Failure to pull tasks out of a past session. `SaveSource` is the one that
/// reports partial success: the tasks are already in the current session and
/// on disk by then, so it warns rather than pretending nothing happened.
#[derive(Debug, Error)]
pub enum PullError {
    #[error("cannot load session '{name}': {source}")]
    LoadSource {
        name: String,
        #[source]
        source: LoadError,
    },
    #[error("cannot save the current session: {source}")]
    SaveTarget {
        #[source]
        source: SaveError,
    },
    #[error("tasks were pulled, but '{name}' still shows them as open: {source}")]
    SaveSource {
        name: String,
        #[source]
        source: SaveError,
    },
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

/// Rename the picked file to `ts` and load it, so resumed work continues under
/// a fresh timestamp. The old name goes away.
///
/// When the new stem collides with the original — resuming within the same
/// minute you saved — the rename is skipped rather than attempted against
/// itself.
pub fn resume(dir: &Path, original: &str, ts: Timestamp) -> Result<Session, ResumeError> {
    let stem = storage::format_file_stem(ts);
    let new_name = storage::unique_filename(dir, &stem, storage::FILE_EXTENSION)
        .map_err(|source| ResumeError::Pick { source })?;

    if original != new_name {
        std::fs::rename(dir.join(original), dir.join(&new_name)).map_err(|source| {
            ResumeError::Rename {
                from: original.to_string(),
                to: new_name.clone(),
                source,
            }
        })?;
    }

    let mut loaded = Session::load(dir, &new_name).map_err(|source| ResumeError::Load {
        name: new_name.clone(),
        source,
    })?;

    // Write the new timestamp back out immediately rather than waiting for the
    // next edit to mark the session dirty. The rename already touched the
    // filesystem; leaving the `# Session` header disagreeing with the filename
    // until the user happens to change something is the worse of the two
    // states. `save` clears `dirty`, so the header does not read as unsaved.
    loaded.timestamp = Some(ts);
    loaded.save(dir).map_err(|source| ResumeError::Rewrite {
        name: new_name,
        source,
    })?;

    Ok(loaded)
}

/// Move the tasks named by `tasks` out of `source.active` and into
/// `target.active`, leaving a completed copy behind in `source.completed`.
/// Returns how many actually moved.
///
/// Each entry is `(index, text)` rather than a bare index because the index
/// was computed against a snapshot the picker took at `p`-press time —
/// position alone is not proof the task still sitting at that position is the
/// one the user chose. If the source file changed on disk in the meantime (a
/// second `caleb` instance, a hand edit while the picker was on screen),
/// `source.active[index]` may hold different text by the time this runs; an
/// entry whose text no longer matches is skipped, exactly like an
/// out-of-range or already-`done` one, rather than moving whatever now sits
/// there.
///
/// `tasks` may arrive unsorted, duplicated, or out of range; the caller is a
/// picker, not a proof. An entry naming a task that is already `done` is
/// ignored too — `markdown::parse` files tasks by the heading above them, not
/// by their checkbox, so `active` can hold a hand-written `- [x]`.
///
/// Rust note: the removals walk the indices in *descending* order, because
/// `Vec::remove` shifts everything after the hole down by one — taking index 0
/// first would leave every later index pointing at the wrong task. The
/// collected tasks are then replayed in ascending order so the target reads in
/// the source's original order.
pub fn pull_tasks(source: &mut Session, target: &mut Session, tasks: &[(usize, String)]) -> usize {
    let mut wanted: Vec<usize> = tasks
        .iter()
        .filter(|(i, text)| {
            source
                .active
                .get(*i)
                .is_some_and(|t| !t.done && &t.text == text)
        })
        .map(|(i, _)| *i)
        .collect();
    wanted.sort_unstable();
    wanted.dedup();

    let taken: Vec<Task> = wanted
        .iter()
        .rev()
        .map(|&i| source.active.remove(i))
        .collect();

    let moved = taken.len();
    for task in taken.into_iter().rev() {
        target.active.push(Task {
            text: task.text.clone(),
            done: false,
        });
        source.completed.push(Task {
            text: task.text,
            done: true,
        });
    }

    if moved > 0 {
        source.dirty = true;
        target.dirty = true;
    }
    moved
}

/// Load `source_name`, move the named tasks into `target`, and write both
/// files.
///
/// The write order is the whole point. The target is saved *first*, so a
/// failure between the two writes leaves the tasks open in both files —
/// visible, and fixable by hand. Saving the source first and then failing
/// would check them off in one file without ever recording them in the other,
/// which loses them outright. For the same reason a `SaveSource` failure is
/// reported rather than rolled back: undoing it can only lose more.
pub fn pull_from_file(
    dir: &Path,
    source_name: &str,
    target: &mut Session,
    tasks: &[(usize, String)],
) -> Result<usize, PullError> {
    let mut loaded = Session::load(dir, source_name).map_err(|e| PullError::LoadSource {
        name: source_name.to_string(),
        source: e,
    })?;

    let moved = pull_tasks(&mut loaded, target, tasks);
    if moved == 0 {
        return Ok(0);
    }

    target
        .save(dir)
        .map_err(|e| PullError::SaveTarget { source: e })?;
    loaded.save(dir).map_err(|e| PullError::SaveSource {
        name: source_name.to_string(),
        source: e,
    })?;

    Ok(moved)
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
    /// Rust note: `Vec::push` aborts on OOM rather than returning an error,
    /// so there is no allocation-failure branch to roll back — this function
    /// cannot fail and needs no `Result`.
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
    use crate::test_util::empty_session;

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

    fn ts_at(hour: u8, minute: u8) -> Timestamp {
        Timestamp {
            year: 2026,
            month: 5,
            day: 31,
            hour,
            minute,
        }
    }

    #[test]
    fn resume_renames_to_the_new_timestamp_and_drops_the_old_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut orig = create_new(dir.path(), ts_at(14, 30)).unwrap();
        orig.add(Pane::Active, "carry me over");
        orig.save(dir.path()).unwrap();

        let now = ts_at(16, 45);
        let resumed = resume(dir.path(), &orig.filename, now).unwrap();

        assert_eq!(resumed.filename, "2026-05-31_16-45.md");
        assert_eq!(resumed.timestamp, Some(now));
        assert_eq!(resumed.active[0].text, "carry me over");
        assert!(!resumed.dirty);
        assert!(!dir.path().join(&orig.filename).exists());
        assert!(dir.path().join(&resumed.filename).exists());
    }

    #[test]
    fn resume_within_the_same_minute_gets_a_collision_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let ts = ts_at(14, 30);
        let mut orig = create_new(dir.path(), ts).unwrap();
        orig.add(Pane::Active, "same minute");
        orig.save(dir.path()).unwrap();

        // The saved file already occupies the stem, so unique_filename walks
        // to the -2 suffix rather than handing back a name that would be
        // renamed onto itself.
        let resumed = resume(dir.path(), &orig.filename, ts).unwrap();
        assert_eq!(resumed.filename, "2026-05-31_14-30-2.md");
        assert_eq!(resumed.active[0].text, "same minute");
        assert!(!dir.path().join(&orig.filename).exists());
    }

    #[test]
    fn resume_of_a_never_saved_session_skips_the_rename() {
        let dir = tempfile::tempdir().unwrap();
        let ts = ts_at(14, 30);
        // Nothing on disk, so unique_filename returns the bare stem, which
        // equals `original` — the guard must skip renaming a file onto itself
        // (and here the file does not exist at all).
        let err = resume(dir.path(), "2026-05-31_14-30.md", ts).unwrap_err();
        assert!(matches!(err, ResumeError::Load { .. }));
    }

    #[test]
    fn resume_rewrites_the_header_to_match_the_new_filename() {
        let dir = tempfile::tempdir().unwrap();
        let mut orig = create_new(dir.path(), ts_at(14, 30)).unwrap();
        orig.add(Pane::Active, "carry me over");
        orig.save(dir.path()).unwrap();

        let resumed = resume(dir.path(), &orig.filename, ts_at(16, 45)).unwrap();

        // The header on disk must agree with the name on disk, without the
        // user having to make an edit first.
        let on_disk = std::fs::read_to_string(dir.path().join(&resumed.filename)).unwrap();
        assert!(
            on_disk.starts_with("# Session 2026-05-31 16:45\n"),
            "header should carry the resumed timestamp, got: {on_disk:?}"
        );
        assert!(on_disk.contains("- [ ] carry me over"));
        assert!(!resumed.dirty, "the rewrite leaves nothing unsaved");
    }

    #[test]
    fn resume_reports_the_missing_name_when_the_file_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let err = resume(dir.path(), "vanished.md", ts_at(9, 0)).unwrap_err();
        assert!(matches!(err, ResumeError::Rename { .. }));
        assert!(err.to_string().contains("vanished.md"));
    }

    #[test]
    fn pull_tasks_moves_the_named_task_and_completes_it_in_the_source() {
        let mut source = crate::test_util::session_with(&["keep me", "take me"]);
        let mut target = crate::test_util::session_with(&["already here"]);

        let moved = pull_tasks(&mut source, &mut target, &[(1, "take me".to_string())]);

        assert_eq!(moved, 1);
        assert_eq!(source.active.len(), 1);
        assert_eq!(source.active[0].text, "keep me");
        assert_eq!(source.completed.len(), 1);
        assert_eq!(source.completed[0].text, "take me");
        assert!(source.completed[0].done, "the source's copy reads as done");
        assert_eq!(target.active.len(), 2);
        assert_eq!(target.active[1].text, "take me");
        assert!(!target.active[1].done, "the target's copy is open work");
        assert!(source.dirty && target.dirty);
    }

    #[test]
    fn pull_tasks_keeps_source_order_and_survives_unsorted_duplicate_indices() {
        // Removing low indices first would shift the ones still to come, so
        // the walk goes descending; the target must still read in file order.
        let mut source = crate::test_util::session_with(&["zero", "one", "two", "three"]);
        let mut target = crate::test_util::session_with(&[]);

        let moved = pull_tasks(
            &mut source,
            &mut target,
            &[
                (3, "three".to_string()),
                (0, "zero".to_string()),
                (3, "three".to_string()),
            ],
        );

        assert_eq!(moved, 2, "the duplicate index counts once");
        let text: Vec<&str> = target.active.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(text, ["zero", "three"]);
        let left: Vec<&str> = source.active.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(left, ["one", "two"]);
        let done: Vec<&str> = source.completed.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(done, ["zero", "three"]);
    }

    #[test]
    fn pull_tasks_ignores_out_of_range_indices() {
        let mut source = crate::test_util::session_with(&["only"]);
        let mut target = crate::test_util::session_with(&[]);

        assert_eq!(
            pull_tasks(&mut source, &mut target, &[(9, "whatever".to_string())]),
            0
        );
        assert_eq!(source.active.len(), 1);
        assert!(target.active.is_empty());
        assert!(!source.dirty, "a no-op must not mark anything unsaved");
        assert!(!target.dirty);
    }

    #[test]
    fn pull_tasks_with_no_indices_is_a_noop() {
        let mut source = crate::test_util::session_with(&["only"]);
        let mut target = crate::test_util::session_with(&[]);

        assert_eq!(pull_tasks(&mut source, &mut target, &[]), 0);
        assert!(!source.dirty);
        assert!(!target.dirty);
    }

    #[test]
    fn pull_tasks_skips_a_task_that_is_already_done() {
        // `markdown::parse` files tasks by heading, not by checkbox, so a
        // hand-written `- [x]` under `## Active` lands in `active`. Pulling it
        // would resurrect finished work as open.
        let mut source = crate::test_util::session_with(&["open one"]);
        source
            .active
            .push(crate::test_util::task("secretly done", true));
        source.dirty = false;
        let mut target = crate::test_util::session_with(&[]);

        assert_eq!(
            pull_tasks(
                &mut source,
                &mut target,
                &[
                    (0, "open one".to_string()),
                    (1, "secretly done".to_string())
                ],
            ),
            1
        );
        assert_eq!(target.active.len(), 1);
        assert_eq!(target.active[0].text, "open one");
        assert_eq!(source.active.len(), 1, "the done one stays put");
        assert_eq!(source.active[0].text, "secretly done");
    }

    #[test]
    fn pull_tasks_skips_a_stale_index_whose_text_no_longer_matches() {
        // The index was computed against a snapshot taken when `p` was
        // pressed. If the file changed underneath before the reload — another
        // `caleb` instance, or a hand edit — position 0 might no longer hold
        // the task the user picked. Position alone must not be trusted.
        let mut source = crate::test_util::session_with(&["changed underneath", "still here"]);
        let mut target = crate::test_util::session_with(&[]);

        let moved = pull_tasks(
            &mut source,
            &mut target,
            &[(0, "stale text".to_string()), (1, "still here".to_string())],
        );

        assert_eq!(moved, 1, "only the entry whose text still matches moves");
        assert_eq!(source.active.len(), 1, "the stale one is left in place");
        assert_eq!(source.active[0].text, "changed underneath");
        assert_eq!(target.active.len(), 1);
        assert_eq!(target.active[0].text, "still here");
    }

    #[test]
    fn pull_from_file_updates_both_files_on_disk() {
        let dir = tempfile::tempdir().unwrap();

        let mut old = create_new(dir.path(), ts_at(14, 30)).unwrap();
        old.add(Pane::Active, "carry me");
        old.add(Pane::Active, "leave me");
        old.save(dir.path()).unwrap();

        let mut current = create_new(dir.path(), ts_at(16, 45)).unwrap();
        let moved = pull_from_file(
            dir.path(),
            &old.filename,
            &mut current,
            &[(0, "carry me".to_string())],
        )
        .unwrap();
        assert_eq!(moved, 1);

        let source = Session::load(dir.path(), &old.filename).unwrap();
        assert_eq!(source.active.len(), 1);
        assert_eq!(source.active[0].text, "leave me");
        assert_eq!(source.completed[0].text, "carry me");
        assert!(source.completed[0].done);

        let target = Session::load(dir.path(), &current.filename).unwrap();
        assert_eq!(target.active[0].text, "carry me");
        assert!(!target.active[0].done);
        assert!(
            !current.dirty,
            "both files were written, nothing is pending"
        );
    }

    #[test]
    fn a_fully_drained_source_reaches_zero_open() {
        // This is what makes `--clean` able to sweep it afterwards.
        let dir = tempfile::tempdir().unwrap();
        let mut old = create_new(dir.path(), ts_at(14, 30)).unwrap();
        old.add(Pane::Active, "one");
        old.add(Pane::Active, "two");
        old.save(dir.path()).unwrap();

        let mut current = create_new(dir.path(), ts_at(16, 45)).unwrap();
        pull_from_file(
            dir.path(),
            &old.filename,
            &mut current,
            &[(0, "one".to_string()), (1, "two".to_string())],
        )
        .unwrap();

        let on_disk = std::fs::read_to_string(dir.path().join(&old.filename)).unwrap();
        assert_eq!(crate::markdown::count_tasks(&on_disk).open, 0, "{on_disk}");
    }

    #[test]
    fn pull_from_file_creates_a_target_that_was_never_saved() {
        let dir = tempfile::tempdir().unwrap();
        let mut old = create_new(dir.path(), ts_at(14, 30)).unwrap();
        old.add(Pane::Active, "carry me");
        old.save(dir.path()).unwrap();

        let mut current = create_new(dir.path(), ts_at(16, 45)).unwrap();
        assert!(!dir.path().join(&current.filename).exists());

        pull_from_file(
            dir.path(),
            &old.filename,
            &mut current,
            &[(0, "carry me".to_string())],
        )
        .unwrap();
        assert!(dir.path().join(&current.filename).exists());
    }

    #[test]
    fn pull_from_file_names_a_missing_source() {
        let dir = tempfile::tempdir().unwrap();
        let mut current = create_new(dir.path(), ts_at(16, 45)).unwrap();

        let err = pull_from_file(
            dir.path(),
            "vanished.md",
            &mut current,
            &[(0, "whatever".to_string())],
        )
        .unwrap_err();
        assert!(matches!(err, PullError::LoadSource { .. }));
        assert!(err.to_string().contains("vanished.md"));
    }

    #[test]
    fn pull_from_file_propagates_an_unparseable_source() {
        let dir = tempfile::tempdir().unwrap();
        let bad = format!("- [ ] {}\n", "x".repeat(MAX_TASK_BYTES + 1));
        std::fs::write(dir.path().join("bad.md"), bad).unwrap();
        let mut current = create_new(dir.path(), ts_at(16, 45)).unwrap();

        assert!(matches!(
            pull_from_file(
                dir.path(),
                "bad.md",
                &mut current,
                &[(0, "whatever".to_string())],
            ),
            Err(PullError::LoadSource { .. })
        ));
    }

    #[test]
    fn pulling_nothing_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut old = create_new(dir.path(), ts_at(14, 30)).unwrap();
        old.add(Pane::Active, "stays");
        old.save(dir.path()).unwrap();

        let mut current = create_new(dir.path(), ts_at(16, 45)).unwrap();
        assert_eq!(
            pull_from_file(dir.path(), &old.filename, &mut current, &[]).unwrap(),
            0
        );
        assert!(
            !dir.path().join(&current.filename).exists(),
            "an empty pull must not create the target file"
        );
    }
}
