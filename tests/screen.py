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

Setting `STYLES` to a path additionally writes what each run of text was drawn
in. Without it nothing in the suite can see a colour, and every span could be
set to the same one with all the checks still passing.
"""

import fcntl
import os
import pty
import re
import select
import struct
import sys
import termios
import time
import unicodedata

# The default is a comfortable window; the tests that care about a cramped one
# set these instead.
ROWS = int(os.environ.get("ROWS", 40))
COLS = int(os.environ.get("COLS", 120))

# A wide character occupies two columns and leaves the second one unwritable,
# which is the distinction the dialog's own width arithmetic turns on. A grid
# that advanced one column per character would agree with a bug rather than
# catch it.
WIDE = frozenset("WF")

# SGR attributes worth keeping apart, and the codes that turn them off again.
# `REVERSE` is here despite the dialog not using it: a harness that silently
# drops the obvious alternative to bold is the trap this dump exists to close.
ATTRS = {1: "bold", 2: "dim", 3: "italic", 4: "underline", 7: "reverse"}
OFF = {22: ("bold", "dim"), 23: ("italic",), 24: ("underline",), 27: ("reverse",)}
DEFAULT = (None, None, frozenset())


def pen_text(pen):
    """A pen as one field of a dump line: `fg8`, `bold`, `fg8+bold`, `-`."""
    fg, bg, attrs = pen
    parts = []
    if fg is not None:
        parts.append(f"fg{fg}")
    if bg is not None:
        parts.append(f"bg{bg}")
    parts.extend(sorted(attrs))
    return "+".join(parts) or "-"


def cell_width(c):
    if unicodedata.combining(c):
        return 0
    return 2 if unicodedata.east_asian_width(c) in WIDE else 1


class Screen:
    """Enough of a terminal to render what ratatui emits: absolute cursor
    moves, erases, text, and the pen each cell was written with. Colours are
    kept as slot numbers rather than resolved, because that is the level the
    dialog picks them at and the level a theme changes underneath it."""

    def __init__(self, rows, cols):
        self.rows, self.cols = rows, cols
        self.grid = [[" "] * cols for _ in range(rows)]
        self.style = [[DEFAULT] * cols for _ in range(rows)]
        self.pen = DEFAULT
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
                    self.style[self.row][self.col] = self.pen
                    # The trailing half of a wide cell holds nothing, so it
                    # takes up a column without contributing a character.
                    for x in range(self.col + 1, min(self.col + w, self.cols)):
                        self.grid[self.row][x] = ""
                        self.style[self.row][x] = self.pen
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
            self.style = [[DEFAULT] * self.cols for _ in range(self.rows)]
        elif final == "K":
            for x in range(self.col, self.cols):
                self.grid[self.row][x] = " "
                self.style[self.row][x] = DEFAULT
        elif final == "m":
            self._sgr(nums or [0])
        elif final == "A":
            self.row = max(self.row - at(0), 0)
        elif final == "B":
            self.row = min(self.row + at(0), self.rows - 1)
        elif final == "C":
            self.col = min(self.col + at(0), self.cols - 1)
        elif final == "D":
            self.col = max(self.col - at(0), 0)

    def _sgr(self, nums):
        fg, bg, attrs = self.pen
        attrs = set(attrs)
        i = 0
        while i < len(nums):
            n = nums[i]
            if n == 0:
                fg = bg = None
                attrs.clear()
            elif n in ATTRS:
                attrs.add(ATTRS[n])
            elif n in OFF:
                attrs.difference_update(OFF[n])
            elif n in (38, 48):
                # `38;5;n` and `38;2;r;g;b`. crossterm writes even the base
                # sixteen the first way, so a slot arrives as `38;5;8`.
                kind = nums[i + 1] if i + 1 < len(nums) else None
                if kind == 5:
                    value, i = nums[i + 2] if i + 2 < len(nums) else 0, i + 2
                elif kind == 2:
                    value, i = tuple(nums[i + 2 : i + 5]), i + 4
                else:
                    value = None
                if n == 38:
                    fg = value
                else:
                    bg = value
            elif 30 <= n <= 37 or 90 <= n <= 97:
                fg = n - 30 if n <= 37 else n - 82
            elif 40 <= n <= 47 or 100 <= n <= 107:
                bg = n - 40 if n <= 47 else n - 92
            elif n == 39:
                fg = None
            elif n == 49:
                bg = None
            i += 1
        self.pen = (fg, bg, frozenset(attrs))

    def styles(self):
        """One line per run of same-pen text: row, column, pen, what it drew.

        Runs of blanks are dropped. A pen is only worth reporting where it put
        something on the screen, and the padding either side of a widget would
        otherwise be most of the dump."""
        lines = []
        for r, (chars, pens) in enumerate(zip(self.grid, self.style), 1):
            c = 0
            while c < self.cols:
                pen, start = pens[c], c
                while c < self.cols and pens[c] == pen:
                    c += 1
                run = "".join(chars[start:c]).strip()
                if run:
                    lines.append(f"{r:3} {start + 1:3} {pen_text(pen):<14} {run}")
        return "\n".join(lines)

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
    if os.environ.get("STYLES"):
        with open(os.environ["STYLES"], "w") as f:
            f.write(screen.styles() + "\n")
    print(screen.text())
    sys.exit(os.waitstatus_to_exitcode(status) if status else 0)


if __name__ == "__main__":
    main()
