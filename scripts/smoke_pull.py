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
land on a cell that already matched the previous frame (the row was blank
before the pull screen painted it), so it is never retransmitted. `visible()`
strips escape codes so those two pieces of text can be checked as adjacent
without depending on whether that particular byte happened to be resent.
"""
import os, pty, fcntl, termios, struct, select, time, pathlib, shutil, re

ANSI_CSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")


def visible(text):
    """`text` with cursor/style escape codes stripped.

    Only for checking that a couple of pieces of chrome text land in the same
    frame, not that they are visually adjacent on screen — collapsing the
    escapes can just as easily glue two unrelated cells together. Never use
    this on fixture task text; keep that single-token instead (see above).
    """
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


drain()

# --- stage one -----------------------------------------------------------
stage1 = send(["p"])
assert "pull open tasks from" in stage1, f"'p' should open the pull screen:\n{stage1}"
assert "2026-05-30" in stage1, f"the session must be listed:\n{stage1}"
vis1 = visible(stage1)
assert "2open" in vis1 or "2 open" in vis1, f"with its pullable count:\n{stage1}"

# --- stage two -----------------------------------------------------------
stage2 = send(["\r"])
assert "alphatask" in stage2, f"its open tasks must be listed:\n{stage2}"
vis2 = visible(stage2)
assert "Enter pull2" in vis2 or "Enter pull 2" in vis2, f"all selected by default:\n{stage2}"

deselected = send([" "])
# Only the changed cell is retransmitted here — "Enter pull" itself already
# matched the previous frame — so check the digit that did change instead of
# the full phrase.
vis_desel = visible(deselected)
assert "1" in vis_desel and "2" not in vis_desel, (
    f"space should deselect and drop the count to 1:\n{deselected}"
)

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
