# caleb

Named for Caleb Smith in
[Ex Machina](<https://en.wikipedia.org/wiki/Ex_Machina_(film)>) — A film by
Alex Garland.

<br>

<p align="center">
  <img src="assets/caleb.jpg" alt="Caleb Smith in Ex Machina" width="700">
</p>

<br>

---

**caleb** is a simple TUI to-do list for coding sessions. One file per
session, stored as plain GitHub-flavored markdown so it's useful in any
editor or renderer. Linux, terminal, mouse-aware.

```
╭─ caleb · 2026-05-31 14:30 · 3 active / 2 done ────────────────╮
│ ╭─ Active ───────────────╮ ╭─ Completed ────────────────────╮ │
│ │ ▸ Wire up the parser   │ │ ✓ Sketch the data model        │ │
│ │   Add render snapshots │ │ ✓ Pick a directory layout      │ │
│ │   Implement -r picker  │ │                                │ │
│ ╰────────────────────────╯ ╰────────────────────────────────╯ │
│  a add  d delete  space toggle  J/K move  s save  q quit  ?   │
╰───────────────────────────────────────────────────────────────╯
```

## Install

Requires Rust 1.97 (edition 2024).

```sh
cargo install --path .
```

## Use

```sh
caleb              # start a new session, named for the current time
caleb -r           # resume a past session (interactive picker)
caleb --list       # print all sessions to stdout
caleb --help
```

Sessions live in `$XDG_DATA_HOME/caleb` (default `~/.local/share/caleb/`),
named `YYYY-MM-DD_HH-MM.md`.

Resuming a past session **renames** the file to the current timestamp —
the old name goes away. Contents travel with the rename.

## Keys

```
Navigation   j/k or ↑/↓    h / ← → / l  switch pane    g / G  top / bottom
Editing      a add   d delete   space|x toggle   Shift+J/K reorder
App          s save   q quit (auto-saves)   ? help overlay   Esc cancel
Mouse        wheel scrolls the focused pane
             click selects a task and focuses its pane
             double-click toggles a task
```

Press `?` inside the app for the full reference.

## File format

```markdown
# Session 2026-05-31 14:30

## Active

- [ ] buy milk
- [ ] walk dog

## Completed

- [x] write the README
```

GitHub-flavored task lists. Edit the file in any editor; caleb parses
`- [ ]` / `- [x]` lines and ignores everything else. Tasks are capped at
150 bytes of UTF-8 text, truncated on a character boundary so multi-byte
text never splits.

## Environment

- `$XDG_DATA_HOME` — override the storage directory.
- `$NO_COLOR` — disables all color output ([no-color.org](https://no-color.org)).
  Bold, dim, reverse, and strike-through are kept, so the cursor stays
  visible without color.

## Platform

Linux only. macOS/BSD might work but aren't tested.

## License

MIT.
