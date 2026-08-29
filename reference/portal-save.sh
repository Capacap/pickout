#!/usr/bin/env bash
# The save half of the file-chooser portal, run inside the terminal window
# portal-wrapper.sh opens. Arguments: the application's suggested path, and
# the file to write the chosen path into (empty means cancelled).
#
# Saving is two questions — which directory, then what name — and a browser
# can only answer the first. So this asks them in order: fzf over every
# directory under $HOME for the first, readline with the suggested name
# already in the buffer for the second. Nothing is navigated; a full sweep of
# $HOME is ~1500 entries and fd returns them in about 12ms, so typing two
# fragments of the path is faster than walking to it.
#
# Deliberately not set -e: every failure here has a specific answer (cancel,
# reprompt, report), and a bare exit would leave the user staring at a window
# that closed for no stated reason.
set -u

suggested="${1:-}"
out="${2:-}"

state="${XDG_STATE_HOME:-$HOME/.local/state}/xdg-desktop-portal-termfilechooser"
log_file="$state/portal-save.log"

# Kept because the failure this replaced could not be diagnosed after the
# fact — the window is gone by the time anything is wrong. Delete the file to
# stop caring; it is rewritten per save, not appended forever.
log() { printf '%s\n' "$*" >> "$log_file" 2>/dev/null || true; }
mkdir -p -- "$state" 2>/dev/null || true
: > "$log_file" 2>/dev/null || true
log "suggested=$suggested out=$out"

printf '\033]0;Save file\007'

# ── Where the app wanted to put it ─────────────────────────────────────────
if [[ -d $suggested ]]; then
    suggested_dir=$suggested
    suggested_name=""
elif [[ -n $suggested ]]; then
    suggested_dir=$(dirname -- "$suggested")
    suggested_name=$(basename -- "$suggested")
else
    suggested_dir=""
    suggested_name=""
fi
[[ -d $suggested_dir ]] || suggested_dir=$HOME

last_dir=$(head -n 1 -- "$state/last_dir" 2>/dev/null) || last_dir=""

# ── Question one: which directory ───────────────────────────────────────────
# Written with $HOME as ~ so the list reads as paths rather than as a column
# of /home/capacap; expanded again after the pick.
tilde() { printf '%s\n' "${1/#$HOME/\~}"; }

dir_list() {
    tilde "$suggested_dir"
    [[ -n $last_dir && -d $last_dir ]] && tilde "$last_dir"
    for d in "$HOME" "$HOME"/Documents "$HOME"/Downloads "$HOME"/Pictures \
             "$HOME"/Videos "$HOME"/Music "$HOME"/Desktop "$HOME"/Projects; do
        [[ -d $d ]] && tilde "$d"
    done
    # fd terminates every directory with a slash; strip it, or each entry
    # arrives as a second spelling of one already in the list above and the
    # dedupe below lets both through.
    fd --type d --absolute-path . "$HOME" 2>/dev/null \
        | sed -e "s|^$HOME|~|" -e 's|\(.\)/$|\1|' 
}

# Slots, not hex: foot.ini is the palette source and everything else names
# ANSI positions so it follows without editing (same as helix and yazi).
# Brightness only, no hue — the selected row is marked by a bright pointer and
# bright text rather than a fill, because slot 0 against the terminal
# background is too small a step to read at a glance.
picked=$(dir_list | awk '!seen[$0]++' | fzf \
    --layout=reverse \
    --info=inline \
    --print-query \
    --tiebreak=begin,index \
    --prompt='Save in  ' \
    --pointer='>' \
    --header='Return chooses the directory   ·   a path typed in full is created   ·   Esc cancels' \
    --color='fg:7,fg+:15,bg:-1,bg+:0,hl:15,hl+:15,info:8,prompt:7,pointer:15,marker:15,header:8,border:8,query:15,spinner:8')
rc=$?
log "fzf rc=$rc out=[${picked//$'\n'/ | }]"

query=$(sed -n 1p <<<"$picked")
match=$(sed -n 2p <<<"$picked")

case $rc in
    0) dir=$match ;;              # a listed directory was selected
    1) dir=$query ;;              # no match: take the typed path literally
    *) log "cancelled at directory"; exit 0 ;;
esac

case $dir in
    "~")   dir=$HOME ;;
    "~/"*) dir=$HOME/${dir#\~/} ;;
    /*)    ;;
    "")    log "empty directory"; exit 0 ;;
    *)     dir=$HOME/$dir ;;      # a bare fragment is relative to $HOME
esac
log "dir=$dir"

# ── Question two: what name ─────────────────────────────────────────────────
printf '\n'
read -e -r -i "$suggested_name" -p 'Save as  ' name || { log "cancelled at name"; exit 0; }
[[ -n $name ]] || { log "empty name"; exit 0; }

case $name in
    /*)    target=$name ;;
    "~/"*) target=$HOME/${name#\~/} ;;
    *)     target=$dir/$name ;;   # may carry directories; they get created
esac

# A directory can never be the answer, and an existing file needs saying so
# out loud. Both reprompt rather than exit, so a mistyped name costs a
# keystroke instead of the whole dialog.
while [[ -e $target ]]; do
    if [[ -d $target ]]; then
        printf '%s is a directory.\n' "$(tilde "$target")"
    else
        read -r -p "Overwrite $(basename -- "$target")? [y/N]  " reply || exit 0
        [[ $reply == [yY]* ]] && break
    fi
    read -e -r -i "$(basename -- "$target")" -p 'Save as  ' name || exit 0
    [[ -n $name ]] || exit 0
    target=$(dirname -- "$target")/$name
done
log "target=$target"

target_dir=$(dirname -- "$target")
if ! mkdir -p -- "$target_dir" 2>/dev/null; then
    printf '\nCannot create %s\n' "$(tilde "$target_dir")"
    read -r -n 1 -p 'Return to close  ' _ || true
    log "mkdir failed for $target_dir"
    exit 0
fi

# The portal derives save_mode=last from the path returned here, but its
# set_last_dir() stats that path first and gives up on ENOENT — so a file that
# does not exist yet, which is nearly every save, would never update it. Write
# the state directly. The portal's own attempt runs afterwards and bails the
# same way, leaving this value standing.
printf '%s\n' "$target_dir" > "$state/last_dir" 2>/dev/null || true

printf '%s\n' "$target" > "$out"
log "wrote $target"
