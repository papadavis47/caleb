#!/usr/bin/env python3
"""Drive caleb through a pty: add tasks, toggle, save, quit, verify the file."""
import os, pty, fcntl, termios, struct, select, time, sys, pathlib, shutil

DATA = "/tmp/caleb-smoke"
shutil.rmtree(DATA, ignore_errors=True)

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
    return out

def send(keys):
    for k in keys:
        os.write(fd, k.encode() if isinstance(k, str) else k)
        drain(0.15)

# ratatui only writes cells that changed, so the chrome appears once — in
# the very first frame. Capture it here; later drains carry diffs only.
first_frame = drain()
send(["a"])                      # open the add field
send(list("first task"))
send(["\r"])                     # commit
send(["a"])
send(list("second task"))
send(["\r"])
send([" "])                      # toggle the selected task
send(["s"])                      # save
drain()
send(["q"])                      # quit (auto-saves)
time.sleep(0.3)
os.waitpid(pid, 0)

files = sorted(pathlib.Path(DATA, "caleb").glob("*.md"))
assert len(files) == 1, f"expected one session file, got {files}"
text = files[0].read_text()
print(text)
assert "- [x] first task" in text, "toggled task should be completed"
assert "- [ ] second task" in text, "untoggled task should stay active"
assert b"caleb" in first_frame, "header should name the app"
assert b"Active" in first_frame, "left pane should be titled Active"
assert b"Completed" in first_frame, "right pane should be titled Completed"
print("SMOKE OK:", files[0].name)
