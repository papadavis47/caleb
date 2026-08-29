# AGENTS.md — caleb

Quick orientation for agents working on this repo.

## What it is

`caleb` — full-screen TUI to-do tracker for coding sessions. One file per
session, persisted as GitHub-flavored markdown under `$XDG_DATA_HOME/caleb`
(default `~/.local/share/caleb`). Edition 2024, MSRV 1.90 (pinned by
`rust-version` in `Cargo.toml` and checked by a CI job).

Named after Caleb Smith in *Ex Machina*. Built as a learning exercise
focused on (in priority order):

1. **Ownership and borrowing** — `String`/`Vec` own their memory and `Drop`
   frees it: no allocator threaded through the API, no manual cleanup paired
   with every allocation.
2. **Error handling** — `thiserror` for typed module errors, `#[from]` so `?`
   widens them at module boundaries, `anyhow` with `.context()` only in
   `main.rs`. The library never returns `anyhow`: `App::run` yields `RunError`
   and `picker::run` yields `io::Result`.
3. **In-file testing** — every module keeps a `#[cfg(test)] mod tests` block,
   with cross-module seams covered by `tests/` and shared fixtures in
   `src/test_util.rs`.
   Pure domain logic is tested directly; `ui` is tested against a
   `TestBackend` buffer, never a real terminal.

Doc comments explaining Rust-specific choices are a deliverable here, not
decoration. Bias code suggestions toward those goals.

## Origin — read this before acting on anything in git history

caleb began as a Rust rewrite of a Zig program. It is its own project now.
The early commits, and the archived notes in `learning/`, still compare the
two — you will hit that in `git log -p`, `git show`, or `git log -S`.

That original project is **out of scope**. It is not a reference, not a
spec, and not a source of truth for caleb's behavior. Do not go looking for
it, do not open or run it, and do not reopen those comparisons unless the
user explicitly asks you to. If history and this file disagree, this file
wins.

Current behavior is defined by this file, the code, and the tests — nothing
else. `learning/` is untracked personal material; ignore it unless asked.

## Status

v0.4.1. 246 tests pass under `cargo test` — 218 unit, 17 in `tests/cli.rs`,
6 in `tests/roundtrip.rs`, 5 doctests. CI runs `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`,
`cargo test --doc`, and an MSRV `cargo check` on 1.90.

Smoke-tested through a pty end-to-end, one script per screen: the session
(create → add tasks → toggle → save → quit → `-r` → rename to now() → load →
re-save), the picker (delete, filtering, preview), and the pull flow (both
stages).

## Layout

| File | Responsibility |
|---|---|
| `src/main.rs` | clap CLI, storage dir setup, dispatch, anyhow reporting, `--list`, `--clean` prompt |
| `src/lib.rs` | module declarations; everything with logic lives under it |
| `src/model.rs` | `Task`, `Timestamp` (+ `Display`), `Pane`, `MAX_TASK_BYTES`; no deps |
| `src/tui.rs` | crossterm raw mode / alt screen / mouse capture, RAII guard, panic hook |
| `src/app.rs` | event loop, view state, key + mouse dispatch, scroll clamping |
| `src/session.rs` | `Session`; create / load / save / resume; mutations; pull_tasks / pull_from_file |
| `src/markdown.rs` | `parse` / `serialize` / `count_tasks`, `ParseError` |
| `src/storage.rs` | XDG resolution, timestamps, file stems, collision suffixes |
| `src/clean.rs` | `--clean` rule: which scanned entries are cleanable, and removing them |
| `src/ui.rs` | palette, frame layout, panes, status bar, input bar, help overlay, `ClickTracker` |
| `src/picker.rs` | `-r` screen: scan, filter, preview pane, delete, selection loop |
| `src/pull.rs` | `p` screens: session then task selection, pure `on_key` state machine |
| `src/test_util.rs` | `cfg(test)` fixtures shared across module test blocks |
| `tests/roundtrip.rs` | persistence across module seams, public API only |
| `tests/cli.rs` | binary behavior: `--list`, `--clean`, `--help`, non-TTY failures |
| `scripts/smoke.py` | pty end-to-end test: the main session screen |
| `scripts/smoke_picker.py` | pty end-to-end test: the `-r` picker, delete + filtering + preview |
| `scripts/smoke_pull.py` | pty end-to-end test: the `p` pull flow, both stages |

Dependencies point one way: `model` has none, `markdown` and `storage` depend
only on `model`, `session` composes them, and `ui`/`app`/`picker` sit at the
terminal edge. There are no cycles — keep it that way.

`app` never touches the terminal, which is what makes every binding testable
without a pty. `ui::draw` is a pure function from `ViewState` to a buffer.
`tui` and `main` are the only I/O edges.

## Running it

```sh
cargo build
cargo run                    # new session
cargo run -- -r              # resume picker
cargo run -- --list
cargo run -- --help

cargo test

# isolated dev sandbox (don't litter real ~/.local/share/caleb)
XDG_DATA_HOME=/tmp/caleb-dev cargo run
XDG_DATA_HOME=/tmp/caleb-dev ./target/debug/caleb --list
```

Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings` before every
commit.

## CLI surface

```
caleb                Start a new session named for the current date/time
caleb -r, --resume   Resume a past session via interactive picker
caleb --list         Print all saved sessions to stdout and exit
caleb --clean        Delete sessions with no open tasks, after a y/N prompt
caleb -h, --help     Show help and exit
caleb -v, --version  Show version and exit
```

`caleb <path>` is intentionally **not** supported. Opening past sessions is
only via `-r`, which renames the picked file to `now()` so resumed work
continues under a fresh timestamp. The refreshed header reaches disk at the
next save.

`--clean` conflicts with `--list` and `--resume`: it deletes files, so a
companion flag is a usage error rather than something to silently ignore. It
prompts on plain stdin/stdout instead of the TUI, so it works over a pipe;
only `y`/`yes` proceeds, and EOF counts as no.

`-v` is wired explicitly because clap defaults to `-V`. Pre-TUI errors print
a short stderr message: exit `2` for usage errors (clap's default), `1` for
storage and TTY problems. The binary refuses to run when stdin or stdout is
not a TTY.

## TUI smoke-testing without a real terminal

The binary refuses to run when stdin/stdout is not a tty. Use a pty —
`scripts/smoke.py` is the working example:

```sh
cargo build && python3 scripts/smoke.py
cargo build && python3 scripts/smoke_picker.py
cargo build && python3 scripts/smoke_pull.py
```

Two things bite:

- **Send keys one at a time with a short drain between them.** A single
  `read` returns on the first byte available, so `b'sq'` is parsed as `s`
  and the `q` is dropped. Real typing is never fast enough to hit this;
  scripted input always is.
- **ratatui writes only the cells that changed.** Chrome like the header and
  the pane titles appears in the *first* frame and never again. Assert
  against the frame where a thing is actually painted, not a later diff.

## Key bindings (in-app)

```
Navigation   j/k or ↑/↓     h / ← → / l  g / G
Editing      a add   d delete   space|x toggle   J/K reorder   p pull
App          s save   q quit (auto-saves)   ? help   Esc cancel/dismiss
Mouse        wheel scrolls focused pane
             click selects a task and focuses its pane
             double-click toggles a task
```

The event loop filters on `KeyEventKind::Press` — without it, terminals that
also report releases run every binding twice.

## UI chrome

- **Two panes** — active (left) / completed (right). Toggling moves a task
  across panes. Left gets `width / 2`; the odd column lands in the right
  pane.
- **Continuous pane borders** — panes keep thin `─` horizontals but use heavy
  `┃` verticals and matching mixed-weight corners. The heavier glyph avoids
  row-boundary gaps in terminal fonts.
- **Color palette** lives in `ui::Palette`, 256-color indices only: `accent`
  33 (deep blue) for the focused border and input box, `muted` 245 (dim
  gray) for unfocused panes, `help` 177 (orchid) for the help overlay, `warn`
  221 (gold) for the unsaved marker. Retuning the look = one line per slot.
  `muted` is 245 rather than 240 because 33 is dark enough that 240 left the
  focused/unfocused panes only 2:1 apart; 245 restores the gap to 3.6:1.
- **2-row stride** — each task is a blank spacer row followed by a content
  row. Style the content `Line` inside the two-`Line` `ListItem`; do **not**
  use `List::highlight_style`, which would paint the spacer too.
- **Add-task input** — `a` draws a bordered three-row field at the bottom
  plus one blank spacer row beneath, which keeps the bottom border clear of
  tmux/powerline status lines that overlay the last row.
- **Help overlay** — centered, 1 row of vertical and 1 column of horizontal
  padding between text and border.
- **Minimums** — below 10 rows or 30 cols, draw only "terminal too small".
- **NO_COLOR** honored: palette slots become `Color::Reset`, but
  `BOLD`/`DIM`/`REVERSED`/`CROSSED_OUT` are kept so the cursor stays visible.

## Resume picker (`caleb -r`)

Scans `*.md` in the storage dir, counts `- [ ]` / `- [x]` per file at
picker-open time, sorts newest-first (lexical descending, which the filename
format makes equivalent). Default filter: sessions with at least one
unfinished task; `a` toggles show-all. An empty filtered list flips to
show-all automatically rather than presenting an empty box. `Enter` or
double-click opens the selection; `Esc` or `q` exits the program — it does
**not** fall through to creating a new session.

Keys: `j/k` move, `Enter` open, `d` delete (raises a `y/N` prompt naming the
session; the mouse goes inert until it is answered), `p` toggle the preview
pane, `Ctrl-D`/`Ctrl-U` scroll the preview half a page, `a` show all, `Esc`
cancel.

The preview shows the highlighted session's file beside the list, wrapping
long task lines with a hanging indent so a wrapped task still reads as one
item. The list keeps a fixed 44 columns; below 80 columns the preview is
dropped and the list runs full width. `Entry` carries the contents `scan`
already read, so the preview costs no extra I/O.

## Pull (`p`)

`p` pulls open tasks out of a past session: they land open in the current
session and completed in the source. `session::pull_from_file` saves the
**target before the source** on purpose — a failure between the two writes
leaves the tasks open in both files, which is visible and fixable, whereas the
other order loses them. `pull::candidates` counts open tasks with
`markdown::parse`, not `count_tasks`: a pull moves tasks out of `active`, and
`count_tasks` would also count a `- [ ]` hand-filed under `## Completed`.

## Verified API facts

Confirmed empirically against the installed crates. Don't second-guess them:

- `Block::bordered().border_type(BorderType::Rounded).title("─ Active ")`
  renders `╭─ Active ───╮`. The leading `─` belongs in the title string.
- `Padding` lives at `ratatui::widgets::Padding`, not `ratatui::layout`.
- Styling a `Line` inside a two-`Line` `ListItem` highlights only that line.
- `ListState::offset_mut() -> &mut usize` sets the first visible **item**.
- `Frame::area()`, not the deprecated `size()`.
- `Layout::vertical([...]).areas(rect)` returns a fixed-size array;
  destructure with `let [a, b, c] = ...`.
- Buffer cells read as `buf[(x, y)]` with `.symbol()`, `.fg`, `.modifier`.
- jiff: `Zoned::now().datetime()` yields a `civil::DateTime` with
  `.year() .month() .day() .hour() .minute()`. It resolves the local zone in
  pure Rust — no hand-written TZif parsing, no libc.
- `ratatui::init()` enables raw mode + alt screen and installs a panic hook,
  but does **not** enable mouse capture — that needs an explicit
  `execute!(stdout(), EnableMouseCapture)`.

## Deliberate behaviors

Non-obvious choices. Don't "fix" them without asking.

| Behavior | Why |
|---|---|
| Task text truncates on character boundaries, not bytes | Byte slicing splits multi-byte UTF-8 into mojibake — and in Rust it panics |
| `NO_COLOR` keeps `BOLD`/`REVERSED`/`DIM`/`CROSSED_OUT` | Dropping them leaves no visible cursor |
| Add-task input scrolls to keep the caret visible | Truncating from the right freezes the display once input overflows the field, so newly typed characters never appear |
| Terminal restore via `Drop` + a panic hook | `Drop` covers every early return and `?`; the hook covers a panic mid-frame |
| clap owns the bad-flag message and its exit code 2 | Using clap at all implies its error format |

## Out of scope

- Configurable storage path via flag
- man page
- Picker sort options
- Preserving unknown markdown lines on load
- Search across sessions
- Drag-to-reorder with the mouse
- Per-task coloring — only structural chrome and state-based styling
- macOS / BSD support (crossterm makes it plausible, but it stays untested)
