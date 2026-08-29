# filepick

A terminal file chooser that is its own portal backend. One binary, two modes:

```
app ──D-Bus──> filepick --serve            portal backend, systemd user service
                    │
                    └─spawn─> kitty --app-id=filepick -- \
                                filepick --dialog <request.json> <response.json>
```

`HANDOFF.md` is the design document — what this replaces, why, and the traps
that cost time the first time round. This file is where it currently stands.

## State

Both halves work, and everything the portal interface can ask for is answered.
The interface is the single screen the design asks for — browser and Name field
visible at once, both live. What is left is dogfooding: routing this machine's
file dialogs to filepick and living with it.

Working:

- `OpenFile`, `SaveFile`, `SaveFiles`, single / multiple / directory modes
- `filters` and `current_filter`, both directions — the option the daemon being
  replaced dropped with `dbus: unknown option filters`
- `current_folder` / `current_file` as NUL-terminated byte arrays, `current_name`
  as a string
- start-directory policy per mode, `last` by default for saves
- cancel, a dialog that writes nothing, a dialog that dies, and an application
  that withdraws its request mid-dialog — all answered as a cancel, and the
  last of those takes the terminal down with it
- `src/model.rs`: directory listing, `~` expansion, names carrying `/`,
  relative fragments, what a typed name would overwrite or have to create,
  glob and MIME filter matching
- the chooser: browse, type to filter fuzzily, `M-h` for dotfiles, `^L` to type
  a path, a live Name field that says what committing would do — `exists`,
  `creates 2 directories`, `is a directory` — and takes two `Enter`s to
  overwrite
- the application's filters applied to the listing, `^F` to cycle them, and an
  "all files" entry of filepick's own so a filter can always be got out of
- `^P`: every directory under `$HOME`, ranked as you type, walked in the
  background so the dialog opens at once
- `Space` to mark, in the modes that take more than one answer; marks survive
  changing directory
- `SaveFiles`: the names the application is about to write, and how many of them
  already exist in the directory the cursor is on

## Keys

| | In the list | In the Name field |
| --- | --- | --- |
| `Enter` | descend, or copy the file's name into Name (open mode: open it) | commit |
| `Tab` | to the Name field | back to the list |
| `Left` / `Right` | up a directory / descend | — |
| typing | filters the listing | edits the name |
| `Space` | marks the row, where more than one answer is wanted | a space |
| `Esc` | clears the filter, then cancels | cancels |

`^S` commits from anywhere, `^P` opens the jump list, `^F` cycles the
application's filters, `^L` makes the path bar editable, `M-h` toggles dotfiles,
`^W` / `^U` / `^K` / `^A` / `^E` edit as readline does.

`Enter` means two things, but never in one place: in the list it descends, in
the Name field it commits. Meaning both in one place is what made the browser
this replaces unusable for saving.

## Build and test

```sh
cargo test         # pure logic: wire protocol, URIs, directory policy, the model
tests/dialog.sh    # the chooser, driven through a pty
tests/portal.sh    # the D-Bus backend, on a private bus, no window, no human
```

`tests/portal.sh` stubs the terminal out with `FILEPICK_TERMINAL` — anything
that takes a command and blocks until it exits will do — and its last section
uses the pty harness as that terminal, so the two halves are checked against
each other rather than each against a stub of its own making.

`tests/screen.py` is the harness: it runs a command under a pty, feeds
keystrokes in paced chunks and prints the screen that was drawn. Two rules it
learned the hard way and that still bite — keys must arrive in chunks, because
an application in raw mode eats a burst; and a real Return is CR (`\r`), never
LF.

## Install

```sh
./install.sh
```

Installs the binary to `~/.local/bin`, registers the `.portal`, D-Bus service
and systemd unit under `~/.local`, and stops there. It does **not** route file
dialogs to filepick — that is a one-line edit in
`~/.config/xdg-desktop-portal/niri-portals.conf`, kept separate so installing
and switching over stay separate decisions. `install.sh` prints the line.

To go back: put `termfilechooser` on that line and restart
`xdg-desktop-portal.service`.

## Configuration

All optional; the defaults are what this machine wants.

| Variable | Default | Meaning |
| --- | --- | --- |
| `FILEPICK_TERMINAL` | `kitty --app-id=filepick --` | Terminal invocation, split on whitespace. Must block until its command exits. |
| `FILEPICK_OPEN_DIR` | `suggested` | Where open dialogs start: `suggested`, `last`, or an absolute path. |
| `FILEPICK_SAVE_DIR` | `last` | Same, for saves. `last` overrides the application on purpose — see `src/portal.rs`. |
| `RUST_LOG` | `filepick=info` | `filepick=debug` to watch requests arrive. |

The `filepick` app-id is matched by a `window-rule` in `~/.config/niri/config.kdl`,
which floats the window. Changing one without the other fails silently.
