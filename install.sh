#!/usr/bin/env bash
# One way in, not the only one. The four files it writes are described in the
# README; anyone packaging pickout or wiring it into a different session is
# expected to place them their own way.
#
# Installs pickout into the user's XDG directories and registers it as a
# portal backend. Nothing is installed system-wide and nothing is overwritten
# outside the paths listed at the end.
#
# This does NOT route the FileChooser interface to pickout. That is a
# deliberate one-line edit in portals.conf, so that installing and switching
# over stay separate decisions. See the note printed at the end.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
prefix=${PREFIX:-$HOME/.local}
binary=$prefix/bin/pickout

data=${XDG_DATA_HOME:-$HOME/.local/share}
config=${XDG_CONFIG_HOME:-$HOME/.config}

cargo build --release --manifest-path "$here/Cargo.toml"

install -Dm755 "$here/target/release/pickout" "$binary"

render() {
    sed "s|@BINARY@|$binary|g" "$1" > "$2.tmp" && mv -- "$2.tmp" "$2"
}

install -d "$data/xdg-desktop-portal/portals" \
           "$data/dbus-1/services" \
           "$config/systemd/user"

render "$here/data/pickout.portal.in" \
       "$data/xdg-desktop-portal/portals/pickout.portal"
render "$here/data/org.freedesktop.impl.portal.desktop.pickout.service.in" \
       "$data/dbus-1/services/org.freedesktop.impl.portal.desktop.pickout.service"
render "$here/data/pickout.service.in" \
       "$config/systemd/user/pickout.service"

# Only a convenience: the D-Bus service file above starts the daemon on demand
# with or without systemd, so a machine that has no user manager is not missing
# anything but `systemctl status`.
if command -v systemctl >/dev/null; then
    systemctl --user daemon-reload || true
    systemctl --user restart pickout.service 2>/dev/null || true
fi

cat <<MSG

Installed:
  $binary
  $data/xdg-desktop-portal/portals/pickout.portal
  $data/dbus-1/services/org.freedesktop.impl.portal.desktop.pickout.service
  $config/systemd/user/pickout.service

pickout is registered but not yet routed. To send file dialogs to it, set

  org.freedesktop.impl.portal.FileChooser=pickout;

in $config/xdg-desktop-portal/portals.conf, or in the <desktop>-portals.conf
for the session in use, then

  systemctl --user restart xdg-desktop-portal.service

To go back, put whatever was on that line before back, and restart the same
unit.
MSG
