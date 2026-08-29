import os, pty, sys, select, time, fcntl, termios, struct
# KEYS: \n-separated chunks, written one at a time with a pause between, so
# readline has re-armed before the next arrives (a real user is not this fast).
chunks = os.environ.get("KEYS", "").encode().decode("unicode_escape").split("\x00")
cmd = sys.argv[1:]
pid, fd = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-256color"
    os.execvp(cmd[0], cmd)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))

buf = b""
def drain(seconds):
    global buf
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                c = os.read(fd, 65536)
            except OSError:
                return False
            if not c:
                return False
            buf += c
    return True

drain(2.5)
for c in chunks:
    os.write(fd, c.encode())
    if not drain(2.0):
        break
drain(2.5)
sys.stdout.write(buf.decode(errors="replace"))
