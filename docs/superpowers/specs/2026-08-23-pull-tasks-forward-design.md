# caleb — pull open tasks forward

**Date:** 2026-08-23
**Status:** approved, ready for implementation planning

## Purpose

Work rarely finishes inside one session. Today the only way to carry an
unfinished task into a new session is `-r`, which *renames* the old file and
resumes it wholesale — all or nothing, and the old session's identity is
destroyed.

This feature adds the other half: from inside a running session, pull selected
open tasks out of a past session and into the current one. The past session
keeps its identity and its history; the tasks it gave up are checked off there,
so a session you fully drain reaches `0 open` and becomes eligible for
`caleb --clean`.

## Semantics

Pulling one task does three things atomically in memory:

| | before | after |
|---|---|---|
| source `## Active` | `- [ ] wire up retries` | — |
| source `## Completed` | — | `- [x] wire up retries` |
| target `## Active` | — | `- [ ] wire up retries` |

The source's copy is marked **done**, not deleted. Rationale: the old session
should read as finished, not as though the work never existed, and a `0 open`
session is what `--clean` looks for. The task text is copied verbatim; no
annotation, arrow, or provenance marker is added, because caleb's markdown
grammar treats everything after `- [x] ` as task text and any marker would
become part of the task string on the next parse.

Explicitly rejected alternatives: deleting from the source (loses the record),
and copying without touching the source (leaves the source open forever, so
`--clean` can never reach it and the same task is offered again on every
subsequent pull).

## User interface

### Entry

`p` — for *pull* — in the main screen's `Normal` mode. Available at any point
in a session, new or resumed. In `AddInput` mode `p` is a literal character, as
now; in `Help` mode it dismisses the overlay, as now.

### Stage 1 — choose a session

Full-screen, replacing the session view for the duration.

```
 caleb — pull open tasks from

   2026-08-20  09:14      3 open
 > 2026-08-19  14:30      1 open
   2026-08-15  11:02      7 open

 j/k move   Enter choose   Esc cancel
```

- Lists sessions that have at least one **pullable** task, **excluding the
  current session's own file** (matched on `Session::filename`). A session with
  nothing open has nothing to give, so there is no `a` show-all toggle.

  "Pullable" is narrower than the picker's `open` count, and the difference
  matters. `markdown::count_tasks` — which feeds `Entry::open` and `--clean` —
  counts every `- [ ]` line wherever it sits, while `markdown::parse` sorts
  tasks by the heading they fall under, so a hand-written `- [ ]` beneath
  `## Completed` is `open` but is not in `parse`'s `active` list. A pull moves
  tasks out of `active`, so the pullable set is **`parse(contents).active`
  filtered to `!done`**, and stage 1 shows that count. Using `Entry::open`
  would let stage 1 advertise a session whose stage 2 is empty.
- Newest first — `picker::scan` order, unchanged.
- No preview pane. Stage 2 shows the tasks themselves, which is what a preview
  would have been for. This also keeps `pull` from needing any of `picker`'s
  layout constants. Session names are rendered through `picker::pretty_name`
  (already `pub`), so both screens spell a session the same way.
- An empty list renders `  no other sessions have open tasks`, and **any** key
  cancels — there is nothing on the screen to act on.

### Stage 2 — choose tasks

```
 caleb — open tasks in 2026-08-19  14:30

   [x] wire up retries
 > [ ] fix the flaky picker test
   [x] document --clean

 space toggle   a all/none   Enter pull 2   Esc back
```

- Every task starts **selected**: taking all of them is the common case, and
  `a` clears the lot in one keystroke.
- `space` toggles the task under the cursor. `a` selects all if any are
  unselected, otherwise deselects all.
- The footer's count is live. `Enter` with zero selected is a no-op — it neither
  pulls nor leaves the screen.
- `Esc` returns to stage 1 with the session cursor where it was. `q` cancels
  the whole flow, matching the picker.
- Nothing is written to disk until `Enter` with a non-empty selection.

### Return

On a successful pull, control returns to the session view with the cursor on
the first pulled task in the Active pane, and the focus on the Active pane. The
tasks are visibly there, so no confirmation dialog and no status message.

## Persistence

A pull writes **both** files. Order is deliberate:

1. Load the source session from disk (fresh — the picker's scan may be stale).
2. Apply the move in memory to both `Session` values.
3. **Save the target first.**
4. **Save the source second.**

A crash or write failure between 3 and 4 leaves the tasks open in *both* files:
visible, and fixable by hand. The reverse order would lose them outright. If
step 4 fails, the pull has already landed in the target — report the failure and
do not roll back, since a rollback can only lose more.

A pull is therefore a save point for the current session: in-progress edits are
flushed along with the pulled tasks. This is stated in the help overlay.

Step 2 does not trust the stage-1/stage-2 indices on position alone. They were
computed against the file contents `picker::scan` captured when `p` was
pressed, and the source is reloaded from disk fresh in step 1 — if the file
changed in between (a second `caleb` instance, or a hand edit while the picker
was on screen), an index could now point at a different task than the one the
user saw and picked. Each selected entry therefore carries its captured text
alongside its index, and the move verifies the two together before anything is
taken: an index whose text no longer matches what is on disk is skipped, the
same as an out-of-range or already-completed one.

## Code shape

### `src/session.rs` — the operation

```rust
/// Move the named tasks out of `source.active` and into `target.active`,
/// marking each completed in `source`. Returns how many moved. Out-of-range,
/// already-done, or text-mismatched entries are ignored, as in `toggle` and
/// `delete` — the text travels alongside each index so a stale one is
/// distinguishable from a still-valid one.
pub fn pull_tasks(source: &mut Session, target: &mut Session, tasks: &[(usize, String)]) -> usize;

/// Load `source_name`, apply `pull_tasks`, then save the target and the
/// source in that order.
pub fn pull_from_file(
    dir: &Path,
    source_name: &str,
    target: &mut Session,
    tasks: &[(usize, String)],
) -> Result<usize, PullError>;

#[derive(Debug, Error)]
pub enum PullError {
    #[error("cannot load session '{name}': {source}")]
    LoadSource { name: String, source: LoadError },
    #[error("cannot save the current session: {source}")]
    SaveTarget { source: SaveError },
    #[error("tasks were pulled, but '{name}' could not be updated: {source}")]
    SaveSource { name: String, source: SaveError },
}
```

`pull_tasks` walks the indices in descending order so each removal cannot
shift the ones still to come. It de-duplicates and sorts them itself rather
than trusting the caller. Both sessions are marked `dirty`; `save` clears the
flag as usual. It lives beside `resume`, which is the same kind of two-file
operation, and it keeps session mutation out of the terminal-edge modules, as
the crate docs require.

No text de-duplication against the target's existing tasks. Pulling the same
session twice cannot re-offer the same task — it is checked off after the first
pull — so the only way to get a duplicate is two sessions genuinely holding the
same text, which is not caleb's business to second-guess.

### `src/pull.rs` — the screens (new)

```rust
pub enum Stage { Sessions, Tasks }

pub struct PullState {
    entries: Vec<picker::Entry>,   // sessions with open > 0, current excluded
    stage: Stage,
    session_cursor: usize,
    tasks: Vec<Task>,              // open tasks of the chosen session
    selected: Vec<bool>,           // parallel to `tasks`
    task_cursor: usize,
}

/// A confirmed pull: which session, and which of its open tasks.
pub struct Pulled {
    pub source: String,
    /// The chosen entries of the source's `open` list, ascending and unique —
    /// text travels with each index so a stale one can be told apart from a
    /// still-valid one.
    pub tasks: Vec<(usize, String)>,
}

/// What a key did to the state.
pub enum Step {
    Stay,
    Cancel,
    Pull(Pulled),
}

impl PullState {
    pub fn new(entries: Vec<picker::Entry>, current: &str) -> Self;
    pub fn on_key(&mut self, code: KeyCode) -> Step;
}

/// Thin draw/read/dispatch loop, mirroring `picker::run`.
/// `None` when the user cancelled at either stage.
pub fn run(dir: &Path, tui: &mut Tui, palette: Palette, current: &str)
    -> std::io::Result<Option<Pulled>>;

fn draw(frame: &mut Frame, state: &PullState, palette: Palette);
```

`on_key` is pure and holds every transition, so the whole flow is unit-tested
without a pty — the same split that makes `picker::confirm_key` and
`filter_visible` testable today.

Entering stage 2 needs the chosen session's open tasks. `picker::Entry` already
carries the file's full `contents` (scan reads every file anyway), so
`markdown::parse` on that string yields them with no extra I/O and no new
failure mode in the middle of the flow. A source file whose contents fail to
parse is dropped from the stage 1 list at construction time rather than
erroring mid-flow.

### `src/app.rs` — dispatch

`handle_key` currently returns `Result<(), SaveError>` and never touches the
terminal, which is what makes every binding testable. To keep that true, it
gains a return value instead of a `&mut Tui` parameter:

```rust
pub enum Action { None, Pull }

fn handle_key(&mut self, key: KeyEvent) -> Result<Action, SaveError>;
```

`App::run` — which already holds the `&mut Tui` — is what calls `pull::run` and
applies the result:

```rust
Action::Pull => {
    if let Some(p) = pull::run(&dir, tui, self.palette, &self.session.filename)? {
        let n = session::pull_from_file(&dir, &p.source, &mut self.session, &p.tasks)?;
        self.focused = Pane::Active;
        // saturating: `pull_from_file` reports what actually moved, which can
        // be fewer than were asked for if the source changed underneath.
        self.active_cursor = self.session.active.len().saturating_sub(n);
        self.clamp_cursors();
    }
}
```

A `PullError` ends the run the way a save failure does — through `RunError`,
with the terminal restored by `Tui`'s RAII guard, so the message is legible.
`RunError` gains `#[error(transparent)] Pull(#[from] PullError)`.

### `src/ui.rs` — help

One line in `HELP_LINES`, under `Editing`, right-padded to `HELP_INNER_W` like
its neighbours:

```
   p            pull open tasks from a past session
```

The wording carries the save-point fact, since a user pressing `p` mid-edit
needs to know their buffer is about to hit disk:

```
   (pulling saves the current session)
```

## Testing

**`session::pull_tasks` (unit).** Moves only the named indices; leaves the rest
of `source.active` in order; sets `done` on the source copy and clears it on
the target copy; appends to `target.active` in source order; marks both dirty;
descending-walk safety with non-adjacent and unsorted indices; out-of-range and
empty-slice are no-ops that leave `dirty` alone.

**`session::pull_from_file` (tempdir).** Both files on disk hold the expected
markdown afterwards; the source drops to `0 open` when fully drained; a missing
source file yields `PullError::LoadSource`; an unparseable source yields the
same; a target that has never been saved is created.

**`pull::PullState` (unit).** Construction excludes the current filename and
sessions with no open tasks; `Enter` in stage 1 loads that session's open tasks
all selected; `space` and `a` toggle as specified; `Enter` on an empty selection
returns `Stay`; `Enter` on a non-empty selection returns the right indices in
ascending order; `Esc` in stage 2 returns to stage 1 with the session cursor
preserved; `Esc` in stage 1 returns `Cancel`; cursor movement clamps at both
ends; an empty entry list makes any key `Cancel`; `Enter` in stage 1 on a
session whose only open tasks vanished from disk cannot happen, because stage 2
reads the contents `scan` already captured.

**`pull::draw` (TestBackend).** Stage 1 renders the session names and open
counts; stage 2 renders checkboxes and a live count in the footer; the empty
state renders its message. Same approach as `picker`'s existing draw tests.

**`app` (unit).** `p` in `Normal` returns `Action::Pull`; `p` in `AddInput`
appends a literal `p`; every other key still returns `Action::None`.

**`tests/roundtrip.rs`.** A file-level pass through the public API: two sessions
on disk, pull two of three tasks, assert both files' contents and that a second
pull from the same session now offers only the one that stayed.

**`scripts/smoke_pull.py`.** A pty run of the whole flow — `p`, choose a
session, deselect one, `Enter`, confirm the tasks appear — following the shape
of `smoke_picker.py`.

## Documentation

- `src/ui.rs` help overlay, as above.
- `README.md`: the key in the key list, and a short paragraph next to the
  resume/`--clean` explanation covering what happens to the source session.
- `AGENTS.md`: `src/pull.rs` in the layout table, `src/session.rs`'s row
  extended, `tests/` rows extended, and the save-ordering rationale recorded
  next to the existing notes on resume's rename.

## Out of scope

Pulling from more than one session per trip; undo; text de-duplication; a
prompt at session start; a CLI flag; a preview pane in stage 1; provenance
markers in the source file; pulling *completed* tasks.
