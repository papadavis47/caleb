# Pull Open Tasks Forward — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `p` to caleb's session screen: a two-stage picker that moves selected open tasks out of a past session into the current one, checking them off in the source.

**Architecture:** The move itself is a pure function over two in-memory `Session` values in `session.rs`, wrapped by one I/O function that saves the target before the source. The two screens live in a new `pull.rs` with a pure `PullState::on_key` state machine plus a thin draw/read loop, mirroring `picker.rs`. `app.rs` stays terminal-free by returning an `Action` from `handle_key`; `App::run`, which already holds the `&mut Tui`, is what calls into `pull::run`.

**Tech Stack:** Rust edition 2024 / rust-version 1.90, ratatui 0.30.2 (`TestBackend` for draw tests), crossterm 0.29, thiserror 2.0, tempfile 3.27 (dev), Python 3 + `pty` for the smoke script.

**Spec:** `docs/superpowers/specs/2026-08-23-pull-tasks-forward-design.md`

## Global Constraints

- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` must pass before every commit. Clippy runs at `pedantic`.
- `unsafe_code = "forbid"`. No new dependencies.
- Every module keeps its unit tests in an in-file `#[cfg(test)] mod tests`. Integration tests under `tests/` use only the public API and cannot see `src/test_util.rs`.
- Layering, per `src/lib.rs`: `model` is plain data; `markdown`/`storage` are pure leaves; `session` composes them into load and save; `ui`/`app`/`picker`/`pull`/`tui` are the terminal edge. **Session mutation never moves into a terminal-edge module.**
- `app.rs` must not touch the terminal — that is what makes every binding unit-testable.
- Task text cap is `MAX_TASK_BYTES = 150`. Tasks read from a file are already within it; no re-truncation on pull.
- Doc comments explain *why*, matching the density of the surrounding code. Rust-teaching notes ("Rust note:") appear where a construct is non-obvious — this repo is also a learning vehicle.
- "Pullable" means `markdown::parse(contents).active` filtered to `!done` — **not** `Entry::open`, which counts `- [ ]` lines regardless of heading.

---

### Task 1: `session::pull_tasks` — the pure move

**Files:**
- Modify: `src/session.rs` (add the function after `resume`, tests in the existing `mod tests`)

**Interfaces:**
- Consumes: `Session`, `Task` (existing), `crate::test_util::session_with` (existing, test-only).
- Produces: `pub fn pull_tasks(source: &mut Session, target: &mut Session, indices: &[usize]) -> usize`

- [ ] **Step 1: Write the failing tests**

Append to `src/session.rs`'s `mod tests`:

```rust
    #[test]
    fn pull_tasks_moves_the_named_task_and_completes_it_in_the_source() {
        let mut source = crate::test_util::session_with(&["keep me", "take me"]);
        let mut target = crate::test_util::session_with(&["already here"]);

        let moved = pull_tasks(&mut source, &mut target, &[1]);

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

        let moved = pull_tasks(&mut source, &mut target, &[3, 0, 3]);

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

        assert_eq!(pull_tasks(&mut source, &mut target, &[9]), 0);
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
        source.active.push(crate::test_util::task("secretly done", true));
        source.dirty = false;
        let mut target = crate::test_util::session_with(&[]);

        assert_eq!(pull_tasks(&mut source, &mut target, &[0, 1]), 1);
        assert_eq!(target.active.len(), 1);
        assert_eq!(target.active[0].text, "open one");
        assert_eq!(source.active.len(), 1, "the done one stays put");
        assert_eq!(source.active[0].text, "secretly done");
    }
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test --lib session::tests::pull_tasks`
Expected: FAIL — `cannot find function 'pull_tasks' in this scope`.

- [ ] **Step 3: Write the implementation**

Insert in `src/session.rs`, after `resume` and before `impl Session`:

```rust
/// Move the tasks at `indices` out of `source.active` and into
/// `target.active`, leaving a completed copy behind in `source.completed`.
/// Returns how many actually moved.
///
/// `indices` may arrive unsorted, duplicated, or out of range; the caller is
/// a picker, not a proof. An index naming a task that is already `done` is
/// ignored too — `markdown::parse` files tasks by the heading above them, not
/// by their checkbox, so `active` can hold a hand-written `- [x]`.
///
/// Rust note: the removals walk the indices in *descending* order, because
/// `Vec::remove` shifts everything after the hole down by one — taking index 0
/// first would leave every later index pointing at the wrong task. The
/// collected tasks are then replayed in ascending order so the target reads in
/// the source's original order.
pub fn pull_tasks(source: &mut Session, target: &mut Session, indices: &[usize]) -> usize {
    let mut wanted: Vec<usize> = indices
        .iter()
        .copied()
        .filter(|&i| source.active.get(i).is_some_and(|t| !t.done))
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
```

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test --lib session::tests::pull_tasks`
Expected: PASS, 5 tests.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/session.rs
git commit -m "feat: add session::pull_tasks to move open tasks between sessions"
```

---

### Task 2: `session::pull_from_file` — the two-file write

**Files:**
- Modify: `src/session.rs` (add `PullError` beside the other error enums, the function after `pull_tasks`, tests in `mod tests`)

**Interfaces:**
- Consumes: `pull_tasks` (Task 1), `Session::load`, `Session::save`, `LoadError`, `SaveError` (existing).
- Produces:
  - `pub enum PullError { LoadSource { name: String, source: LoadError }, SaveTarget { source: SaveError }, SaveSource { name: String, source: SaveError } }`
  - `pub fn pull_from_file(dir: &Path, source_name: &str, target: &mut Session, indices: &[usize]) -> Result<usize, PullError>`

- [ ] **Step 1: Write the failing tests**

Append to `src/session.rs`'s `mod tests`:

```rust
    #[test]
    fn pull_from_file_updates_both_files_on_disk() {
        let dir = tempfile::tempdir().unwrap();

        let mut old = create_new(dir.path(), ts_at(14, 30)).unwrap();
        old.add(Pane::Active, "carry me");
        old.add(Pane::Active, "leave me");
        old.save(dir.path()).unwrap();

        let mut current = create_new(dir.path(), ts_at(16, 45)).unwrap();
        let moved = pull_from_file(dir.path(), &old.filename, &mut current, &[0]).unwrap();
        assert_eq!(moved, 1);

        let source = Session::load(dir.path(), &old.filename).unwrap();
        assert_eq!(source.active.len(), 1);
        assert_eq!(source.active[0].text, "leave me");
        assert_eq!(source.completed[0].text, "carry me");
        assert!(source.completed[0].done);

        let target = Session::load(dir.path(), &current.filename).unwrap();
        assert_eq!(target.active[0].text, "carry me");
        assert!(!target.active[0].done);
        assert!(!current.dirty, "both files were written, nothing is pending");
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
        pull_from_file(dir.path(), &old.filename, &mut current, &[0, 1]).unwrap();

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

        pull_from_file(dir.path(), &old.filename, &mut current, &[0]).unwrap();
        assert!(dir.path().join(&current.filename).exists());
    }

    #[test]
    fn pull_from_file_names_a_missing_source() {
        let dir = tempfile::tempdir().unwrap();
        let mut current = create_new(dir.path(), ts_at(16, 45)).unwrap();

        let err = pull_from_file(dir.path(), "vanished.md", &mut current, &[0]).unwrap_err();
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
            pull_from_file(dir.path(), "bad.md", &mut current, &[0]),
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
        assert_eq!(pull_from_file(dir.path(), &old.filename, &mut current, &[]).unwrap(), 0);
        assert!(
            !dir.path().join(&current.filename).exists(),
            "an empty pull must not create the target file"
        );
    }
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test --lib session::tests::pull_from_file`
Expected: FAIL — `cannot find function 'pull_from_file' in this scope`.

- [ ] **Step 3: Write the implementation**

Add the error enum in `src/session.rs`, after `ResumeError`:

```rust
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
```

Add the function after `pull_tasks`:

```rust
/// Load `source_name`, move the tasks at `indices` into `target`, and write
/// both files.
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
    indices: &[usize],
) -> Result<usize, PullError> {
    let mut loaded = Session::load(dir, source_name).map_err(|e| PullError::LoadSource {
        name: source_name.to_string(),
        source: e,
    })?;

    let moved = pull_tasks(&mut loaded, target, indices);
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
```

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test --lib session::tests`
Expected: PASS — the 6 new tests plus the existing ones.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/session.rs
git commit -m "feat: add session::pull_from_file, saving target before source"
```

---

### Task 3: `pull::PullState` — the state machine

**Files:**
- Create: `src/pull.rs`
- Modify: `src/lib.rs` (declare the module)

**Interfaces:**
- Consumes: `picker::Entry` (existing, `pub` fields `name`/`open`/`total`/`contents`), `markdown::parse`, `model::Task`, `crossterm::event::KeyCode`.
- Produces:
  - `pub struct Candidate { pub name: String, pub open: Vec<(usize, String)> }`
  - `pub enum Stage { Sessions, Tasks }`
  - `pub struct Pulled { pub source: String, pub indices: Vec<usize> }`
  - `pub enum Step { Stay, Cancel, Pull(Pulled) }`
  - `pub struct PullState` with `pub fn new(entries: &[Entry], current: &str) -> Self` and `pub fn on_key(&mut self, code: KeyCode) -> Step`
  - `pub fn candidates(entries: &[Entry], current: &str) -> Vec<Candidate>`

- [ ] **Step 1: Create the module with its types and stubs**

Create `src/pull.rs`:

```rust
//! `p` — pull open tasks out of a past session into the current one.
//!
//! Two stages: choose a session, then choose which of its open tasks come
//! across. Every transition lives in [`PullState::on_key`], which is pure, so
//! the whole flow is unit-tested without a pty — the same split that makes
//! `picker`'s helpers testable. [`run`] is only a draw/read/dispatch loop.

use crate::markdown;
use crate::picker::Entry;
use crossterm::event::KeyCode;

/// A past session with something worth pulling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    /// Each open task's index in the file's `active` list, and its text. The
    /// index is what [`crate::session::pull_from_file`] needs; the text is
    /// what the screen shows.
    pub open: Vec<(usize, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Sessions,
    Tasks,
}

/// A confirmed pull.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pulled {
    pub source: String,
    /// Indices into the source's `active` list, ascending and unique.
    pub indices: Vec<usize>,
}

/// What a key did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Stay,
    Cancel,
    Pull(Pulled),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullState {
    candidates: Vec<Candidate>,
    stage: Stage,
    session_cursor: usize,
    /// Parallel to the chosen candidate's `open`. Empty in `Stage::Sessions`.
    selected: Vec<bool>,
    task_cursor: usize,
}

/// Past sessions that have at least one pullable task, newest first.
///
/// "Pullable" is narrower than the picker's `open` count and the difference is
/// load-bearing: `count_tasks` counts every `- [ ]` line wherever it sits,
/// while `parse` files tasks by the heading above them. A hand-written `- [ ]`
/// under `## Completed` is `open` but is not in `active`, and a pull moves
/// tasks out of `active`. Counting the wrong one would put a session on stage
/// one whose stage two is empty.
///
/// A file that fails to parse is dropped here rather than erroring in the
/// middle of the flow. `Entry` already carries the contents `scan` read, so
/// this costs no extra I/O.
pub fn candidates(entries: &[Entry], current: &str) -> Vec<Candidate> {
    todo!()
}

impl PullState {
    pub fn new(entries: &[Entry], current: &str) -> Self {
        todo!()
    }

    pub fn on_key(&mut self, code: KeyCode) -> Step {
        todo!()
    }
}
```

Add to `src/lib.rs`, keeping the list alphabetical (after `pub mod picker;`):

```rust
pub mod pull;
```

- [ ] **Step 2: Write the failing tests**

Append to `src/pull.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, contents: &str) -> Entry {
        let counts = markdown::count_tasks(contents);
        Entry {
            name: name.to_string(),
            open: counts.open,
            total: counts.total,
            contents: contents.to_string(),
        }
    }

    const TWO_OPEN: &str =
        "# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] alpha\n- [ ] beta\n\n## Completed\n\n- [x] gamma\n";

    fn state() -> PullState {
        PullState::new(&[entry("2026-05-31_14-30.md", TWO_OPEN)], "current.md")
    }

    #[test]
    fn candidates_carry_each_open_tasks_index_and_text() {
        let got = candidates(&[entry("a.md", TWO_OPEN)], "current.md");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "a.md");
        assert_eq!(got[0].open, vec![(0, "alpha".to_string()), (1, "beta".to_string())]);
    }

    #[test]
    fn candidates_exclude_the_current_session() {
        assert!(candidates(&[entry("current.md", TWO_OPEN)], "current.md").is_empty());
    }

    #[test]
    fn candidates_exclude_sessions_with_nothing_open() {
        let done = "## Completed\n\n- [x] finished\n";
        assert!(candidates(&[entry("a.md", done)], "current.md").is_empty());
    }

    #[test]
    fn candidates_ignore_an_open_line_filed_under_completed() {
        // `count_tasks` calls this session open; `parse` does not put the task
        // in `active`, so there is nothing here a pull could move.
        let odd = "## Completed\n\n- [ ] hand written in the wrong place\n";
        assert_eq!(markdown::count_tasks(odd).open, 1, "precondition");
        assert!(candidates(&[entry("a.md", odd)], "current.md").is_empty());
    }

    #[test]
    fn candidates_skip_a_task_that_is_done_but_filed_under_active() {
        let odd = "## Active\n\n- [x] already done\n- [ ] real work\n";
        let got = candidates(&[entry("a.md", odd)], "current.md");
        assert_eq!(got[0].open, vec![(1, "real work".to_string())]);
    }

    #[test]
    fn candidates_drop_a_file_that_cannot_be_parsed() {
        let bad = format!("## Active\n\n- [ ] {}\n", "x".repeat(200));
        assert!(candidates(&[entry("bad.md", &bad)], "current.md").is_empty());
    }

    #[test]
    fn enter_on_a_session_opens_its_tasks_all_selected() {
        let mut s = state();
        assert_eq!(s.on_key(KeyCode::Enter), Step::Stay);
        assert_eq!(s.stage, Stage::Tasks);
        assert_eq!(s.selected, vec![true, true]);
        assert_eq!(s.task_cursor, 0);
    }

    #[test]
    fn space_toggles_the_task_under_the_cursor() {
        let mut s = state();
        s.on_key(KeyCode::Enter);
        s.on_key(KeyCode::Char(' '));
        assert_eq!(s.selected, vec![false, true]);
        s.on_key(KeyCode::Char(' '));
        assert_eq!(s.selected, vec![true, true]);
    }

    #[test]
    fn a_selects_everything_unless_everything_is_already_selected() {
        let mut s = state();
        s.on_key(KeyCode::Enter);
        s.on_key(KeyCode::Char('a'));
        assert_eq!(s.selected, vec![false, false], "all selected -> clear");
        s.on_key(KeyCode::Char('a'));
        assert_eq!(s.selected, vec![true, true], "any clear -> select all");

        s.on_key(KeyCode::Char(' '));
        assert_eq!(s.selected, vec![false, true], "precondition: a mix");
        s.on_key(KeyCode::Char('a'));
        assert_eq!(s.selected, vec![true, true], "a mix selects all");
    }

    #[test]
    fn enter_pulls_the_selected_indices_in_ascending_order() {
        let mut s = state();
        s.on_key(KeyCode::Enter);
        s.on_key(KeyCode::Char(' ')); // clear index 0
        let step = s.on_key(KeyCode::Enter);
        assert_eq!(
            step,
            Step::Pull(Pulled {
                source: "2026-05-31_14-30.md".to_string(),
                indices: vec![1],
            })
        );
    }

    #[test]
    fn enter_on_an_empty_selection_does_nothing() {
        let mut s = state();
        s.on_key(KeyCode::Enter);
        s.on_key(KeyCode::Char('a')); // clear all
        assert_eq!(s.on_key(KeyCode::Enter), Step::Stay);
        assert_eq!(s.stage, Stage::Tasks, "and it stays on the screen");
    }

    #[test]
    fn esc_in_the_task_stage_goes_back_with_the_session_cursor_intact() {
        let two = vec![
            entry("2026-05-31_14-30.md", TWO_OPEN),
            entry("2026-05-30_09-00.md", TWO_OPEN),
        ];
        let mut s = PullState::new(&two, "current.md");
        s.on_key(KeyCode::Char('j'));
        assert_eq!(s.session_cursor, 1);

        s.on_key(KeyCode::Enter);
        assert_eq!(s.on_key(KeyCode::Esc), Step::Stay);
        assert_eq!(s.stage, Stage::Sessions);
        assert_eq!(s.session_cursor, 1, "the cursor must not reset");
    }

    #[test]
    fn esc_in_the_session_stage_cancels() {
        let mut s = state();
        assert_eq!(s.on_key(KeyCode::Esc), Step::Cancel);
    }

    #[test]
    fn q_cancels_from_either_stage() {
        let mut s = state();
        assert_eq!(s.on_key(KeyCode::Char('q')), Step::Cancel);

        let mut s = state();
        s.on_key(KeyCode::Enter);
        assert_eq!(s.on_key(KeyCode::Char('q')), Step::Cancel);
    }

    #[test]
    fn cursors_clamp_at_both_ends() {
        let mut s = state();
        s.on_key(KeyCode::Char('k'));
        assert_eq!(s.session_cursor, 0, "already at the top");
        s.on_key(KeyCode::Char('j'));
        assert_eq!(s.session_cursor, 0, "only one session to sit on");

        s.on_key(KeyCode::Enter);
        s.on_key(KeyCode::Down);
        assert_eq!(s.task_cursor, 1);
        s.on_key(KeyCode::Down);
        assert_eq!(s.task_cursor, 1, "two tasks, so index 1 is the end");
        s.on_key(KeyCode::Up);
        s.on_key(KeyCode::Up);
        assert_eq!(s.task_cursor, 0);
    }

    #[test]
    fn every_key_cancels_when_there_is_nothing_to_pull() {
        // The screen has nothing on it to act on, so no key should leave the
        // user stuck looking at it.
        for code in [KeyCode::Enter, KeyCode::Char('j'), KeyCode::Char(' '), KeyCode::Esc] {
            let mut s = PullState::new(&[], "current.md");
            assert_eq!(s.on_key(code), Step::Cancel, "{code:?} must cancel");
        }
    }
}
```

- [ ] **Step 3: Run the tests and verify they fail**

Run: `cargo test --lib pull::`
Expected: FAIL — the `todo!()` bodies panic with "not yet implemented".

- [ ] **Step 4: Write the implementation**

Replace the three `todo!()` bodies in `src/pull.rs`:

```rust
pub fn candidates(entries: &[Entry], current: &str) -> Vec<Candidate> {
    entries
        .iter()
        .filter(|e| e.name != current)
        .filter_map(|e| {
            let parsed = markdown::parse(&e.contents).ok()?;
            let open: Vec<(usize, String)> = parsed
                .active
                .iter()
                .enumerate()
                .filter(|(_, t)| !t.done)
                .map(|(i, t)| (i, t.text.clone()))
                .collect();
            if open.is_empty() {
                return None;
            }
            Some(Candidate {
                name: e.name.clone(),
                open,
            })
        })
        .collect()
}

impl PullState {
    pub fn new(entries: &[Entry], current: &str) -> Self {
        Self {
            candidates: candidates(entries, current),
            stage: Stage::Sessions,
            session_cursor: 0,
            selected: Vec::new(),
            task_cursor: 0,
        }
    }

    /// Whether there is anything to pull. The screen says so, and every key
    /// cancels.
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// The session the cursor is on, if any.
    fn current(&self) -> Option<&Candidate> {
        self.candidates.get(self.session_cursor)
    }

    pub fn on_key(&mut self, code: KeyCode) -> Step {
        if self.is_empty() {
            return Step::Cancel;
        }
        match self.stage {
            Stage::Sessions => self.session_key(code),
            Stage::Tasks => self.task_key(code),
        }
    }

    fn session_key(&mut self, code: KeyCode) -> Step {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => return Step::Cancel,
            KeyCode::Down | KeyCode::Char('j') => {
                if self.session_cursor + 1 < self.candidates.len() {
                    self.session_cursor += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.session_cursor = self.session_cursor.saturating_sub(1);
            }
            KeyCode::Enter => {
                let count = self.current().map_or(0, |c| c.open.len());
                self.selected = vec![true; count];
                self.task_cursor = 0;
                self.stage = Stage::Tasks;
            }
            _ => {}
        }
        Step::Stay
    }

    fn task_key(&mut self, code: KeyCode) -> Step {
        match code {
            KeyCode::Char('q') => return Step::Cancel,
            KeyCode::Esc => self.stage = Stage::Sessions,
            KeyCode::Down | KeyCode::Char('j') => {
                if self.task_cursor + 1 < self.selected.len() {
                    self.task_cursor += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.task_cursor = self.task_cursor.saturating_sub(1);
            }
            KeyCode::Char(' ') => {
                if let Some(flag) = self.selected.get_mut(self.task_cursor) {
                    *flag = !*flag;
                }
            }
            KeyCode::Char('a') => {
                let target = !self.selected.iter().all(|&s| s);
                self.selected.fill(target);
            }
            KeyCode::Enter => {
                let Some(candidate) = self.current() else {
                    return Step::Stay;
                };
                let indices: Vec<usize> = candidate
                    .open
                    .iter()
                    .zip(&self.selected)
                    .filter(|(_, &picked)| picked)
                    .map(|((i, _), _)| *i)
                    .collect();
                if indices.is_empty() {
                    return Step::Stay;
                }
                return Step::Pull(Pulled {
                    source: candidate.name.clone(),
                    indices,
                });
            }
            _ => {}
        }
        Step::Stay
    }
}
```

- [ ] **Step 5: Run the tests and verify they pass**

Run: `cargo test --lib pull::`
Expected: PASS, 16 tests.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/pull.rs src/lib.rs
git commit -m "feat: add the pull screen state machine"
```

---

### Task 4: `pull::draw` and `pull::run` — the screens

**Files:**
- Modify: `src/pull.rs` (rendering, the loop, and draw tests)

**Interfaces:**
- Consumes: `PullState` (Task 3), `picker::pretty_name` (existing, already `pub`), `picker::scan` (existing), `ui::Palette`, `tui::Tui`, `ratatui::Frame`.
- Produces: `pub fn run(dir: &Path, tui: &mut Tui, palette: Palette, current: &str) -> std::io::Result<Option<Pulled>>`

- [ ] **Step 1: Write the failing draw tests**

Add to `src/pull.rs`'s `mod tests` (and extend the `use super::*;` at its top with `use ratatui::Terminal; use ratatui::backend::TestBackend;`):

```rust
    fn render(width: u16, height: u16, state: &PullState) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| draw(f, state, crate::ui::Palette::new(true)))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn screen(buf: &ratatui::buffer::Buffer) -> String {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_session_stage_lists_names_and_open_counts() {
        let text = screen(&render(70, 8, &state()));
        assert!(text.contains("2026-05-31  14:30"), "{text}");
        assert!(text.contains("2 open"), "{text}");
        assert!(text.contains("Enter choose"), "{text}");
    }

    #[test]
    fn the_task_stage_shows_checkboxes_and_a_live_count() {
        let mut s = state();
        s.on_key(KeyCode::Enter);
        let text = screen(&render(70, 8, &s));
        assert!(text.contains("[x] alpha"), "{text}");
        assert!(text.contains("[x] beta"), "{text}");
        assert!(text.contains("Enter pull 2"), "{text}");

        s.on_key(KeyCode::Char(' '));
        let text = screen(&render(70, 8, &s));
        assert!(text.contains("[ ] alpha"), "{text}");
        assert!(text.contains("Enter pull 1"), "the count must follow: {text}");
    }

    #[test]
    fn the_task_stage_headline_names_the_session() {
        let mut s = state();
        s.on_key(KeyCode::Enter);
        let text = screen(&render(70, 8, &s));
        assert!(text.contains("2026-05-31  14:30"), "{text}");
    }

    #[test]
    fn an_empty_state_says_so() {
        let text = screen(&render(70, 8, &PullState::new(&[], "current.md")));
        assert!(text.contains("no other sessions have open tasks"), "{text}");
    }
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test --lib pull::`
Expected: FAIL — `cannot find function 'draw' in this scope`.

- [ ] **Step 3: Write the rendering and the loop**

**Replace** Task 3's three-line import block at the top of `src/pull.rs` with
this one — `use crate::picker::Entry;` becomes `use crate::picker::{self, Entry};`,
so a second `use` of `picker` would not compile:

```rust
use crate::markdown;
use crate::picker::{self, Entry};
use crate::tui::Tui;
use crate::ui::Palette;
use crossterm::event::{self, Event, KeyCode};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem};
use std::path::Path;
```

Add before `mod tests`:

```rust
/// Marker in front of the row the cursor is on. Same glyph the picker uses.
const CURSOR: &str = "\u{25b8}";

/// Column the session rows' open-counts start on, so they line up under each
/// other without the picker's full right-alignment machinery.
const COUNT_COLUMN: usize = 26;

/// Draw whichever stage is current. Chrome is monochrome, like the picker's.
fn draw(frame: &mut Frame, state: &PullState, palette: Palette) {
    let _ = palette;
    let [header, body, status] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let (title, rows, hint) = if state.is_empty() {
        (
            " caleb \u{2014} pull open tasks".to_string(),
            vec!["  no other sessions have open tasks".to_string()],
            " any key returns".to_string(),
        )
    } else {
        match state.stage {
            Stage::Sessions => (
                " caleb \u{2014} pull open tasks from".to_string(),
                state
                    .candidates
                    .iter()
                    .enumerate()
                    .map(|(i, c)| session_row(c, i == state.session_cursor))
                    .collect(),
                " j/k move   Enter choose   Esc cancel".to_string(),
            ),
            Stage::Tasks => {
                let candidate = state.current().expect("a stage-two session exists");
                (
                    format!(
                        " caleb \u{2014} open tasks in {}",
                        picker::pretty_name(&candidate.name)
                    ),
                    candidate
                        .open
                        .iter()
                        .zip(&state.selected)
                        .enumerate()
                        .map(|(i, ((_, text), &picked))| {
                            task_row(text, picked, i == state.task_cursor)
                        })
                        .collect(),
                    format!(
                        " space toggle   a all/none   Enter pull {}   Esc back",
                        state.selected.iter().filter(|&&s| s).count()
                    ),
                )
            }
        }
    };

    frame.render_widget(
        Line::from(title).style(Style::default().add_modifier(Modifier::BOLD)),
        ratatui::layout::Rect {
            height: 1,
            ..header
        },
    );
    frame.render_widget(
        List::new(rows.into_iter().map(ListItem::new).collect::<Vec<_>>()),
        body,
    );
    frame.render_widget(
        Line::from(hint).style(Style::default().add_modifier(Modifier::DIM)),
        status,
    );
}

/// `▸ 2026-05-31  14:30         2 open`. A hand-named session longer than
/// `COUNT_COLUMN` keeps one space before its count rather than running the two
/// halves together, which is what the `.max(1)` is for.
fn session_row(candidate: &Candidate, selected: bool) -> String {
    let marker = if selected { CURSOR } else { " " };
    let name = picker::pretty_name(&candidate.name);
    let pad = COUNT_COLUMN.saturating_sub(name.chars().count());
    format!(
        "{marker} {name}{:pad$}{} open",
        "",
        candidate.open.len(),
        pad = pad.max(1)
    )
}

fn task_row(text: &str, picked: bool, selected: bool) -> String {
    let marker = if selected { CURSOR } else { " " };
    let box_ = if picked { 'x' } else { ' ' };
    format!("{marker} [{box_}] {text}")
}

/// Run the two-stage picker to a decision. `None` means the user cancelled.
///
/// Mouse events are ignored on purpose: a stray click must not toggle a task
/// the user did not mean to include, and there is no drag or scroll here worth
/// the hit-testing.
pub fn run(
    dir: &Path,
    tui: &mut Tui,
    palette: Palette,
    current: &str,
) -> std::io::Result<Option<Pulled>> {
    let entries = picker::scan(dir)?;
    let mut state = PullState::new(&entries, current);

    loop {
        tui.terminal().draw(|frame| draw(frame, &state, palette))?;

        if let Event::Key(key) = event::read()? {
            if key.kind == event::KeyEventKind::Press {
                match state.on_key(key.code) {
                    Step::Stay => {}
                    Step::Cancel => return Ok(None),
                    Step::Pull(pulled) => return Ok(Some(pulled)),
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test --lib pull::`
Expected: PASS, 20 tests.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/pull.rs
git commit -m "feat: render and drive the pull screens"
```

---

### Task 5: Wire `p` into the session screen

**Files:**
- Modify: `src/app.rs` (`RunError`, new `Action` enum, `handle_key`, `handle_event`, `run`, tests)

**Interfaces:**
- Consumes: `pull::run`, `pull::Pulled` (Task 4), `session::pull_from_file`, `session::PullError` (Task 2).
- Produces: `pub enum Action { None, Pull }`; `handle_key` and `handle_event` now return `Result<Action, SaveError>`.

- [ ] **Step 1: Write the failing tests**

Add to `src/app.rs`'s `mod tests`, and change the existing `press` helper to return the action:

```rust
    fn press(app: &mut App, c: char) -> Action {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
            .unwrap()
    }

    #[test]
    fn p_requests_a_pull() {
        let mut app = app_with(&["a"]);
        assert_eq!(press(&mut app, 'p'), Action::Pull);
    }

    #[test]
    fn other_keys_request_nothing() {
        let mut app = app_with(&["a", "b"]);
        for c in ['j', 'k', 'h', 'l', 'g', 'G', 'd', 'x', 'a', '?'] {
            let mut app = app_with(&["a", "b"]);
            assert_eq!(press(&mut app, c), Action::None, "{c} must not pull");
        }
        assert_eq!(press(&mut app, 'q'), Action::None);
    }

    #[test]
    fn p_is_a_literal_character_while_adding_a_task() {
        let mut app = app_with(&[]);
        press(&mut app, 'a');
        assert_eq!(press(&mut app, 'p'), Action::None);
        assert_eq!(app.input, "p", "it must type, not pull");
    }

    #[test]
    fn p_dismisses_the_help_overlay_without_pulling() {
        let mut app = app_with(&[]);
        press(&mut app, '?');
        assert_eq!(press(&mut app, 'p'), Action::None);
        assert_eq!(app.mode, Mode::Normal);
    }
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test --lib app::`
Expected: FAIL — `cannot find type 'Action' in this scope`.

- [ ] **Step 3: Add the action type and thread it through**

In `src/app.rs`, add after the `Mode` enum:

```rust
/// Work the event loop must do that `handle_key` cannot: it would need the
/// terminal, and keeping `handle_key` terminal-free is what makes every
/// binding testable without a pty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Pull,
}
```

Extend `RunError`:

```rust
    #[error(transparent)]
    Pull(#[from] crate::session::PullError),
```

Change `handle_key`'s signature and its early returns:

```rust
    fn handle_key(&mut self, key: KeyEvent) -> Result<Action, SaveError> {
        match self.mode {
            Mode::Help => {
                if !matches!(key.code, KeyCode::Null) {
                    self.mode = Mode::Normal;
                }
                return Ok(Action::None);
            }
            Mode::AddInput => {
                self.handle_input_key(key);
                return Ok(Action::None);
            }
            Mode::Normal => {}
        }

        let mut action = Action::None;
        match key.code {
```

Add the binding inside that `match`, next to the other editing keys:

```rust
            KeyCode::Char('p') => action = Action::Pull,
```

and replace the tail of the function:

```rust
        self.clamp_cursors();
        Ok(action)
    }
```

Change `handle_event` to carry the action out:

```rust
    fn handle_event(
        &mut self,
        event: &Event,
        now: Instant,
        pane_height: u16,
    ) -> Result<Action, SaveError> {
        match *event {
            Event::Key(key) if key.kind == event::KeyEventKind::Press => {
                let action = self.handle_key(key)?;
                self.adjust_scroll(pane_height);
                return Ok(action);
            }
            Event::Mouse(m) => {
                self.handle_mouse(m, now);
                self.clamp_scroll_only(pane_height);
            }
            _ => {}
        }
        Ok(Action::None)
    }
```

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test --lib app::`
Expected: PASS — the 4 new tests plus the existing ones. Existing tests that call `press(...)` as a statement still compile; `Action` is not `#[must_use]`.

- [ ] **Step 5: Perform the pull from the event loop**

Add the imports at the top of `src/app.rs`:

```rust
use crate::pull;
use crate::session::{self, SaveError, Session};
```

(replacing the existing `use crate::session::{SaveError, Session};`)

Add the method to `impl App`:

```rust
    /// Hand the terminal to the pull screens, then apply whatever they return.
    ///
    /// A pull writes both files, so it doubles as a save point for the current
    /// session — the help overlay says so.
    fn pull(&mut self, tui: &mut Tui) -> Result<(), RunError> {
        let Some(pulled) = pull::run(
            &self.storage_dir,
            tui,
            self.palette,
            &self.session.filename,
        )?
        else {
            return Ok(());
        };

        let dir = self.storage_dir.clone();
        let moved =
            session::pull_from_file(&dir, &pulled.source, &mut self.session, &pulled.indices)?;

        // Land the cursor on the first task that arrived. `moved` is what
        // actually moved, which can be fewer than were asked for, so the
        // arithmetic saturates rather than wrapping.
        self.focused = Pane::Active;
        self.active_cursor = self.session.active.len().saturating_sub(moved);
        self.clamp_cursors();
        Ok(())
    }
```

Change the body of `run`'s loop:

```rust
            let action = self.handle_event(&event::read()?, Instant::now(), pane_height)?;
            if action == Action::Pull {
                self.pull(tui)?;
            }
```

- [ ] **Step 6: Verify the whole suite still passes**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/app.rs
git commit -m "feat: bind p to pull open tasks from a past session"
```

---

### Task 6: Help overlay and documentation

**Files:**
- Modify: `src/ui.rs` (`HELP_LINES`, plus a width-consistency test)
- Modify: `README.md`
- Modify: `AGENTS.md`

**Interfaces:**
- Consumes: `HELP_LINES`, `HELP_INNER_W` (existing).
- Produces: nothing other code depends on.

- [ ] **Step 1: Write the failing tests**

Add to `src/ui.rs`'s `mod tests`:

```rust
    #[test]
    fn the_help_overlay_documents_the_pull_key() {
        let text = HELP_LINES.join("\n");
        assert!(text.contains("pull"), "the p key must be listed:\n{text}");
        assert!(
            text.contains("saves"),
            "pulling writes the current session; the overlay must say so:\n{text}"
        );
    }

    #[test]
    fn every_help_line_is_the_same_width() {
        // The overlay is a fixed-width field; a short line leaves a ragged
        // hole in the box.
        for line in HELP_LINES {
            assert_eq!(
                line.chars().count(),
                HELP_INNER_W as usize,
                "ragged help line: {line:?}"
            );
        }
    }
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test --lib ui::tests::the_help_overlay_documents_the_pull_key ui::tests::every_help_line_is_the_same_width`
Expected: FAIL on the first — "the p key must be listed". The width test may pass already; keep it as a regression guard.

- [ ] **Step 3: Add the help lines**

In `src/ui.rs`, insert into `HELP_LINES` immediately after the `Shift+J / K` line (both padded to exactly 52 columns):

```rust
    "   p            pull tasks from a past session      ",
    "                (saves the current session)         ",
```

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test --lib ui::`
Expected: PASS.

- [ ] **Step 5: Update README.md**

In the `## Use` code block, leave the CLI lines alone — `p` is an in-app key. After the paragraph about the picker hiding finished sessions, add:

```markdown
Press `p` in a session to pull unfinished work forward. Choose a past session,
then tick which of its open tasks you want; they arrive in your Active pane and
are checked off in the session they came from. That session drops to zero open
tasks, so `caleb --clean` can then sweep it. Pulling saves the current session.
```

- [ ] **Step 6: Update AGENTS.md**

In the layout table, add after the `src/picker.rs` row:

```markdown
| `src/pull.rs` | `p` screens: session then task selection, pure `on_key` state machine |
```

Extend the `src/session.rs` row to end with `; pull_tasks / pull_from_file`.

After the paragraph about resume's rename, add:

```markdown
`p` pulls open tasks out of a past session: they land open in the current
session and completed in the source. `session::pull_from_file` saves the
**target before the source** on purpose — a failure between the two writes
leaves the tasks open in both files, which is visible and fixable, whereas the
other order loses them. `pull::candidates` counts open tasks with
`markdown::parse`, not `count_tasks`: a pull moves tasks out of `active`, and
`count_tasks` would also count a `- [ ]` hand-filed under `## Completed`.
```

- [ ] **Step 7: Verify and commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui.rs README.md AGENTS.md
git commit -m "docs: document the pull key in the overlay, README, and AGENTS"
```

---

### Task 7: End-to-end coverage

**Files:**
- Modify: `tests/roundtrip.rs`
- Create: `scripts/smoke_pull.py`
- Modify: `AGENTS.md` (the smoke-test section and the layout table's `scripts/` rows)

**Interfaces:**
- Consumes: the public API — `session::create_new`, `session::pull_from_file`, `Session::load`, `picker::scan`, `pull::candidates`.
- Produces: nothing other code depends on.

- [ ] **Step 1: Write the failing integration test**

Append to `tests/roundtrip.rs`:

```rust
#[test]
fn pulled_tasks_cross_the_file_boundary_and_are_not_offered_twice() {
    let dir = tempfile::tempdir().unwrap();

    let mut old = session::create_new(dir.path(), ts(14, 30)).unwrap();
    old.add(Pane::Active, "carry me");
    old.add(Pane::Active, "and me");
    old.add(Pane::Active, "not me");
    old.save(dir.path()).unwrap();

    let mut current = session::create_new(dir.path(), ts(16, 45)).unwrap();
    let moved =
        session::pull_from_file(dir.path(), &old.filename, &mut current, &[0, 1]).unwrap();
    assert_eq!(moved, 2);

    // Both files, re-read from disk through the public API.
    let source = Session::load(dir.path(), &old.filename).unwrap();
    assert_eq!(source.active.len(), 1);
    assert_eq!(source.active[0].text, "not me");
    let done: Vec<&str> = source.completed.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(done, ["carry me", "and me"]);

    let target = Session::load(dir.path(), &current.filename).unwrap();
    let carried: Vec<&str> = target.active.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(carried, ["carry me", "and me"]);
    assert!(target.active.iter().all(|t| !t.done));

    // A second trip through the picker offers only what stayed behind.
    let entries = caleb::picker::scan(dir.path()).unwrap();
    let offered = caleb::pull::candidates(&entries, &current.filename);
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].name, old.filename);
    assert_eq!(offered[0].open, vec![(0, "not me".to_string())]);
}
```

- [ ] **Step 2: Run it and verify it fails**

Run: `cargo test --test roundtrip pulled_tasks_cross_the_file_boundary`
Expected: FAIL if any earlier task was left incomplete; PASS once Tasks 1–4 are in. If it passes immediately, that is the expected outcome here — this task's purpose is coverage across the module seam, not a new behavior.

- [ ] **Step 3: Write the smoke script**

Create `scripts/smoke_pull.py`, mirroring `scripts/smoke_picker.py`'s structure:

```python
#!/usr/bin/env python3
"""Drive `p` through a pty: pick a session, deselect a task, pull the rest.

The unit tests cover every transition in `PullState`; what only a pty can show
is that the key reaches the state machine at all, that the screens actually
render, and that the pulled tasks are on the session screen afterwards.

Fixtures use single-token task text on purpose. ratatui writes only the cells
that changed, so a phrase with spaces arrives split by cursor-position escapes
and will not match as a contiguous substring.
"""
import os, pty, fcntl, termios, struct, select, time, pathlib, shutil

DATA = "/tmp/caleb-pull-smoke"
shutil.rmtree(DATA, ignore_errors=True)
sessions = pathlib.Path(DATA, "caleb")
sessions.mkdir(parents=True)

(sessions / "2026-05-30_09-00.md").write_text(
    "# Session 2026-05-30 09:00\n\n## Active\n\n- [ ] alphatask\n- [ ] betatask\n"
)

pid, fd = pty.fork()
if pid == 0:
    os.environ["XDG_DATA_HOME"] = DATA
    os.environ["TERM"] = "xterm-256color"
    os.execv("./target/debug/caleb", ["./target/debug/caleb"])

fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))


def drain(timeout=0.3):
    out = b""
    while select.select([fd], [], [], timeout)[0]:
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            break
        if not chunk:
            break
        out += chunk
    return out.decode(errors="replace")


def send(keys):
    """Keys one at a time, draining after each — see AGENTS.md."""
    out = ""
    for k in keys:
        os.write(fd, k.encode())
        out += drain()
    return out


drain()

# --- stage one -----------------------------------------------------------
stage1 = send(["p"])
assert "pull open tasks from" in stage1, f"'p' should open the pull screen:\n{stage1}"
assert "2026-05-30" in stage1, f"the session must be listed:\n{stage1}"
assert "2 open" in stage1, f"with its pullable count:\n{stage1}"

# --- stage two -----------------------------------------------------------
stage2 = send(["\r"])
assert "alphatask" in stage2, f"its open tasks must be listed:\n{stage2}"
assert "Enter pull 2" in stage2, f"all selected by default:\n{stage2}"

deselected = send([" "])
assert "Enter pull 1" in deselected, f"space should deselect:\n{deselected}"

# --- back on the session screen ------------------------------------------
pulled = send(["\r"])
assert "betatask" in pulled, f"the pulled task should be in Active:\n{pulled}"
assert "pull open tasks" not in pulled, f"the pull screen should be gone:\n{pulled}"

send(["q"])
time.sleep(0.3)
os.waitpid(pid, 0)

after = (sessions / "2026-05-30_09-00.md").read_text()
assert "- [x] betatask" in after, f"the source must check it off:\n{after}"
assert "- [ ] alphatask" in after, f"the deselected one stays open:\n{after}"
print("SMOKE OK")
```

- [ ] **Step 4: Run everything**

```bash
cargo build
cargo test
python3 scripts/smoke_pull.py
```
Expected: `SMOKE OK`, and a green `cargo test`.

- [ ] **Step 5: Document the script**

In `AGENTS.md`'s layout table, add after the `scripts/smoke_picker.py` row:

```markdown
| `scripts/smoke_pull.py` | pty end-to-end test: the `p` pull flow, both stages |
```

Add `python3 scripts/smoke_pull.py` alongside the other smoke-script invocations in the TUI smoke-testing section.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
python3 scripts/smoke_pull.py
git add tests/roundtrip.rs scripts/smoke_pull.py AGENTS.md
git commit -m "test: cover the pull flow end to end and through a pty"
```

---

## Verification

After Task 7, all of these must pass from a clean tree:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
python3 scripts/smoke.py
python3 scripts/smoke_picker.py
python3 scripts/smoke_pull.py
```

Manual check, since no automated test covers the return-to-session cursor placement:

```bash
XDG_DATA_HOME=/tmp/caleb-dev cargo run
```
Add a task, press `p`, pick a session, press Enter, and confirm the cursor sits on the first pulled task in the Active pane.
