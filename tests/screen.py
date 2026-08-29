#!/usr/bin/env python3
"""Runs the dialog under a pty and prints the screen it ended up drawing.

Grown from `reference/ptyharness.py`, which proved two things the hard way and
both still apply: keys must arrive in **paced chunks**, because an application
in raw mode flushes pending input when it re-arms and a single burst gets
eaten; and a real Return is **CR**, not LF.

What is added here is the screen. Dumping the raw byte stream shows every
repaint interleaved, which is unreadable and untestable; this keeps a grid and
replays the escape sequences into it, so what comes out is what a person would
have been looking at when the program exited.

    KEYS='\\x1b[B\\x00\\r' tests/screen.py -- ./filepick --dialog req.json res.json
"""

import fcntl
import os
import pty
import re
import unicodedata
import select
import struct
import sys
import termios
import time

ROWS, COLS = 40, 120

# A wide character occupies two columns and leaves the second one unwritable,
# which is the distinction the dialog's own width arithmetic turns on. A grid
# that advanced one column per character would agree with a bug rather than
# catch it.
WIDE = frozenset("WF")


def cell_width(c):
    if unicodedata.combining(c):
        return 0
    return 2 if unicodedata.east_asian_width(c) in WIDE else 1


class Screen:
    """Enough of a terminal to render what ratatui emits: absolute cursor
    moves, erases, and text. Styling is dropped — the tests are about layout
    and content, and colours are checked by eye against foot.ini."""

    def __init__(self, rows, cols):
        self.rows, self.cols = rows, cols
        self.grid = [[" "] * cols for _ in range(rows)]
        self.row = self.col = 0
        self.pending = ""

    def render(self, data):
        # A read can land in the middle of an escape sequence; carrying the
        # tail over is the difference between a clean grid and stray digits
        # scattered through it.
        data = self.pending + data
        self.pending = ""
        i = 0
        while i < len(data):
            c = data[i]
            if c == "\x1b":
                consumed = self._escape(data, i)
                if consumed == 0:
                    self.pending = data[i:]
                    return
                i += consumed
                continue
            i += 1
            if c == "\r":
                self.col = 0
            elif c == "\n":
                self.row = min(self.row + 1, self.rows - 1)
            elif c == "\x08":
                self.col = max(self.col - 1, 0)
            elif c >= " ":
                w = cell_width(c)
                if self.col < self.cols and self.row < self.rows:
                    self.grid[self.row][self.col] = c
                    # The trailing half of a wide cell holds nothing, so it
                    # takes up a column without contributing a character.
                    for x in range(self.col + 1, min(self.col + w, self.cols)):
                        self.grid[self.row][x] = ""
                self.col += w

    def _escape(self, data, i):
        rest = data[i:]
        # OSC: ESC ] ... BEL or ST. Window titles land here.
        osc = re.match(r"\x1b\][^\x07\x1b]*(\x07|\x1b\\)", rest)
        if osc:
            return osc.end()
        csi = re.match(r"\x1b\[([\x30-\x3f]*)([\x20-\x2f]*)([\x40-\x7e])", rest)
        if csi:
            params, _, final = csi.groups()
            self._csi(params, final)
            return csi.end()
        # Still arriving: the whole of what is left could be the start of a
        # sequence whose final byte has not been read yet.
        if re.fullmatch(r"\x1b(\][^\x07\x1b]*|\[[\x30-\x3f]*[\x20-\x2f]*)?", rest):
            return 0
        # Anything else two-byte (ESC ( B and friends) is not layout.
        return 2

    def _csi(self, params, final):
        # `?`-prefixed parameters are private modes (alternate screen, cursor
        # visibility) and change nothing about the grid.
        if params.startswith("?"):
            return
        nums = [int(n) if n else 0 for n in params.split(";")] if params else []

        def at(index, default=1):
            return nums[index] if len(nums) > index and nums[index] else default

        if final == "H":
            self.row = min(at(0) - 1, self.rows - 1)
            self.col = min(at(1) - 1, self.cols - 1)
        elif final == "J" and at(0, 0) == 2:
            self.grid = [[" "] * self.cols for _ in range(self.rows)]
        elif final == "K":
            for x in range(self.col, self.cols):
                self.grid[self.row][x] = " "
        elif final == "A":
            self.row = max(self.row - at(0), 0)
        elif final == "B":
            self.row = min(self.row + at(0), self.rows - 1)
        elif final == "C":
            self.col = min(self.col + at(0), self.cols - 1)
        elif final == "D":
            self.col = max(self.col - at(0), 0)

    def text(self):
        lines = ["".join(row).rstrip() for row in self.grid]
        while lines and not lines[-1]:
            lines.pop()
        return "\n".join(lines)


def main():
    argv = sys.argv[1:]
    if argv and argv[0] == "--":
        argv = argv[1:]
    if not argv:
        sys.exit("usage: screen.py -- <command> [args...]")

    raw = os.environ.get("KEYS", "")
    chunks = raw.encode().decode("unicode_escape").split("\x00") if raw else []

    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        # The dialog reads $HOME to write paths back as `~`.
        os.execvp(argv[0], argv)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))

    screen = Screen(ROWS, COLS)

    def drain(seconds):
        end = time.time() + seconds
        alive = True
        while time.time() < end:
            ready, _, _ = select.select([fd], [], [], 0.05)
            if not ready:
                continue
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                alive = False
                break
            if not chunk:
                alive = False
                break
            screen.render(chunk.decode(errors="replace"))
        return alive

    drain(1.0)
    for chunk in chunks:
        os.write(fd, chunk.encode())
        if not drain(0.5):
            break
    drain(0.5)

    os.close(fd)
    _, status = os.waitpid(pid, 0)
    print(screen.text())
    sys.exit(os.waitstatus_to_exitcode(status) if status else 0)


if __name__ == "__main__":
    main()
