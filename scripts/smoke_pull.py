#!/usr/bin/env python3
"""Drive `p` through a pty: pick a session, deselect a task, pull the rest.

The unit tests cover every transition in `PullState`; what only a pty can show
is that the key reaches the state machine at all, that the screens actually
render, and that the pulled tasks are on the session screen afterwards.

Fixtures use single-token task text on purpose. ratatui writes only the cells
that changed, so a phrase with spaces arrives split by cursor-position escapes
and will not match as a contiguous substring.

The same effect shows up in the pull screens' own chrome: "N open" and
"Enter pull N" are single format strings, but the space inside each one can
land on a cell that already matched the previous frame, so it is never
retransmitted — even a resize-triggered full repaint diffs against a reset
buffer whose default cell is an unstyled space, so a *plain* space at that
same spot still counts as unchanged and stays unsent.

Two different fixes for two different rows, chosen after checking both by
hand against the raw pty output:

- The hint line ("Enter pull N") carries `Modifier::DIM` on the whole line
  (see `pull.rs::draw`). That modifier differs from the reset buffer's
  default style even where the character is a plain space, so
  `scripts/smoke_picker.py`'s resize trick (`repaint()` below) genuinely
  forces the whole line to be retransmitted, and the literal phrase can be
  asserted as written in the brief.
- The session-row list ("N open") has no such styling — it is a bare `List`
  with no `highlight_style` — so its interior spaces stay indistinguishable
  from the reset buffer's blank default even after a resize; `repaint()`
  does not help there. `visible()` strips escape codes so the digit and the
  word can be checked as adjacent regardless of whether that particular byte
  was retransmitted. Never use `visible()` on fixture task text — the same
  stripping can just as easily glue two unrelated rows together.
"""
import os, pty, fcntl, termios, struct, select, time, pathlib, shutil, re

ANSI_CSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")


def visible(text):
    """`text` with cursor/style escape codes stripped — see module docstring."""
    return ANSI_CSI.sub("", text)


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


_cols = [80]


def repaint():
    """Force a full redraw by actually resizing, and return the whole frame.

    A resize is the one event guaranteed to invalidate ratatui's previous
    buffer, so the next frame repaints every cell whose value differs from
    the reset buffer's blank default — which, for a *styled* line, includes
    its interior spaces (see module docstring). Toggles between 80 and 79
    columns so each call is a genuine change; `src/pull.rs`'s `draw` has no
    width-dependent branch, so the text itself does not move.
    """
    _cols[0] = 79 if _cols[0] == 80 else 80
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, _cols[0], 0, 0))
    return drain()


drain()

# --- stage one -----------------------------------------------------------
stage1 = send(["p"])
assert "pull open tasks from" in stage1, f"'p' should open the pull screen:\n{stage1}"
assert "2026-05-30" in stage1, f"the session must be listed:\n{stage1}"

# The session-row list carries no style, so a resize-forced repaint would
# not help here (checked by hand) — use the escape-stripped view instead.
vis1 = visible(stage1)
assert "2 open" in vis1 or "2open" in vis1, f"with its pullable count:\n{stage1}"

# --- stage two -----------------------------------------------------------
stage2 = send(["\r"])
assert "alphatask" in stage2, f"its open tasks must be listed:\n{stage2}"

# The hint line is uniformly DIM, so this repaint really does retransmit
# every cell, interior spaces included — assert the brief's literal phrase.
full2 = repaint()
assert "Enter pull 2" in full2, f"all selected by default:\n{full2}"

send([" "])

full3 = repaint()
assert "Enter pull 1" in full3, f"space should deselect:\n{full3}"

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
