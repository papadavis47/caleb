<img src="./assets/logo-mark-dark.svg" width="128" alt="caleb logo">

# caleb

Named for the character Caleb Smith, played by Domhnall Gleeson, in
[Ex Machina](<https://en.wikipedia.org/wiki/Ex_Machina_(film)>) — A film by
[Alex Garland](https://en.wikipedia.org/wiki/Alex_Garland).

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

Requires Rust 1.90 or newer (edition 2024).

```sh
cargo install --path .
```

## Use

```sh
caleb              # start a new session, named for the current time
caleb -r           # resume a past session (interactive picker)
caleb --list       # print all sessions to stdout
caleb --clean      # delete sessions with no open tasks (asks first)
caleb --help
```

Sessions live in `$XDG_DATA_HOME/caleb` (default `~/.local/share/caleb/`),
named `YYYY-MM-DD_HH-MM.md`.

Resuming a past session **renames** the file to the current timestamp —
the old name goes away. Contents travel with the rename.

A preview pane beside the list shows the highlighted session's file as you
move through it, so you can see what a session holds without opening it —
which matters, since opening one renames it. It needs 80 columns; below that
the list runs full width. `p` toggles it, `Ctrl-d`/`Ctrl-u` scroll it.

The picker hides sessions with no unfinished tasks; press `a` to show all.

Press `p` in a session to pull unfinished work forward. Choose a past session,
then tick which of its open tasks you want; they arrive in your Active pane and
are checked off in the session they came from. Drain a session completely and it
drops to zero open tasks, so `caleb --clean` can then sweep it. Pulling saves the
current session.

`caleb --clean` clears those finished sessions off disk. It lists every
session with no open tasks — including ones that never got a task — and
deletes them only after you answer `y`. It must be used on its own; pairing
it with another flag is a usage error.
Press `d` to delete the highlighted session — it asks `y/n` first, and the
file is removed from disk for good. Nothing else in caleb ever deletes a file.

## Keys

```
Navigation   j/k or ↑/↓    h / ← → / l  switch pane    g / G  top / bottom
Editing      a add   d delete   space|x toggle   Shift+J/K reorder
App          s save   q quit (auto-saves)   ? help overlay   Esc cancel
Mouse        wheel scrolls the focused pane
             click selects a task and focuses its pane
             double-click toggles a task

Picker       j/k move   Enter open   d delete (y/n confirm)
             p preview   Ctrl-d/Ctrl-u scroll preview
             a show all   Esc cancel
```

Press `?` inside the app for the full reference.

## File format

```markdown
# Session 2026-05-31 14:30

## Active

- [ ] refactor search component
- [ ] implement structs for scaffold.rs
- [ ] use delta to begin new project idea
- [ ] write up new PR for some recent commits to fork

## Completed

- [x] write README
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
