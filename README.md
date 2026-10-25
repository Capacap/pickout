# pickout

A terminal file chooser that is its own XDG desktop portal backend. When
an application asks the desktop for a file, pickout opens in a terminal
window and browses there, and what you pick goes back as though the GTK
dialog had returned it.

If you work in the terminal, the file dialog is where the muscle memory
stops. No fuzzy filter, no typing a path, no jumping to the directory you
were in five minutes ago. pickout puts those back, and keeps the browser
and the Name field on one screen so saving stays a single question.

![A save dialog opened by a browser: the listing with file sizes, the
application's own PNG filter named in the footer, and the Name field
warning that the name it was given already
exists](https://raw.githubusercontent.com/Capacap/pickout/main/.github/screenshot.png)

## Usage

You never run pickout yourself. An application opens a file dialog, the
portal starts the daemon, and the daemon spawns a terminal window with
the chooser inside it.

| | In the list | In the Name field |
| --- | --- | --- |
| `Enter` | descend, or copy the file's name into Name (open mode: open it) | commit |
| `Tab` | to the Name field | back to the list |
| `Left` / `Right` | up a directory / descend | |
| typing | filters the listing | edits the name |
| `Space` | marks the row, where more than one answer is wanted | a space |
| `Esc` | clears the filter, then cancels | cancels |

`^S` and `M-Enter` commit from anywhere, `^P` opens the jump list, `^F`
cycles the application's filters, `^L` makes the path bar editable, `^N`
makes a directory (parents included) and goes into it, `M-h` toggles
dotfiles, and `^W` / `^U` / `^K` / `^A` / `^E` edit as readline does.
`^N` is offered wherever the answer is a place to save or a directory,
and not when opening a file.

`Enter` means two things, but never in one place. In the list it
descends, in the Name field it commits. Meaning both at once is what
made the browser this replaces unusable for saving.

The Name field says what committing would do before you commit, whether
that is `exists`, `creates 2 directories` or `is a directory`.
Overwriting takes two `Enter`s. A name that is a directory, or ends in
`/`, is somewhere to go rather than something to save as: `Enter` makes
whatever of it is missing, goes in, and leaves the field empty for the
filename. `Space` marks a row in the modes that
take more than one answer, and marks survive changing directory: once
anything is marked, the marks are the whole answer.

`^P` ranks every directory under `$HOME` as you type. The walk runs in
the background, so the dialog still opens at once.

The footer is not a written-down copy of any of this. It asks the same
table the dispatcher asks, so a key it names is a key that works, and one
that works in the pane you are in gets named. Rebind something and the
footer says the key you chose.

## Installation

Requires a session running `xdg-desktop-portal`, and a terminal emulator
that stays in the foreground until the command it was given exits.

```
app ──D-Bus──> pickout --serve            portal backend, D-Bus activated
                    │
                    └─spawn─> kitty --app-id=pickout -- \
                                pickout --dialog <request.json> <response.json>
```

pickout is four files, and the binary is only the first of them.
`./install.sh` builds it and puts all four where a user session looks:

```sh
git clone https://github.com/Capacap/pickout
cd pickout && ./install.sh
```

| File | Where |
| --- | --- |
| `pickout` | `~/.local/bin` |
| `pickout.portal` | `$XDG_DATA_HOME/xdg-desktop-portal/portals` |
| `org.freedesktop.impl.portal.desktop.pickout.service` | `$XDG_DATA_HOME/dbus-1/services` |
| `pickout.service` | `$XDG_CONFIG_HOME/systemd/user` |

The `.portal` file declares which interface the backend implements and,
in `UseIn`, the sessions it may be chosen in. That line ships wide and is
yours to narrow. The D-Bus service file starts the daemon on demand and
names the systemd unit, so activation goes through the user manager where
there is one. Without systemd, drop the `SystemdService=` line and the
unit with it, and D-Bus runs `Exec=` directly instead.

Registering is not routing. To send file dialogs to pickout, set

```
org.freedesktop.impl.portal.FileChooser=pickout;
```

in `$XDG_CONFIG_HOME/xdg-desktop-portal/portals.conf`, or in the
`<desktop>-portals.conf` for the session in use, then restart
`xdg-desktop-portal`. The two steps stay separate on purpose, so
installing and switching over are separate decisions, and going back is
the same line with whatever was on it before.

Compositors that tile by default want a float rule matching the `pickout`
app-id; a niri example is in
[contrib/niri.kdl](https://github.com/Capacap/pickout/blob/main/contrib/niri.kdl).
Changing the app-id without the rule fails silently, and the dialog opens
as an ordinary tiled column.

## Configuration

All optional. `$XDG_CONFIG_HOME/pickout/config.toml`, or the file
`PICKOUT_CONFIG` names:

```toml
terminal = "kitty --app-id=pickout --"   # split on whitespace; must block
open_dir = "suggested"                   # suggested | last | an absolute path
save_dir = "last"                        # last overrides the application on
                                         # purpose, see src/portal.rs

[keys.global]                            # global | list | name | jump | path | mkdir
"^E" = "commit"
"M-p" = "jump"

[keys.list]
"M-j" = "down"
"M-k" = "up"
```

D-Bus starts the daemon without a shell's exports, so the file is the one
route always open to it. `PICKOUT_TERMINAL`, `PICKOUT_OPEN_DIR` and
`PICKOUT_SAVE_DIR` override it where the environment does reach, and
`RUST_LOG=pickout=debug` watches requests arrive.

Keys are written the way the footer draws them, so a line copied off the
screen is already a binding: `^S`, `M-h`, `Enter`, `Space`, `PageDown`.
`ctrl-s` and `alt-h` say the same thing. A binding names what one key
does and takes nothing else away, so a shipped key keeps working unless
you bind over it.

The actions, by the pane they mean something in:

| Pane | Actions |
| --- | --- |
| `global` | `commit` `cancel` `jump` `filter` `path` `mkdir` `hidden` `pane` |
| `list` | the above, and `activate` `parent` `up` `down` `page-up` `page-down` `first` `last` `mark` `dismiss` `erase` `clear` |
| `name` | the global set |
| `jump` | `go` `close` `up` `down` `page-up` `page-down` |
| `path`, `mkdir` | `go` `close` |

`dismiss` is the whole of what `Esc` means in the list, clearing the
filter and cancelling only once there is none, rather than the first half
of it. `^C` is not in the table, because a way out that a config file can
take away is not a way out. Neither are the readline keys inside a text
field, which belong to the field.

Nothing in the file is fatal. A key that names nothing, an action that
does not exist, an action bound where it could do nothing: each costs its
own line and says so on the message line. A dialog that failed to start
would reach the application as a cancel it could not tell from a real one.

## Development

```sh
cargo test         # wire protocol, URIs, directory policy, the model
tests/dialog.sh    # the chooser, driven through a pty
tests/portal.sh    # the D-Bus backend, on a private bus, no window
```

`tests/portal.sh` stubs the terminal out with `PICKOUT_TERMINAL`, which
anything that takes a command and blocks until it exits will satisfy. Its
last section uses the pty harness as that terminal, so the two halves are
checked against each other rather than each against a stub of its own
making. Both suites point `PICKOUT_CONFIG` at a file of their own, so the
developer's settings are never silently part of the result.

`tests/screen.py` runs a command under a pty, feeds keystrokes in paced
chunks and prints the screen that was drawn. Two rules it learned the
hard way: keys must arrive in chunks, because an application in raw mode
eats a burst, and a real Return is CR (`\r`), never LF.

## License

Licensed under either of
[Apache License, Version 2.0](https://github.com/Capacap/pickout/blob/main/LICENSE-APACHE)
or [MIT license](https://github.com/Capacap/pickout/blob/main/LICENSE-MIT)
at your option. Unless you explicitly
state otherwise, any contribution intentionally submitted for
inclusion in the work by you shall be dual licensed as above, without
any additional terms or conditions.
