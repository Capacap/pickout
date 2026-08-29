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

The portal half is done and proven end to end. The dialog half is a stub that
answers without asking anyone; the interface described in `HANDOFF.md` is not
built yet.

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

Not built: the dialog itself (`src/dialog.rs` is a placeholder).

## Build and test

```sh
cargo test         # pure logic: the wire protocol, URIs, directory policy
tests/portal.sh    # the D-Bus backend, on a private bus, no window, no human
```

`tests/portal.sh` stubs the terminal out with `FILEPICK_TERMINAL`. Anything that
takes a command and blocks until it exits will do.

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
