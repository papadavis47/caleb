#!/usr/bin/env python3
"""Drive the -r picker through a pty: delete a session, and prove that doing so
does not silently unhide the finished ones.

The unhiding was a real regression: `show_all` used to be re-decided on every
frame, so deleting the last unfinished session flipped the whole list into
view. `show_all_on_open` now settles it once, and nothing here would catch it
moving back into the loop, hence this script.
"""
import os, pty, fcntl, termios, struct, select, time, pathlib, shutil

DATA = "/tmp/caleb-picker-smoke"
shutil.rmtree(DATA, ignore_errors=True)
sessions = pathlib.Path(DATA, "caleb")
sessions.mkdir(parents=True)

# One session with unfinished work, three without. Only the first is listed
# when the picker opens.
(sessions / "2026-05-31_14-30.md").write_text(
    "# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] open task\n"
)
for name in ["2026-05-28_09-00.md", "2026-05-29_09-00.md", "2026-05-30_09-00.md"]:
    (sessions / name).write_text(
        "# Session 2026-05-30 09:00\n\n## Completed\n\n- [x] done\n"
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


first_frame = drain()
assert "2026-05-31" in first_frame, f"unfinished session should be listed:\n{first_frame}"
assert "2026-05-28" not in first_frame, f"finished sessions start hidden:\n{first_frame}"
assert "d delete" in first_frame, f"hint line should advertise the key:\n{first_frame}"

prompt = send(["d"])
assert "y/n" in prompt, f"'d' should raise a confirm prompt:\n{prompt}"
assert "2026-05-31" in prompt, f"prompt should name the session:\n{prompt}"

send(["n"])
assert len(list(sessions.glob("*.md"))) == 4, "'n' must not delete anything"

after = send(["d", "y"])
left = sorted(p.name for p in sessions.glob("*.md"))
assert "2026-05-31_14-30.md" not in left, f"'y' should delete: {left}"
assert len(left) == 3, f"only the highlighted session should go: {left}"

# The regression this file exists for.
assert "2026-05-28" not in after, f"finished sessions unhid themselves:\n{after}"
assert "no unfinished sessions" in after, f"empty state should explain itself:\n{after}"
assert "3" in after, f"empty state should count what is hidden:\n{after}"

revealed = send(["a"])
assert "2026-05-28" in revealed, f"'a' should still reveal them:\n{revealed}"

send(["q"])
time.sleep(0.3)
os.waitpid(pid, 0)
print("SMOKE OK:", left)
