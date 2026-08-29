#!/usr/bin/env bash
# Drives the D-Bus backend with no human and no window, on a private session
# bus so it cannot collide with the real one or with whatever is currently
# routing file dialogs.
#
# The terminal is stubbed out by FILEPICK_TERMINAL. Anything that takes a
# command and blocks until it exits will do, which is the whole of what the
# daemon asks of a terminal. Most of this stands a script in for the dialog so
# the daemon is tested alone; the last section runs the real dialog under the
# pty harness, so the two halves are checked against each other rather than
# each against a stub of its own making.
#
# Run: tests/portal.sh
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

if [[ ${1:-} != --inner ]]; then
    cargo build --manifest-path "$root/Cargo.toml"
    exec dbus-run-session -- "${BASH_SOURCE[0]}" --inner
fi

bus=org.freedesktop.impl.portal.desktop.filepick
obj=/org/freedesktop/portal/desktop
iface=org.freedesktop.impl.portal.FileChooser

tmp=$(mktemp -d)
daemon=
trap 'kill "$daemon" 2>/dev/null || true; rm -rf -- "$tmp"' EXIT

export XDG_RUNTIME_DIR="$tmp/run" XDG_STATE_HOME="$tmp/state" HOME="$tmp/home"
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_STATE_HOME" "$HOME/Pictures"

# ashpd asks for the bus name without allowing replacement, so swapping the
# stubbed terminal means restarting rather than starting a second daemon.
# start_daemon <terminal> [VAR=value ...]
start_daemon() {
    local terminal=$1; shift
    if [[ -n $daemon ]]; then
        kill "$daemon" 2>/dev/null || true
        wait "$daemon" 2>/dev/null || true
    fi
    env FILEPICK_TERMINAL="$terminal" "$@" \
        "$root/target/debug/filepick" --serve >>"$tmp/daemon.log" 2>&1 &
    daemon=$!
    for _ in $(seq 50); do
        busctl --user status "$bus" >/dev/null 2>&1 && return
        sleep 0.1
    done
    echo "daemon never took the bus name; log follows" >&2
    cat "$tmp/daemon.log" >&2
    exit 1
}

pass=0 fail=0
check() { # check <name> <expected> <actual>
    if [[ $2 == "$3" ]]; then
        printf '  ok    %s\n' "$1"; pass=$((pass + 1))
    else
        printf '  FAIL  %s\n     expected: %s\n     actual:   %s\n' "$1" "$2" "$3"
        fail=$((fail + 1))
    fi
}

call() { busctl --user call "$bus" "$obj" "$iface" "$@" 2>&1; }

# A path as the NUL-terminated byte array the portal spec demands for
# current_folder/current_file/files. `current_name` is a plain string; that
# asymmetry is easy to get wrong and fails silently, so it is tested first.
bytes() { python3 -c 'import sys; b=sys.argv[1].encode()+b"\x00"; print(len(b), *b)' "$1"; }

# A stand-in dialog, given the response it should write.
# argv reaching it is: <binary> --dialog <request> <response>
fake_dialog() {
    local path="$tmp/fake-$1.sh"
    printf '#!/usr/bin/env bash\n%s\n' "$2" > "$path"
    chmod +x "$path"
    printf '%s' "$path"
}

# A stand-in dialog that answers the way the request implies, so the daemon can
# be tested on its own. The real dialog needs a terminal; these tests
# deliberately have none, and the last section below runs it under one.
# argv reaching it is: <this> <binary> --dialog <request> <response>
stub="$tmp/stub-dialog.py"
cat > "$stub" <<'PYTHON'
import json, os, sys

request = json.load(open(sys.argv[3]))
start, mode = request["start_dir"], request["mode"]

if mode == "save_file":
    paths = [os.path.join(start, request["current_name"] or "filepick-stub")]
elif "save_files" in mode:
    paths = [os.path.join(start, name) for name in mode["save_files"]["names"]]
elif "open_directory" in mode:
    paths = [start]
else:
    paths = [os.path.join(start, "filepick-stub")]

json.dump(
    {"outcome": "selected", "paths": paths, "filter": request["current_filter"]},
    open(sys.argv[4], "w"),
)
PYTHON

echo "portal round trip"
start_daemon "python3 $stub"

check "version is 4" \
    "u 4" \
    "$(busctl --user get-property "$bus" "$obj" "$iface" version 2>&1)"

check "SaveFile returns the suggested name in the start directory" \
    "ua{sv} 0 1 \"uris\" as 1 \"file://$HOME/board.corkboard\"" \
    "$(call SaveFile osssa{sv} /t/1 com.test "" Save 1 current_name s board.corkboard)"

check "current_folder arrives as a byte array and is honoured" \
    "ua{sv} 0 1 \"uris\" as 1 \"file://$HOME/Pictures/filepick-stub\"" \
    "$(call OpenFile osssa{sv} /t/2 com.test "" Open 1 current_folder ay $(bytes "$HOME/Pictures"))"

check "directory mode answers with a directory" \
    "ua{sv} 0 1 \"uris\" as 1 \"file://$HOME\"" \
    "$(call OpenFile osssa{sv} /t/3 com.test "" Pick 1 directory b true)"

check "SaveFiles answers once per name, percent-encoded" \
    "ua{sv} 0 1 \"uris\" as 2 \"file://$HOME/one.txt\" \"file://$HOME/two%20three.txt\"" \
    "$(call SaveFiles osssa{sv} /t/4 com.test "" "Save all" 1 files aay 2 $(bytes one.txt) $(bytes "two three.txt"))"

# The daemon being replaced dropped filters entirely, logging `unknown option
# filters`. Applications that name their file types are heard here.
check "filters survive and the chosen one comes back" \
    "ua{sv} 0 2 \"uris\" as 1 \"file://$HOME/b.corkboard\" \"current_filter\" (sa(us)) \"Images\" 1 1 \"image/png\"" \
    "$(call SaveFile osssa{sv} /t/5 com.test "" Save 3 \
        current_name s b.corkboard \
        filters "a(sa(us))" 2 Corkboard 1 0 '*.corkboard' Images 1 1 image/png \
        current_filter "(sa(us))" Images 1 1 image/png)"

check "no scratch files are left behind" \
    "" \
    "$(ls -A "$XDG_RUNTIME_DIR/filepick" 2>/dev/null)"

echo
echo "where the dialog opens"

# `suggested` obeys the application. A save answers with a path that does not
# exist yet — that is what a save is. The daemon being replaced stat'ed the
# returned path and abandoned the whole success reply on ENOENT, so every save
# silently did nothing, and it created the file up front to avoid that.
start_daemon "python3 $stub" FILEPICK_SAVE_DIR=suggested
check "suggested: the application's folder is used" \
    "ua{sv} 0 1 \"uris\" as 1 \"file://$HOME/Pictures/brand-new.corkboard\"" \
    "$(call SaveFile osssa{sv} /t/6 com.test "" Save 2 \
        current_name s brand-new.corkboard \
        current_folder ay $(bytes "$HOME/Pictures"))"

check "a path that does not exist is still a valid answer" \
    "absent" \
    "$([[ -e $HOME/Pictures/brand-new.corkboard ]] && echo present || echo absent)"

# Recorded from a path that does not exist, which is where the old daemon's
# equivalent gave up — it stat'ed first, so it never learned anything from a save.
check "the directory it landed in is remembered" \
    "$HOME/Pictures" \
    "$(cat "$XDG_STATE_HOME/filepick/last_dir")"

# `last` is the default for saves and deliberately overrides the application:
# most GTK apps propose ~/Documents for every save regardless of what is being
# saved, so where the previous save landed is the better guess.
start_daemon "python3 $stub"
check "last: the application's folder is overridden" \
    "ua{sv} 0 1 \"uris\" as 1 \"file://$HOME/Pictures/another.corkboard\"" \
    "$(call SaveFile osssa{sv} /t/7 com.test "" Save 2 \
        current_name s another.corkboard \
        current_folder ay $(bytes "$HOME"))"

echo
echo "how a dialog can end"

start_daemon "$(fake_dialog cancel 'printf "%s" "{\"outcome\":\"cancelled\"}" > "$3"')"
check "a declined dialog is response 1, not an error" \
    "ua{sv} 1 0" \
    "$(call SaveFile osssa{sv} /t/8 com.test "" Save 0)"

# Closing the window kills the dialog before it writes anything. That is a
# cancel, not a failure: an application must not see a D-Bus error because
# someone changed their mind.
start_daemon "$(fake_dialog silent 'exit 0')"
check "a dialog that writes nothing is a cancel" \
    "ua{sv} 1 0" \
    "$(call SaveFile osssa{sv} /t/10 com.test "" Save 0)"

start_daemon "$(fake_dialog crash 'exit 3')"
check "a dialog that dies is a cancel too" \
    "ua{sv} 1 0" \
    "$(call SaveFile osssa{sv} /t/11 com.test "" Save 0)"

# An application that gives up first (its own window closed, say) calls Close
# on the request. The dialog must not outlive it as an orphan window with
# nothing behind it.
start_daemon "$(fake_dialog slow 'sleep 30')"
call SaveFile osssa{sv} /t/12 com.test "" Save 0 >/dev/null 2>&1 &
for _ in $(seq 50); do
    pgrep -f "$tmp/fake-slow.sh" >/dev/null && break
    sleep 0.1
done
busctl --user call "$bus" /t/12 org.freedesktop.impl.portal.Request Close >/dev/null 2>&1 || true
sleep 0.5

check "Close kills the dialog" \
    "gone" \
    "$(pgrep -f "$tmp/fake-slow.sh" >/dev/null && echo running || echo gone)"

check "Close cleans up after the dialog" \
    "" \
    "$(ls -A "$XDG_RUNTIME_DIR/filepick" 2>/dev/null)"

# Everything above stubs the dialog out, which proves the daemon and says
# nothing about whether the two halves read the same JSON. Here the terminal is
# the pty harness and the dialog is the real one, driven by keystrokes.
echo
echo "the two halves together"

start_daemon "python3 $root/tests/screen.py --" \
    FILEPICK_SAVE_DIR=suggested \
    FILEPICK_OPEN_DIR=suggested \
    KEYS='\x15board.corkboard\x00\r'
check "a name typed into the real dialog comes back as a URI" \
    "ua{sv} 0 1 \"uris\" as 1 \"file://$HOME/Pictures/board.corkboard\"" \
    "$(call SaveFile osssa{sv} /t/13 com.test "" Save 2 \
        current_name s untitled.corkboard \
        current_folder ay $(bytes "$HOME/Pictures"))"

start_daemon "python3 $root/tests/screen.py --" \
    FILEPICK_OPEN_DIR=suggested \
    KEYS='\x1b'
check "and Esc in the real dialog is a cancel" \
    "ua{sv} 1 0" \
    "$(call OpenFile osssa{sv} /t/14 com.test "" Open 1 current_folder ay $(bytes "$HOME/Pictures"))"

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[[ $fail -eq 0 ]]
