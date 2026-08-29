# filepick — a TUI file chooser that is its own portal backend

Status: **in progress.** This document is the design — what is being replaced,
why, and the traps worth not re-learning. `README.md` says where the build
currently stands. Written 28/08/2026; building started 29/08/2026.

## The pain

File-chooser dialogs on this machine go to
`xdg-desktop-portal-termfilechooser`, a C daemon that spawns a terminal and
runs a file manager in it. Opening files works well. **Placing** files does
not, and the reasons are structural rather than incidental:

- A file browser answers "which of these existing things?". Saving asks
  "which directory, and what shall it be called?" — two questions, and the
  second has no answer that exists yet.
- The daemon works around that by creating a placeholder file for the browser
  to select. That placeholder is the source of the worst bug in the whole
  chain (see Traps).

The current setup works and is documented, so this project is not urgent. It
exists because the remaining friction is inherent to gluing a browser to a
save dialog, and because owning both ends is more fun than working around
someone else's.

## What already works, and what still doesn't

The live setup is in `~/.config/xdg-desktop-portal-termfilechooser/`, copied
into `reference/` here. It is:

- `portal-wrapper.sh` — dispatches: open/multiple/directory to yazi, save to
  `portal-save.sh`
- `portal-save.sh` — fzf over every directory under `$HOME`, then a readline
  field for the filename
- `config` — note `create_help_file=0`, which is load-bearing

Still wrong, and not fixable from config:

- **`filters` is ignored entirely.** The daemon logs `dbus: unknown option
  filters` and drops them. Applications that say "I save `.corkboard` files"
  are not heard.
- **`SaveFiles` is not implemented at all** — the vtable has only `OpenFile`
  and `SaveFile`.
- **Two steps, two surfaces.** Directory then name, sequentially. A GUI file
  chooser shows both at once and that is better.
- The daemon/chooser boundary is positional argv plus a temp file, an
  undocumented contract that has to be re-derived from the stock wrapper
  every time.

## Architecture

**One binary, two modes.** The daemon has no terminal, so it must spawn one;
that split is unavoidable. Making both ends the same program is what removes
the fragility.

```
app ──D-Bus──> filepick --serve          (portal backend, systemd user service)
                    │
                    └─spawn─> foot -a termfilechooser -e filepick --dialog \
                                    --request /run/user/1000/filepick/req.json \
                                    --response .../res.json
```

Request and response are JSON with shared serde types. The temp file is fine;
what was fragile before was the *undocumented positional* contract, not the
file.

Use [`ashpd`](https://crates.io/crates/ashpd) for the backend half — it has
backend implementation support and ships a
[demo](https://github.com/bilelmoussaoui/ashpd/blob/main/demo/backend/README.md)
showing the shape. It handles Request objects, response encoding, and D-Bus
activation, which is the part that would otherwise be a slog. Phosh is
building a portal this way now.

## The interface

A single screen, both questions visible at once. This is the whole point of
the project — the two-step fzf flow in `reference/portal-save.sh` works, but
a GUI chooser shows you where you are and what it will be called at the same
time, and that is better.

```
┌─ Save board ───────────────────────────────────────────────────┐
│  ~/Pictures/xyz                                                │
├────────────────────────────────────────────────────────────────┤
│    ..                                                          │
│    ref/                                                        │
│  > Screenshots/                                                │
│    bern_copy.corkboard                              31.5 kB    │
│    photo.png                                         1.2 MB    │
├────────────────────────────────────────────────────────────────┤
│  Name  untitled.corkboard                                      │
└────────────────────────────────────────────────────────────────┘
   Tab pane   Enter open/commit   ^P jump   ^L path   Esc cancel
```

**The name field is always visible and always live.** That is precisely what
yazi could not do, and the reason saving through it never worked.

Key map — the important decisions:

- **In the list**: `Enter` on a directory descends into it. `Enter` on a file
  copies its name into the Name field but does **not** commit. Typing filters
  the current directory fuzzily. `Space` toggles selection in multi-select
  mode.
- **In the Name field**: `Enter` commits.
- `Tab` moves between the two.
- `Ctrl-S` (or `Alt-Enter`) commits from anywhere.

`Enter` meaning two different things in two visually distinct places is
fine. `Enter` meaning two things in *one* place is what broke yazi — there,
it had to mean both "descend into this directory" and "commit to this path",
and the portal only ever saw the commit.

Keep the fzf superpower: `Ctrl-P` opens a fuzzy filter over every directory
under `$HOME` and jumps the browser there. `fd --type d` returns ~1500
entries in about 12 ms, so this is instant and beats navigating. Do not make
it the only way in — browsing is still how you discover what is next to what.

Other behaviour worth specifying up front:

- Name field shows an inline warning when the typed name already exists —
  `exists · Enter again to overwrite`. No modal.
- A name containing `/` creates its parent directories.
- `Ctrl-L` makes the path bar editable for typing a path directly.
- Open mode hides the Name field; directory mode commits the current
  directory; multi-select shows a count where the Name field was.

## The portal contract

Verified against the C source at
`~/.local/src/xdg-desktop-portal-termfilechooser` on 28/08/2026.

- Bus name `org.freedesktop.impl.portal.desktop.<name>`, object
  `/org/freedesktop/portal/desktop`, interface
  `org.freedesktop.impl.portal.FileChooser`.
- `OpenFile`, `SaveFile`, `SaveFiles`: all `osssa{sv}` in, `ua{sv}` out —
  `(handle: o, app_id: s, parent_window: s, title: s, options: a{sv})`.
- Response codes: `0` success, `1` cancelled by user, `2` ended some other
  way. Results carry `uris` as a variant of `as`.
- **`current_folder` and `current_file` are `ay`, not `s`** — NUL-terminated
  byte arrays. `current_name` *is* `s`. This asymmetry is easy to get wrong
  and the failure is silent.
- Options to handle: `multiple`, `modal`, `directory` (all `b`), `filters`,
  `current_filter`, `current_name`, `current_folder`, `current_file`,
  `accept_label`, `choices`.

Routing, once built: add a `.portal` file next to
`reference/termfilechooser.portal` and point
`org.freedesktop.impl.portal.FileChooser` at it in
`~/.config/xdg-desktop-portal/niri-portals.conf`. Both are in `reference/`.

## Traps

These cost real time. Do not re-learn them.

**Never require the save target to exist.** The C daemon's
`create_help_file` (default **1**) creates the suggested file before opening
the dialog, writes its own instructions into it, and afterwards *demands*
that the returned path exist — `filechooser.c` stats it and `goto cleanup`s
on ENOENT, skipping the entire success reply. The application receives a
D-Bus error rather than a path, and the save silently does nothing. This is
the bug that made every save fail. A save dialog returns a path that by
definition does not exist yet.

**A save dialog does not create the file.** It returns a path; the
application writes it. Creating it yourself leaves litter when the app
abandons the save — the old daemon scattered five 461-byte instruction stubs
across `~/Pictures` and `~/Documents` doing exactly this.

**Enter cannot mean both "descend" and "commit" in one place.** See the
interface section.

**The compositor float rule matches app-id, not title.** niri floats
`^termfilechooser$` at 1000×600 (`~/.config/niri/config.kdl`). Either keep
that app-id when spawning foot or update the rule; they fail independently
and nothing warns you.

## Testing

Two harnesses, both proven in the session that produced this document.

**Drive the backend with no human in the loop.** This is how the
`create_help_file` bug was isolated — point the daemon at a chooser that
writes a fixed path and returns, then:

```sh
busctl --user call org.freedesktop.impl.portal.desktop.filepick \
  /org/freedesktop/portal/desktop org.freedesktop.impl.portal.FileChooser \
  SaveFile "osssa{sv}" /org/test/req com.test "" "Save" 0
```

A working backend answers `ua{sv} 0 1 "uris" as 1 "file:///…"`. A broken one
answers `Call failed: Unknown method SaveFile or interface …`, which is what
a handler returning without a reply looks like from the caller's side.

**Drive the TUI through a pty.** `reference/ptyharness.py` allocates a pty,
sets `TERM` and a window size, and feeds keystrokes in timed chunks. Two
things it taught the hard way:

- Feed keys in **paced chunks**, not one dump. Readline flushes pending input
  when it re-arms, so a burst gets eaten and the test looks like a hang.
- A real Return is **CR (`\r`)**, not LF. Anything in raw mode — fzf, and any
  ratatui app — ignores `\n`. This will look exactly like "Enter does
  nothing".

Keep the path-resolution logic pure and unit-test it separately: `~`
expansion, names carrying `/`, relative fragments, collision detection.

## Scope

**In:** `OpenFile`, `SaveFile`, `SaveFiles`; single/multi/directory modes;
filters honoured; the single-screen UI above.

**Out, at least for v1:** `choices` (the extra-widgets option), thumbnails or
previews, remote/URI locations that are not `file://`, replacing yazi as a
general file manager. This is a chooser, not a file manager.

**Unchanged regardless:** a terminal still gets spawned, the niri float rule
still applies, `niri-portals.conf` still does the routing, and LibreWolf's
`user.js` still needs `widget.use-xdg-desktop-portal.file-picker=1`.

## Look

Follow the existing system rather than inventing a palette. `~/.config/foot/foot.ini`
is the source; everything else names **ANSI slot numbers** so it moves when
foot does. Do not hardcode hex.

Chrome signals with **brightness, never hue** — no accent colour. Mark
selection with a bright pointer and bright text rather than a fill; slot 0
against the terminal background is too small a step to read at a glance.
Relevant memories: `system-chrome-understated`, `palette-inheriting-themes`,
`ansi-slot-names-differ`, `dark-text-on-light-accent`.

## Switching over, and back

The current setup keeps working until deliberately replaced. To switch,
change `cmd=` in `~/.config/xdg-desktop-portal-termfilechooser/config`, or
repoint `org.freedesktop.impl.portal.FileChooser` in `niri-portals.conf` at
the new backend. To go back, put `cmd=yazi-wrapper.sh` back — the stock
wrapper is still installed in `/usr/share/`.

Restart after any change:

```sh
systemctl --user restart xdg-desktop-portal-termfilechooser.service
```

## Reference files

| File | What it is |
| --- | --- |
| `reference/portal-wrapper.sh` | Live dispatcher: yazi for open, script for save |
| `reference/portal-save.sh` | Live save dialog — the proven two-step interaction |
| `reference/config` | Live daemon config; note `create_help_file=0` |
| `reference/niri-portals.conf` | How the FileChooser interface is routed |
| `reference/termfilechooser.portal` | The `.portal` registration file to model |
| `reference/ptyharness.py` | Pty test harness for driving a TUI headlessly |

Source of the daemon being replaced:
`~/.local/src/xdg-desktop-portal-termfilechooser` (hand-built into `/usr`,
owned by no RPM). `src/filechooser/filechooser.c` is the file worth reading —
option parsing around line 600, the help-file logic around line 690.
