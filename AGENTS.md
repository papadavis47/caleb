# AGENTS.md — caleb

Quick orientation for agents working on this repo.

## What it is

`caleb` — full-screen TUI to-do tracker for coding sessions. One file per
session, persisted as GitHub-flavored markdown under `$XDG_DATA_HOME/caleb`
(default `~/.local/share/caleb`). Rust 1.97, edition 2024.

A port of the Zig program `ava` — functionally and visually identical, plus
mouse selection. **The reference project `~/priority-projects/zig/ava` is
read-only. Never modify it.** caleb also never reads or writes ava's storage
directory; both binaries run side by side.

Named after Caleb Smith in *Ex Machina*. Built as a learning exercise
focused on (in priority order):

1. **Ownership and borrowing** — where ava threaded an `Allocator` and paired
   every `alloc` with a `defer`, `String`/`Vec` own their memory and `Drop`
   frees it. The port's whole point is seeing what that removes.
2. **Error handling** — `thiserror` for typed module errors, `#[from]` in
   place of Zig's merged error sets, `anyhow` with `.context()` only at the
   `main` boundary.
3. **In-file testing** — every module keeps a `#[cfg(test)] mod tests` block.
   Pure domain logic is tested directly; `ui` is tested against a
   `TestBackend` buffer, never a real terminal.

Doc comments explaining Rust-specific choices are a deliverable here, not
decoration. Bias code suggestions toward those goals.

## Status

v1 complete. 104 in-file tests pass under `cargo test`. Smoke-tested through
a pty end-to-end: create session → add tasks → toggle → save → quit → `-r` →
rename to now() → load → re-save.

## Layout

| File | Responsibility |
|---|---|
| `src/main.rs` | clap CLI, storage dir setup, dispatch, anyhow reporting, `--list` |
| `src/tui.rs` | crossterm raw mode / alt screen / mouse capture, RAII guard, panic hook |
| `src/app.rs` | event loop, view state, key + mouse dispatch, scroll clamping |
| `src/session.rs` | `Task`, `Timestamp`, `Pane`, `Session`; create / load / save; mutations |
| `src/markdown.rs` | `parse` / `serialize`, `ParseError` |
| `src/storage.rs` | XDG resolution, timestamps, file stems, collision suffixes |
| `src/ui.rs` | palette, frame layout, panes, status bar, input bar, help overlay |
| `src/picker.rs` | `-r` screen: scan, count, filter, draw, selection loop |
| `scripts/smoke.py` | pty end-to-end test |

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
caleb -h, --help     Show help and exit
caleb -v, --version  Show version and exit
```

`caleb <path>` is intentionally **not** supported. Opening past sessions is
only via `-r`, which renames the picked file to `now()` so resumed work
continues under a fresh timestamp. The refreshed header reaches disk at the
next save.

`-v` is wired explicitly because clap defaults to `-V`. Pre-TUI errors print
a short stderr message: exit `2` for usage errors (clap's default), `1` for
storage and TTY problems. The binary refuses to run when stdin or stdout is
not a TTY.

## TUI smoke-testing without a real terminal

The binary refuses to run when stdin/stdout is not a tty. Use a pty —
`scripts/smoke.py` is the working example:

```sh
cargo build && python3 scripts/smoke.py
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
Editing      a add   d delete   space|x toggle   J/K reorder
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
  pane, matching ava.
- **Color palette** lives in `ui::Palette`, 256-color indices only: `accent`
  141 (light violet) for the focused border and input box, `muted` 240 (dim
  gray) for unfocused panes, `help` 177 (orchid) for the help overlay, `warn`
  221 (gold) for the unsaved marker. Retuning the look = one line per slot.
- **2-row stride** — each task is a blank spacer row followed by a content
  row. Style the content `Line` inside the two-`Line` `ListItem`; do **not**
  use `List::highlight_style`, which would paint the spacer too.
- **Add-task input** — `a` draws a bordered three-row field at the bottom
  plus one blank spacer row beneath, which keeps the bottom border clear of
  tmux/powerline status lines that overlay the last row.
- **Help overlay** — centered, 1 row of vertical and 1 column of horizontal
  padding between text and border.
- **Minimums** — below 8 rows or 30 cols, draw only "terminal too small".
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

## Verified API facts

Confirmed empirically against the installed crates. Don't second-guess them:

- `Block::bordered().border_type(BorderType::Rounded).title("─ Active ")`
  renders `╭─ Active ───╮` — ava's exact border. The leading `─` belongs in
  the title string.
- `Padding` lives at `ratatui::widgets::Padding`, not `ratatui::layout`.
- Styling a `Line` inside a two-`Line` `ListItem` highlights only that line.
- `ListState::offset_mut() -> &mut usize` sets the first visible **item**.
- `Frame::area()`, not the deprecated `size()`.
- `Layout::vertical([...]).areas(rect)` returns a fixed-size array;
  destructure with `let [a, b, c] = ...`.
- Buffer cells read as `buf[(x, y)]` with `.symbol()`, `.fg`, `.modifier`.
- jiff: `Zoned::now().datetime()` yields a `civil::DateTime` with
  `.year() .month() .day() .hour() .minute()`. This one call replaces ava's
  289-line hand-written TZif parser.
- `ratatui::init()` enables raw mode + alt screen and installs a panic hook,
  but does **not** enable mouse capture — that needs an explicit
  `execute!(stdout(), EnableMouseCapture)`.

## Divergences from ava

Everything caleb does differently. Each is deliberate.

| # | Divergence | Why |
|---|---|---|
| 1 | Mouse click-to-select, click-to-focus, and double-click-to-toggle | New feature requested for caleb; ava has wheel-scroll only |
| 2 | `NO_COLOR` keeps `BOLD`/`REVERSED`/`DIM`/`CROSSED_OUT` | ava's code drops them, leaving no visible cursor; its own docs say otherwise |
| 3 | Task text truncates on character boundaries, not bytes | `render.zig:347` and `session.zig:124` split multi-byte UTF-8 into mojibake |
| 4 | clap's bad-flag message replaces ava's hand-written one | Using clap at all implies its error format; exit code 2 is unchanged |
| 5 | Storage is `$XDG_DATA_HOME/caleb`, not `.../ava` | Both binaries can run side by side without fighting over files |
| 6 | Help overlay gains a `Mouse` section | Documents divergence 1 |
| 7 | Terminal restore via `Drop` + panic hook | `Drop` is Rust's `defer`; the panic hook covers a case ava cannot |
| 8 | Rendering is diffed, not full-repaint | ratatui diffs by design; ava chose full repaint deliberately, but this is invisible to the user |
| 9 | Add-task input scrolls to keep the caret visible | ava truncates from the right (`render.zig:421-426`), freezing the display once input overflows the field, so newly typed characters never appear. caleb scrolls the view instead. Confirmed with the user after review surfaced the difference. |

Everything else — layout, palette, glyphs, key bindings, file format,
filenames, picker behavior, CLI surface, exit codes — matches ava.

## Out of scope

- Configurable storage path via flag
- man page
- Picker sort options
- Preserving unknown markdown lines on load
- Search across sessions
- Drag-to-reorder with the mouse
- Importing or reading ava's session directory
- Per-task coloring — only structural chrome and state-based styling
- macOS / BSD support (crossterm makes it plausible, but it stays untested)
