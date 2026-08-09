# caleb — design

**Date:** 2026-08-09
**Status:** approved, ready for implementation planning

A Rust/ratatui reimplementation of [`ava`](../../../../zig/ava) — a full-screen
TUI to-do tracker for coding sessions. Same functionality, same aesthetics,
different language.

Named for Caleb Smith in *Ex Machina* (Alex Garland, 2014), as ava was named
for the film's robot.

**The ava project is reference material only. Nothing in it gets modified.**

---

## Purpose

Two goals, in priority order:

1. **A working, installable Rust binary.** `cargo install --path .` produces
   `caleb`, functionally and visually indistinguishable from `ava` except
   where this document says otherwise.
2. **A Rust learning vehicle.** ava was built to practice Zig's memory
   management, error handling, and built-in testing. caleb's learning targets
   are the Rust analogs: ownership and borrowing, `Result`/`?`/`From`, enums
   and pattern matching, traits (`Drop`, `Display`, `Error`), iterators, and
   the module system — plus ratatui's widget/buffer model.

Every module is written by Claude with comments explaining the Rust-specific
choices (why `Option<Timestamp>` and not a null, why `&str` here and `String`
there, what `?` desugars to, why `Vec<Task>` needs no explicit free). The
annotated diff against ava's Zig is itself the lesson.

---

## Dependencies

| Crate | Role | Replaces in ava |
|---|---|---|
| `ratatui` | Widgets, layout, cell buffer | most of `render.zig` |
| `crossterm` | Raw mode, alt screen, key/mouse/resize events | `terminal.zig`, `input.zig`, `ansi.zig` |
| `jiff` | Local wall-clock time, IANA tzdb | `tz.zig` (289 lines of TZif parsing) |
| `clap` (derive) | Argument parsing | hand-rolled `parseArgs` |
| `thiserror` | Per-module error enums | Zig's named error sets |
| `anyhow` | Top-level error reporting in `main` | manual stderr + `process.exit` |

Dev-dependency: `tempfile`, replacing `std.testing.tmpDir`.

Versions are whatever `cargo add` resolves at implementation time; pin them in
`Cargo.lock` and commit it (this is a binary, not a library).

`jiff` was chosen over `chrono` because it reads `/etc/localtime` and
`/usr/share/zoneinfo` in pure Rust — a true 1:1 replacement for what `tz.zig`
does by hand — and because its Temporal-derived API makes it hard to confuse a
civil time with an absolute instant. It is confined to one function in
`storage.rs`; swapping it out later is a ten-line change.

---

## Module map

```
src/
  main.rs      clap parsing, XDG dir setup, dispatch, anyhow error report
  tui.rs       crossterm raw mode / alt screen / mouse, RAII guard + panic hook
  app.rs       event loop; owns cursors, scroll, focus, mode, input buffer
  session.rs   Session + Task + Timestamp + Pane; create/load/save; mutations
  markdown.rs  parse / serialize
  storage.rs   XDG resolution, file stem, collision suffix, jiff -> Timestamp
  ui.rs        two-pane frame, header, status bar, input bar, help overlay, palette
  picker.rs    -r screen: scan, count, filter, selection loop
```

Eight files against ava's eleven. `ansi.zig` (ratatui emits escapes),
`input.zig` (crossterm decodes events), and `tz.zig` (jiff) have no Rust
counterpart; `terminal.zig` shrinks into `tui.rs`.

Each module keeps one clear job and is testable on its own: `markdown` is pure
bytes-in/data-out, `storage` splits pure formatting from the one filesystem
function, `ui` is a pure function from state to a rendered buffer, and `app`
owns mutation without touching the terminal directly.

---

## Data model

```rust
pub struct Task {
    pub text: String,
    pub done: bool,
}

pub struct Timestamp {
    pub year: u16, pub month: u8, pub day: u8,
    pub hour: u8, pub minute: u8,
}

pub enum Pane { Active, Completed }

pub struct Session {
    pub filename: String,
    pub timestamp: Option<Timestamp>,
    pub active: Vec<Task>,
    pub completed: Vec<Task>,
    pub dirty: bool,
}
```

Tasks are capped at 150 bytes of UTF-8 (`MAX_TASK_BYTES`), as in ava. When
truncating at that cap, cut on a UTF-8 character boundary rather than
mid-sequence.

### What Rust changes

These are the intended teaching moments, and they should be called out in code
comments where they occur:

- **`String` owns its bytes.** Every `Task.deinit`, every
  `errdefer allocator.free`, the `allocator` field on `Session`, and all of
  `Parsed.deinit` are deleted. `Drop` handles it.
- **`Session::toggle` stops being fallible.** `session.zig:82-98` un-removes
  the task if `append` fails, then frees the text if the rollback also fails.
  `Vec::push` doesn't return a `Result`, so that entire branch vanishes.
- **`Timestamp` stays a plain struct**, converted from `jiff::Zoned` in one
  `storage.rs` function. jiff never leaks past that boundary, so `markdown.rs`
  and the header formatting are a line-for-line port of the Zig.
- **`Tui` implements `Drop`**, replacing `defer term.restore()`, plus a panic
  hook that restores the terminal *before* printing the panic — a failure mode
  ava cannot fully prevent.

---

## File format

Unchanged from ava, and deliberately tool-agnostic (the header says
`# Session`, not the program name):

```markdown
# Session 2026-05-31 14:30

## Active

- [ ] buy milk
- [ ] walk dog

## Completed

- [x] write the README
```

Parser rules, ported exactly:

- `# Session YYYY-MM-DD HH:MM` sets the timestamp. Malformed or out-of-range
  (month 0 or >12, day 0 or >31, hour >23, minute >59) leaves it `None`
  rather than failing the parse.
- `## Active` / `## Completed` switch which list subsequent tasks join.
- `- [ ] text` and `- [x] text` are tasks. Text over 150 bytes is
  `ParseError::LineTooLong`.
- Tasks before any heading default to Active.
- CRLF is tolerated (trailing `\r` trimmed).
- Every other line is silently ignored.

Serialization is canonical and byte-stable, so parse → serialize → parse is a
fixed point:

```
# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] first\n\n## Completed\n\n- [x] third\n
```

With no timestamp, the header line is `# Session` alone.

---

## Storage

`$XDG_DATA_HOME/caleb`, falling back to `$HOME/.local/share/caleb`. Neither
set is a fatal startup error. **Its own directory — caleb does not read or
write ava's session files.**

Filenames are `YYYY-MM-DD_HH-MM.md`, zero-padded throughout. On collision,
append `-2`, `-3`, … until a name is free (check-then-create; the TOCTOU race
is out of scope for a single-user local CLI).

Resuming a session **renames** its file to the current timestamp. The old name
goes away, contents travel with the rename, and the in-memory header timestamp
is updated to match.

---

## CLI

```
caleb                Start a new session named for the current date/time
caleb -r, --resume   Resume a past session via interactive picker
caleb --list         Print all saved sessions to stdout and exit
caleb -h, --help     Show help and exit
caleb -v, --version  Show version and exit
```

`caleb <path>` is intentionally unsupported — past sessions open only through
`-r`, which renames to now() so resumed work continues under a fresh stamp.

`-v` must be overridden in clap, which defaults to `-V`.

Pre-TUI failures print a short stderr message and exit non-zero: 2 for usage
errors (clap's default, matching `main.zig:73`), 1 for storage and TTY
problems. The binary refuses to run when stdin or stdout is not a TTY.

`--list` prints `<name>   <n> open / <n> total` per session, newest first.

---

## Rendering

### Layout

Rows, top to bottom:

```
row 1              header
row 2              pane top borders
rows 3..rows-2     pane content (Active left, Completed right)
row rows-1         pane bottom borders
row rows           status bar
```

`Layout::vertical([Length(1), Min(0), Length(1)])`. In add-task mode the
bottom block becomes `Length(4)` — a 3-row bordered field plus one blank
spacer row, and the pane bottom borders slide up to `rows - 4`. The spacer is
deliberate: it keeps the field's bottom border clear of tmux/powerline status
lines that overlay the last row.

Panes split with `Length(cols / 2)` + `Min(0)`, **not** two `Percentage(50)`s,
so an odd column lands in the right pane exactly as `render.zig:172-174` does.

Below 8 rows or 30 columns, draw only `caleb: terminal too small`.

### Chrome

- `Block` with `BorderType::Rounded` for `╭ ╮ ╰ ╯ ─ │`.
- Pane titles are the string `"─ Active "` / `"─ Completed "`; ratatui starts
  titles one cell past the corner, reproducing ava's `╭─ Active ───╮`.
- Header: ` caleb · YYYY-MM-DD HH:MM · N active / N done`, bold, plus
  `  •unsaved` in `warn` when dirty.
- Status bar: ` a add  d delete  space toggle  J/K move  s save  q quit  ? help`,
  dim.
- Completed tasks carry a `✓ ` marker; active tasks a two-space indent that
  keeps text aligned across panes. The cursor row is reverse video — the
  active pane has no per-row glyph.
- Completed tasks render dim + strikethrough.
- Task text is truncated to the pane's inner width. ratatui truncates by
  display width, which fixes ava's byte-truncation (`render.zig:347`) splitting
  multi-byte characters into mojibake.

### Task spacing

Each task occupies two rows: a blank spacer, then the content row. The spacer
comes first so the first item gets the same gap from the top border that
separates adjacent items. `visible_tasks = inner_rows / 2`.

Selection styling is applied per-`Line` when building `ListItem`s, **not** via
`highlight_style` — a two-`Line` item would otherwise paint the spacer row
reversed too, giving a 2-row highlight where ava has 1.

### Scroll

ava's explicit `active_scroll` / `completed_scroll` offsets and its
`clamp_pane` math are kept, and the result is written into `ListState`'s
offset. Scrolling is not delegated to `ListState` because the mouse wheel
scrolls the focused pane *without* moving the cursor (`app.zig:275-283`),
which `ListState` alone cannot express.

Clamp rules: keep the cursor visible (scroll down to `cursor - inner + 1`, up
to `cursor`), and never scroll past `len - inner`.

### Palette

A single `Palette` struct, mirroring `render.zig:39-50` so retuning is a
one-line change per slot:

| Slot | Index | Use |
|---|---|---|
| `accent` | 141 (MediumPurple1) | focused pane border, input box border |
| `muted` | 240 (dim gray) | unfocused pane borders |
| `help` | 177 (orchid) | help overlay border |
| `warn` | 221 (gold) | unsaved marker |

### NO_COLOR

When `NO_COLOR` is set to a non-empty value, the four palette colors resolve
to `Color::Reset`, but `BOLD`, `REVERSED`, `DIM`, and `CROSSED_OUT` are
**retained**.

This intentionally diverges from ava's runtime behavior. `render.zig` gates
every attribute on `color_enabled` (`:333` reverse, `:147` bold, `:368` dim),
so `NO_COLOR=1` leaves ava with no visible cursor and no focus indicator.
ava's own `AGENTS.md` states the opposite intent — "the app stays usable via
`▸`, `✓`, `[ ]`, bold, and reverse video" — and bold/reverse/dim are not
colors, so no-color.org does not ask for them to be dropped. caleb follows
ava's documented intent over its code.

### Overlays

**Add-task field** — three rows plus a blank spacer:

```
row rows-3   ╭─ Add task ──...──╮
row rows-2   │ > <buf>_<padding>│
row rows-1   ╰──────...─────────╯
row rows     (blank)
```

Accent-colored border, bold `> ` prompt, `_` cursor after the typed text.

**Help overlay** — centered, `Clear` beneath it, orchid border, one row of
vertical and one column of horizontal padding between text and border. Inner
width stays ava's 52 columns; height sizes to the content, which is ava's
reference plus a new `Mouse` section (so no longer ava's fixed 19 lines). The
overlay is skipped entirely if the terminal cannot fit it with a 1-cell margin
on every side.

---

## Input

### Keys

```
Navigation   j/k or ↑/↓    h / ← → / l  switch pane    g / G  top / bottom
Editing      a add   d delete   space|x toggle   Shift+J/K reorder
App          s save   q quit (auto-saves)   ? help overlay   Esc cancel
```

In help mode, any real key dismisses (resize and unknown keys are ignored).
In add-task mode: printable text appends, Backspace pops a whole UTF-8
character, Enter commits a non-empty buffer as a new Active task, Esc cancels
and discards. Control bytes other than tab are rejected from the buffer.

Quitting auto-saves when dirty.

### Mouse

ava had wheel-scroll only. caleb adds:

| Action | Effect |
|---|---|
| Wheel up/down | Scroll the focused pane, cursor unmoved (ava's behavior) |
| Left click on a task | Select it; focus follows to its pane |
| Left click elsewhere in a pane | Focus that pane, cursor unmoved |
| Double click on a task | Toggle done (moves it across panes) |
| Click in the picker | Select that session row |
| Double click in the picker | Open that session |

Hit-testing needs pane geometry, which ratatui computes inside the draw
closure — so `ui::draw` records the pane `Rect`s back into `App` each frame,
and events consult the most recent frame's layout.

The coordinate math falls out of the 2-row stride:
`task_idx = scroll + row_offset / 2`. Integer division maps row offsets 0 and
1 to task 0, 2 and 3 to task 1 — so a spacer click selects the task it belongs
to with no special case. An index past the end of the list is a focus-only
click. The picker uses one row per entry starting at row 3
(`picker.zig:224`), so its math is `idx = row - 3`.

Double-click is `last_click: Option<(Instant, Pane, usize)>` on `App`; the
same pane and index within 400 ms toggles. Only `MouseEventKind::Down(Left)`
triggers selection, so press-and-release does not fire twice.

A click dismisses the help overlay exactly as any key does. Mouse events are
ignored while the add-task field is open.

Capturing the mouse disables the terminal's native click-drag text selection
inside the app; `Shift`+drag overrides it in most terminals. ava already made
this tradeoff by enabling `?1000h`.

Resize arrives as `Event::Resize` — no `SIGWINCH` handler, no atomic flag, no
`EINTR` retry loop (`input.zig:54-69`, `terminal.zig:106-117`).

---

## Resume picker

Same alt-screen idiom as the main app. Scans `*.md` in the storage directory
at open time, counts `- [ ]` / `- [x]` lines per file, sorts newest-first
(lexical descending, which the filename format makes equivalent).

Default filter shows only sessions with at least one unfinished task; `a`
toggles show-all. If the filtered list is empty, it flips to show-all
automatically rather than presenting an empty box.

`Enter` (or double-click) opens the selected file, triggering the
rename-to-now(). `Esc` or `q` exits the program — it does **not** fall through
to creating a new session.

Rows render as `YYYY-MM-DD  HH:MM   N open / N total`, with any `-2` collision
suffix preserved; names that don't match the format print verbatim.

Status bar: ` j/k move   Enter open   a show all   Esc cancel`.

---

## Error handling

`thiserror` enums per module, mirroring ava's small named error sets:

- `markdown::ParseError::LineTooLong`
- `storage::ResolveError::NoStorageDir`
- `session::LoadError` / `session::SaveError`, wrapping `io::Error` via
  `#[from]`

`main` returns `anyhow::Result<()>` and attaches `.context()` for the friendly
messages ava writes by hand — storage directory unopenable, stdin/stdout not a
TTY, session file unreadable.

clap owns the bad-flag message. ava prints `ava: unknown option '--bogus'` /
`try 'ava --help'`; clap prints its own format with a did-you-mean suggestion.
Both exit 2.

---

## Testing

In-file `#[cfg(test)] mod tests` blocks, the direct analog of ava's in-file
`test "..."` blocks, so the layout stays recognizable next to the Zig. Run
with `cargo test`.

ava's 70 tests do not all have counterparts: its `ansi`, `input`, `terminal`,
and `tz` tests cover machinery that crossterm, ratatui, and jiff now own, and
those crates carry their own suites. Every test of caleb's *own* logic ports
over:

- **markdown** — empty input, header parsing, malformed and out-of-range
  headers, pane classification, orphan tasks, ignored lines, `LineTooLong`,
  CRLF, both serialize shapes, and the byte-exact round-trip.
- **storage** — epoch and leap-day timestamp conversion, negative clamp, file
  stem padding, all four XDG resolution cases, and unique-filename collision
  and gap behavior against a `tempfile` directory.
- **session** — add/delete counts, toggle across panes, the 150-byte cap, and
  save→load round-trip.
- **app** — quit, bounded cursor movement, toggle, entering add mode, input
  append, UTF-8-aware backspace, `J` reorder, help enter/dismiss.

Rendering tests get stronger than ava's. ratatui's `TestBackend` renders into
a real `Buffer`, so instead of substring probes
(`indexOf(out, "Active") != null`) the tests assert exact row contents and
exact per-cell styles. That is how "aesthetically identical" is verified
rather than asserted.

New tests beyond ava's set:

- Mouse hit-testing: content row, spacer row, past-end-of-list, wrong pane,
  and the picker's row math.
- Double-click timing: inside and outside the 400 ms window, and on a
  different index.
- `NO_COLOR` retains `REVERSED`/`BOLD`/`DIM` while dropping indexed colors.
- UTF-8-safe truncation at the 150-byte cap and at pane width.

---

## Deliverables

- `caleb` binary, installable via `cargo install --path .`
- `README.md` — ported from ava's, adjusted for Rust, cargo, and the mouse
- `AGENTS.md` — orientation for future agents, mirroring ava's
- `LICENSE` — MIT, matching ava
- `Cargo.lock` committed

---

## Divergences from ava

Everything caleb does differently, in one place. Each is deliberate.

| # | Divergence | Why |
|---|---|---|
| 1 | Mouse click-to-select, click-to-focus, and double-click-to-toggle | New feature requested for caleb; ava has wheel-scroll only |
| 2 | `NO_COLOR` keeps `BOLD`/`REVERSED`/`DIM`/`CROSSED_OUT` | ava's code drops them, leaving no visible cursor; its own docs say otherwise |
| 3 | Task text truncates on character boundaries, not bytes | `render.zig:347` and `session.zig:124` split multi-byte UTF-8 into mojibake |
| 4 | clap's bad-flag message replaces ava's hand-written one | Using clap at all implies its error format; exit code 2 is unchanged |
| 5 | Storage is `$XDG_DATA_HOME/caleb`, not `.../ava` | Both binaries can run side by side without fighting over files |
| 6 | Help overlay gains a `Mouse` section | Documents divergence 1 |
| 7 | Terminal restore via `Drop` + panic hook | `Drop` is Rust's `defer`; the panic hook covers a case ava cannot |
| 8 | Rendering is diffed, not full-repaint | ratatui diffs by design; `AGENTS.md:203` notes ava chose full repaint deliberately, but this is invisible to the user |

Everything else — layout, palette, glyphs, key bindings, file format,
filenames, picker behavior, CLI surface, exit codes — matches ava.

## Out of scope

Carried over from ava's deferred list, plus this port's own exclusions:

- Configurable storage path via flag
- man page
- Picker sort options
- Preserving unknown markdown lines on load
- Search across sessions
- Drag-to-reorder with the mouse
- Importing or reading ava's session directory
- Per-task coloring — only structural chrome and state-based styling
- macOS / BSD support (crossterm makes it plausible, but it stays untested)

---

## Open questions

None. All design decisions are settled above.
