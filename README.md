<picture>
  <source media="(prefers-color-scheme: dark)" srcset="./assets/logo-mark-dark.svg">
  <img src="./assets/logo-mark-light.svg" width="128" alt="caleb logo">
</picture>

# caleb

Named for the character **Caleb Smith**, played by [Domhnall Gleeson](https://en.wikipedia.org/wiki/Domhnall_Gleeson), in
[Ex Machina](<https://en.wikipedia.org/wiki/Ex_Machina_(film)>) — A film by
[Alex Garland](https://en.wikipedia.org/wiki/Alex_Garland).

<br>

<p align="center">
  <img src="assets/caleb.jpg" alt="Caleb Smith in Ex Machina" width="700">
</p>

> Header image: _Ex Machina_ (2014), dir. Alex Garland — stills by Colin
> Field / Aimee Spinks. © Universal Pictures International / A24. Not
> covered by this project's license.

<br>

---

**caleb** is a TUI task manager for coding sessions. One file per
session, stored as plain GitHub-flavored markdown so it's useful in any
editor or renderer. Linux, terminal, mouse-aware.

Tasks sit in two panes side by side — Active and Completed — and checking one
off moves it across.

## Why on earth another to-do list?

**Of all things, right?**

Because it is a fun, personal Rust based project and it is useful to me. Maybe other people might find it useful and I offer it here in that spirit. That, plus I love a certain cool movie and I wanted a piece of software named after a character in it. This fit the bill :fire:

Using this thing is fun and helpful for me, no doubt :100:

## Install

Requires Rust 1.90 or newer (edition 2024).

```sh
git clone git@github.com:papadavis47/caleb.git
cd caleb
cargo install --path .
```

## Use

```sh
caleb              # start a new session, named for the current time
caleb -r           # resume a past session (interactive picker)
caleb --list       # print all sessions to stdout
caleb --clean      # delete sessions with no open tasks (asks first)
caleb --help
caleb -v           # version
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
Press `d` to delete the highlighted session — it asks `y/n` first, and the file
is removed from disk for good.

Press `p` in a session to pull unfinished work forward. Choose a past session,
then tick which of its open tasks you want; they arrive in your Active pane and
are checked off in the session they came from. Drain a session completely and it
drops to zero open tasks, so `caleb --clean` can then sweep it. A pull writes
both files itself — the current session first, then the one the tasks came
from — so nothing is left unsaved.

`caleb --clean` clears those finished sessions off disk. It lists every
session with no open tasks — including ones that never got a task — and
deletes them only after you answer `y`. It must be used on its own; pairing
it with another flag is a usage error.

`--clean` and the picker's `d` are the only two things in caleb that delete a
file, and both ask first.

## Keys

Press `?` inside the app for the same reference on screen.

### In a session

| Key               | Action                                                  |
| ----------------- | ------------------------------------------------------- |
| `j` `k` / `↓` `↑` | Move the cursor within the focused pane                 |
| `h` / `←`         | Focus the Active pane                                   |
| `l` / `→`         | Focus the Completed pane                                |
| `g` / `G`         | Jump to the top / bottom of the pane                    |
| `a`               | Add a task — opens the input field at the bottom        |
| `d`               | Delete the selected task                                |
| `space` / `x`     | Toggle done, moving the task to the other pane          |
| `J` / `K`         | Move the selected task down / up                        |
| `p`               | Pull open tasks from a past session — writes both files |
| `s`               | Save                                                    |
| `q`               | Quit — saves first if there are unsaved changes         |
| `?`               | Help overlay; any key dismisses it                      |

**While the add-task field is open:** type the task, `Backspace` deletes a
character, `Enter` commits it to Active, `Esc` cancels and discards. Tasks are
capped at 150 bytes; the view scrolls so the caret stays visible.

### Mouse

| Action       | Effect                                                 |
| ------------ | ------------------------------------------------------ |
| Wheel        | Scroll the focused pane — the cursor stays where it is |
| Click        | Select a task and focus its pane                       |
| Double-click | Toggle that task done                                  |

Capturing the mouse disables the terminal's own click-drag selection inside
the app; hold `Shift` while dragging to get it back.

### Resume picker — `caleb -r`

| Key                 | Action                                           |
| ------------------- | ------------------------------------------------ |
| `j` `k` / `↓` `↑`   | Move between sessions                            |
| `Enter`             | Open the selected session (renames it to now)    |
| `d`                 | Delete it — asks `y`/`n` first                   |
| `p`                 | Show / hide the preview pane                     |
| `Ctrl-d` / `Ctrl-u` | Scroll the preview half a page                   |
| `a`                 | Show all sessions, not just ones with open tasks |
| `Esc` / `q`         | Quit without opening anything                    |

Click a row to select it, double-click to open it.

### Pull — `p` in a session

Two steps. First choose a session:

| Key               | Action                              |
| ----------------- | ----------------------------------- |
| `j` `k` / `↓` `↑` | Move between sessions               |
| `Enter`           | Choose this one and go to its tasks |
| `Esc` / `q`       | Cancel                              |

Then choose which of its open tasks come across — all start ticked:

| Key               | Action                                  |
| ----------------- | --------------------------------------- |
| `j` `k` / `↓` `↑` | Move between tasks                      |
| `space`           | Tick / untick the task under the cursor |
| `a`               | Tick all / none                         |
| `Enter`           | Pull the ticked tasks                   |
| `Esc`             | Back to the session list                |
| `q`               | Cancel the whole thing                  |

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

The chrome uses four 256-color slots: blue `33` for the focused pane border
and the add-task field, dim gray `245` for unfocused panes, orchid `177` for
the help overlay, and gold `221` for the unsaved marker.

## Platform

Linux only. macOS/BSD might work but aren't tested.

## License

MIT.
