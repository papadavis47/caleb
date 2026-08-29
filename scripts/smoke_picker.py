#!/usr/bin/env python3
"""Drive the -r picker through a pty: delete, filtering, and the preview pane.

Two behaviours here have no other regression guard:

- Deleting the last unfinished session must not unhide the finished ones.
  `show_all` used to be re-decided every frame, which made it do exactly that.
- The preview pane appears at 80 columns and vanishes below.

Fixtures use single-token task text on purpose. ratatui writes only the cells
that changed, so a phrase with spaces in it arrives split by cursor-position
escapes and will not match as a contiguous substring.
"""
import os, pty, fcntl, termios, struct, select, time, pathlib, shutil

DATA = "/tmp/caleb-picker-smoke"
shutil.rmtree(DATA, ignore_errors=True)
sessions = pathlib.Path(DATA, "caleb")
sessions.mkdir(parents=True)

# One session with unfinished work, three without. Only the first is listed
# when the picker opens.
(sessions / "2026-05-31_14-30.md").write_text(
    "# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] alphatask\n"
)
# Long enough that the preview needs scrolling: 4 header lines + 40 tasks.
done = "".join(f"- [x] task{i:02}\n" for i in range(40))
for name in ["2026-05-28_09-00.md", "2026-05-29_09-00.md", "2026-05-30_09-00.md"]:
    (sessions / name).write_text(
        f"# Session 2026-05-30 09:00\n\n## Completed\n\n{done}"
    )

pid, fd = pty.fork()
if pid == 0:
    os.environ["XDG_DATA_HOME"] = DATA
    os.environ["TERM"] = "xterm-256color"
    os.execv("./target/debug/caleb", ["./target/debug/caleb", "-r"])

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


# The first frame carries the full screen; later reads are diffs only.
first = drain()
assert "2026-05-31" in first, f"unfinished session should be listed:\n{first}"
assert "2026-05-28" not in first, f"finished sessions start hidden:\n{first}"
assert "d delete" in first, f"hint line should advertise delete:\n{first}"
assert "p toggle preview" in first, f"hint line should advertise the preview:\n{first}"
assert "alphatask" in first, f"preview should show the highlighted file:\n{first}"

# --- delete -------------------------------------------------------------
prompt = send(["d"])
assert "y/n" in prompt, f"'d' should raise a confirm prompt:\n{prompt}"
assert "2026-05-31" in prompt, f"prompt should name the session:\n{prompt}"

send(["n"])
assert len(list(sessions.glob("*.md"))) == 4, "'n' must not delete anything"

after = send(["d", "y"])
left = sorted(p.name for p in sessions.glob("*.md"))
assert "2026-05-31_14-30.md" not in left, f"'y' should delete: {left}"
assert len(left) == 3, f"only the highlighted session should go: {left}"

# --- the list must not reorganise itself --------------------------------
assert "2026-05-28" not in after, f"finished sessions unhid themselves:\n{after}"
assert "no unfinished sessions" in after, f"empty state should explain itself:\n{after}"
assert "3" in after, f"empty state should count what is hidden:\n{after}"

revealed = send(["a"])
assert "2026-05-28" in revealed, f"'a' should still reveal them:\n{revealed}"
assert "task00" in revealed, f"preview should follow the cursor:\n{revealed}"

# --- preview scrolling ---------------------------------------------------
# The pane is 21 rows, so ctrl-d advances half a page: 10 lines. The file is
# 4 heading/blank lines then task00, so line 10 is task06 — assert on the top
# of the pane, which is the one row the diff always rewrites in full.
down = send(["\x04"])
assert "task06" in down, f"ctrl-d should scroll half a page:\n{down}"
# Back at offset 0 the file's headings are on screen again, which is the
# unambiguous marker: further down, only task digits differ between frames.
up = send(["\x15"])
assert "# Session" in up, f"ctrl-u should scroll back to the top:\n{up}"

# --- preview toggle and width threshold ---------------------------------
off = send(["p"])
assert "task00" not in off, f"'p' should hide the preview:\n{off}"
back = send(["p"])
assert "task00" in back, f"'p' should bring it back:\n{back}"

fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 70, 0, 0))
narrow = drain()
assert "task00" not in narrow, f"70 columns is too narrow for a preview:\n{narrow}"
assert "toggle preview" not in narrow, f"nor for its hint:\n{narrow}"
assert "0 open / 40 total" in narrow, f"the list must remain:\n{narrow}"

send(["q"])
time.sleep(0.3)
os.waitpid(pid, 0)
print("SMOKE OK:", left)
