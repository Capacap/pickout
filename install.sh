#!/usr/bin/env bash
# What this machine uses, not the only way in. The four files it writes are
# described in the README; anyone packaging filepick or wiring it into a
# different session is expected to place them their own way.
#
# Installs filepick into the user's XDG directories and registers it as a
# portal backend. Nothing is installed system-wide and nothing is overwritten
# outside the paths listed at the end.
#
# This does NOT route the FileChooser interface to filepick — that is a
# deliberate one-line edit in niri-portals.conf, so that installing and
# switching over stay separate decisions. See the note printed at the end.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
prefix=${PREFIX:-$HOME/.local}
binary=$prefix/bin/filepick

data=${XDG_DATA_HOME:-$HOME/.local/share}
config=${XDG_CONFIG_HOME:-$HOME/.config}

cargo build --release --manifest-path "$here/Cargo.toml"

install -Dm755 "$here/target/release/filepick" "$binary"

render() {
    sed "s|@BINARY@|$binary|g" "$1" > "$2.tmp" && mv -- "$2.tmp" "$2"
}

install -d "$data/xdg-desktop-portal/portals" \
           "$data/dbus-1/services" \
           "$config/systemd/user"

render "$here/data/filepick.portal.in" \
       "$data/xdg-desktop-portal/portals/filepick.portal"
render "$here/data/org.freedesktop.impl.portal.desktop.filepick.service.in" \
       "$data/dbus-1/services/org.freedesktop.impl.portal.desktop.filepick.service"
render "$here/data/filepick.service.in" \
       "$config/systemd/user/filepick.service"

# Only a convenience: the D-Bus service file above starts the daemon on demand
# with or without systemd, so a machine that has no user manager is not missing
# anything but `systemctl status`.
if command -v systemctl >/dev/null; then
    systemctl --user daemon-reload || true
    systemctl --user restart filepick.service 2>/dev/null || true
fi

cat <<MSG

Installed:
  $binary
  $data/xdg-desktop-portal/portals/filepick.portal
  $data/dbus-1/services/org.freedesktop.impl.portal.desktop.filepick.service
  $config/systemd/user/filepick.service

filepick is registered but not yet routed. To send file dialogs to it, set

  org.freedesktop.impl.portal.FileChooser=filepick;

in $config/xdg-desktop-portal/portals.conf, or in the <desktop>-portals.conf
for the session in use — niri-portals.conf on this machine — then

  systemctl --user restart xdg-desktop-portal.service

To go back, put termfilechooser back on that line and restart the same unit.
MSG
