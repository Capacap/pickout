#!/usr/bin/env sh
# Invoked by xdg-desktop-portal-termfilechooser; the argument contract is
# documented in xdg-desktop-portal-termfilechooser(5). Selected by cmd= in the
# config beside this file, whose search path reaches this directory before
# /usr/share — the stock yazi-wrapper.sh is still installed there untouched,
# and putting its name back in cmd= is the whole of the way back.
#
# Opening is yazi's job and it is good at it, so those modes are the stock
# wrapper's code unchanged. Saving is not a browsing problem and yazi is the
# wrong shape for it: --chooser-file makes Return commit to whatever the
# cursor sits on, which is the same key that descends into a directory, so
# there is no way to say "this directory, under a new name". Save mode goes to
# portal-save.sh instead — see the header there.

multiple="$1"
directory="$2"
save="$3"
path="$4"
out="$5"
debug="$6"

set -e
if [ "$debug" = 1 ]; then
    set -x
fi

termcmd="${TERMCMD:-kitty --title termfilechooser}"
here=$(dirname -- "$(readlink -f -- "$0")")

# TERMCMD arrives as a string of words rather than an argv, so the invocation
# has to be reassembled as one string for sh -c; arguments are quoted on the
# way in. Same dance as the stock wrapper, for the same reason.
run_in_terminal() {
    _command="$termcmd"
    for _arg in "$@"; do
        _escaped=$(printf '%s' "$_arg" | sed 's/"/\\"/g')
        _command="$_command \"$_escaped\""
    done
    sh -c "$_command"
}

if [ "$save" = "1" ]; then
    run_in_terminal bash "$here/portal-save.sh" "$path" "$out"
    exit 0
fi

if [ "$directory" = "1" ]; then
    run_in_terminal yazi --chooser-file="$out" --cwd-file="$out.1" "$path"
    if [ ! -s "$out" ] && [ -s "$out.1" ]; then
        cat "$out.1" > "$out"
    fi
    rm -f "$out.1"
else
    # multiple=1 needs no different arguments; yazi's chooser file carries one
    # line per selected entry either way.
    run_in_terminal yazi --chooser-file="$out" "$path"
fi
