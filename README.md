# filepick

A terminal file chooser that is its own portal backend. One binary, two modes:

```
app ──D-Bus──> filepick --serve            portal backend, D-Bus activated
                    │
                    └─spawn─> kitty --app-id=filepick -- \
                                filepick --dialog <request.json> <response.json>
```

`HANDOFF.md` is the design document — what this replaces, why, and the traps
that cost time the first time round. This file is where it currently stands.

## State

Both halves work, and everything the portal interface can ask for is answered.
The interface is the single screen the design asks for — browser and Name field
visible at once, both live. This machine's file dialogs are routed to filepick;
what is left is living with it and finding out what the tests could not.

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
  changing directory, and once anything is marked the marks are the whole
  answer — the cursor is where navigating left it, not something chosen
- `SaveFiles`: the names the application is about to write, and how many of them
  already exist in the directory the cursor is on
- keys, the terminal and the start-directory policy from a config file, with one
  binding table behind both the dispatcher and the footer

## Keys

| | In the list | In the Name field |
| --- | --- | --- |
| `Enter` | descend, or copy the file's name into Name (open mode: open it) | commit |
| `Tab` | to the Name field | back to the list |
| `Left` / `Right` | up a directory / descend | — |
| typing | filters the listing | edits the name |
| `Space` | marks the row, where more than one answer is wanted | a space |
| `Esc` | clears the filter, then cancels | cancels |

`^S` and `M-Enter` commit from anywhere, `^P` opens the jump list, `^F` cycles
the application's filters, `^L` makes the path bar editable, `M-h` toggles
dotfiles, `^W` / `^U` / `^K` / `^A` / `^E` edit as readline does.

`Enter` means two things, but never in one place: in the list it descends, in
the Name field it commits. Meaning both in one place is what made the browser
this replaces unusable for saving.

The footer is not a written-down copy of any of this. It asks the same table the
dispatcher asks — which key does this, and does it do anything here — so a key
it names is a key that works, and one that works in the pane you are in gets
named. Rebind something and the footer says the key you chose.

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

Both suites point `FILEPICK_CONFIG` at a file of their own, so the developer's
settings are not silently part of the result.

`tests/screen.py` is the harness: it runs a command under a pty, feeds
keystrokes in paced chunks and prints the screen that was drawn. Two rules it
learned the hard way and that still bite — keys must arrive in chunks, because
an application in raw mode eats a burst; and a real Return is CR (`\r`), never
LF.

## Install

filepick is four files. `./install.sh` is what this machine uses to put them in
place, and it is not the only way in — a package, a Nix module or a hand-written
drop-in wires up the same four, wherever the session expects them.

| File | Where | What it does |
| --- | --- | --- |
| `filepick` | `~/.local/bin` | the binary, in both its modes |
| `filepick.portal` | `$XDG_DATA_HOME/xdg-desktop-portal/portals` | declares which interface this backend implements, and `UseIn` the sessions it may be chosen in — that line is yours to narrow |
| `org.freedesktop.impl.portal.desktop.filepick.service` | `$XDG_DATA_HOME/dbus-1/services` | lets D-Bus start the daemon on demand |
| `filepick.service` | `$XDG_CONFIG_HOME/systemd/user` | optional: puts the daemon under the user manager, for `systemctl --user status filepick` and `Restart=on-failure` |

The D-Bus service file names the systemd unit, so activation goes through the
user manager where there is one. **Without systemd**, drop the
`SystemdService=` line and the last file with it: D-Bus then runs `Exec=`
directly and everything else is unchanged.

Registering is not routing. To send file dialogs to filepick, set

```
org.freedesktop.impl.portal.FileChooser=filepick;
```

in `$XDG_CONFIG_HOME/xdg-desktop-portal/portals.conf`, or in the
`<desktop>-portals.conf` for the session in use, then restart
`xdg-desktop-portal`. The two stay separate on purpose: installing and
switching over are separate decisions, and going back is the same line with
whatever was there before.

The switch-over is worth checking without a window in the way: point the
`terminal` setting at a script that runs `tests/screen.py` and drive the real
dialog with `KEYS`. A frontend call then goes app → `xdg-desktop-portal` →
filepick → dialog → URI with nothing stubbed but the terminal.

## Configuration

All optional; the defaults are what this machine wants.
`$XDG_CONFIG_HOME/filepick/config.toml`, or the file `FILEPICK_CONFIG` names:

```toml
terminal = "kitty --app-id=filepick --"   # split on whitespace; must block
open_dir = "suggested"                    # suggested | last | an absolute path
save_dir = "last"                         # last overrides the application on
                                          # purpose — see src/portal.rs

[keys.global]                             # global | list | name | jump | path
"^E" = "commit"
"M-p" = "jump"

[keys.list]
"^N" = "down"
"^P" = "up"
```

The daemon is started by D-Bus, which does not inherit a shell's exports, so
the file is the route that is always open to it. `FILEPICK_TERMINAL`,
`FILEPICK_OPEN_DIR` and `FILEPICK_SAVE_DIR` still override it where the
environment does reach — the tests use them — and `RUST_LOG=filepick=debug`
watches requests arrive.

Keys are written the way the footer draws them, so a line copied off the screen
is already a binding: `^S`, `M-h`, `Enter`, `Space`, `PageDown`; `ctrl-s` and
`alt-h` say the same thing. A binding names what one key does and takes nothing
else away, so the shipped key keeps working unless you bind over it.

The actions, by the pane they mean something in:

| Pane | Actions |
| --- | --- |
| `global` | `commit` `cancel` `jump` `filter` `path` `hidden` `pane` |
| `list` | the above, and `activate` `parent` `up` `down` `page-up` `page-down` `first` `last` `mark` `dismiss` `erase` `clear` |
| `name` | the global set |
| `jump` | `go` `close` `up` `down` `page-up` `page-down` |
| `path` | `go` `close` |

`dismiss` is `Esc`'s whole meaning in the list — clear the filter, and cancel
only once there is none — rather than the first half of it. `^C` is not in the
table: a way out that a config file can take away is not a way out. Neither are
the readline keys inside a text field, which belong to the field.

Nothing in the file is fatal. A key that names nothing, an action that does not
exist, an action bound where it could do nothing: each costs its own line and
says so on the message line. A dialog that failed to start would reach the
application as a cancel it could not tell from a real one.

The `filepick` app-id is matched by a `window-rule` in `~/.config/niri/config.kdl`,
which floats the window. Changing one without the other fails silently — and a
`terminal` setting that replaces the kitty invocation has to set the app-id
itself, or the rule stops matching.
