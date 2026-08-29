#!/usr/bin/env bash
# The chooser, driven through a pty with no human in the loop.
#
# Every case is a request file, a string of keystrokes and an assertion about
# either the screen that was drawn or the response that was written. Nothing
# here goes near D-Bus — `tests/portal.sh` covers that half — so a failure
# points at the interface rather than at the plumbing under it.
#
# Two things the pty harness taught, both still live: keys arrive in paced
# chunks separated by \x00, because a burst gets eaten by an application in raw
# mode; and a real Return is CR (\r), never LF.
set -u

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
binary="$root/target/debug/filepick"
screen="$root/tests/screen.py"

if [[ ! -x $binary ]]; then
    echo "build first: cargo build" >&2
    exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
export HOME=$tmp

# A tree with one of everything the interface has to say something about.
mkdir -p "$tmp/Pictures/Screenshots" "$tmp/Pictures/ref"
head -c 31500 /dev/urandom > "$tmp/Pictures/bern_copy.corkboard"
head -c 1200000 /dev/urandom > "$tmp/Pictures/photo.png"
head -c 900 /dev/urandom > "$tmp/Pictures/.hidden_note"

passed=0
failed=0

check() { # check <name> <expected> <actual>
    if [[ $2 == "$3" ]]; then
        printf '  ok    %s\n' "$1"
        passed=$((passed + 1))
    else
        printf '  FAIL  %s\n        expected: %s\n        actual:   %s\n' "$1" "$2" "$3"
        failed=$((failed + 1))
    fi
}

request() { # request <name> <json>
    printf '%s\n' "$2" > "$tmp/$1.json"
}

# The four questions, as the daemon would pose them.
request save '{
  "mode": "save_file",
  "title": "Save board",
  "accept_label": null,
  "start_dir": "'"$tmp"'/Pictures",
  "current_name": "untitled.corkboard",
  "filters": [],
  "current_filter": null
}'
request open '{
  "mode": {"open_file": {"multiple": false}},
  "title": "Open board",
  "accept_label": null,
  "start_dir": "'"$tmp"'/Pictures",
  "current_name": null,
  "filters": [],
  "current_filter": null
}'
request folder '{
  "mode": {"open_directory": {"multiple": false}},
  "title": "Choose folder",
  "accept_label": null,
  "start_dir": "'"$tmp"'/Pictures",
  "current_name": null,
  "filters": [],
  "current_filter": null
}'
request many '{
  "mode": {"save_files": {"names": ["one.txt", "two.txt"]}},
  "title": "Save into",
  "accept_label": "_Export",
  "start_dir": "'"$tmp"'/Pictures",
  "current_name": null,
  "filters": [],
  "current_filter": null
}'

# run <request> <keys> — leaves the screen in $out and the answer in $answer.
run() {
    rm -f "$tmp/response.json"
    KEYS=$2 python3 "$screen" -- \
        "$binary" --dialog "$tmp/$1.json" "$tmp/response.json" > "$tmp/screen.txt" 2>&1
    out=$(cat "$tmp/screen.txt")
    answer=$(cat "$tmp/response.json" 2>/dev/null || true)
}

# The content of one screen row, without the border or its padding.
row() {
    sed -n "${1}p" "$tmp/screen.txt" | sed -e 's/^│ *//' -e 's/ *│$//' -e 's/  */ /g'
}

paths() { # the paths in the response, with $HOME written back as ~
    python3 -c '
import json, os, sys
try:
    answer = json.load(open(sys.argv[1]))
except OSError:
    print("<no response>")
    sys.exit()
if answer["outcome"] != "selected":
    print(answer["outcome"])
else:
    home = os.environ["HOME"]
    print(" ".join(p.replace(home, "~", 1) for p in answer["paths"]))
' "$tmp/response.json"
}

echo
echo "the browser"

run save ''
check "the title is the application's" "┌─ Save board ─" "${out:0:15}"
check "the path bar abbreviates \$HOME" "~/Pictures" "$(row 2)"
check "the cursor starts below .." "> ref/" "$(row 5)"
check "sizes are shown for files only" "bern_copy.corkboard 31.5 kB" "$(row 7)"
check "dotfiles are hidden by default" "photo.png 1.2 MB" "$(row 8)"

run save '\t\x00\x1bh'
check "M-h reveals dotfiles" ".hidden_note 900 B" "$(row 7)"

run save '\t\x00pho'
check "typing filters the listing" "> photo.png 1.2 MB" "$(row 5)"
check "the query shows in the path bar" "~/Pictures /pho" "$(row 2)"

run save '\t\x00.hid'
check "a query starting with a dot finds dotfiles" "> .hidden_note 900 B" "$(row 5)"

run save '\t\x00pho\x00\x1b'
check "Esc clears the query before it cancels" "~/Pictures" "$(row 2)"

run save '\t\x00\x1b[B\x00\r'
check "Enter descends into a directory" "~/Pictures/Screenshots" "$(row 2)"

run save '\t\x00\x1b[B\x00\r\x00\x1b[D'
check "Left comes back up" "~/Pictures" "$(row 2)"
check "and lands on the directory just left" "> Screenshots/" "$(row 6)"

echo
echo "the name field"

run save ''
check "the suggested name is pre-filled" "Name untitled.corkboard" "$(row 38)"

run save '\x15photo.png'
check "a name that exists says so" "Name photo.png exists" "$(row 38)"

run save '\x15deep/er/a.txt'
check "a name carrying directories says what it would make" \
    "Name deep/er/a.txt creates 2 directories" "$(row 38)"

run save '\x15ref'
check "a directory is not a filename" "Name ref is a directory" "$(row 38)"

run save '\x15bern_copy.corkboard/x'
check "a file standing in for a directory blocks the save" \
    "Name bern_copy.corkboard/x bern_copy.corkboard is a file" "$(row 38)"

run save '\t\x00\x1b[B\x1b[B\x00\r'
check "Enter on a file copies its name rather than committing" \
    "Name bern_copy.corkboard exists" "$(row 38)"
check "and writes nothing" "<no response>" "$(paths)"

echo
echo "how a dialog ends"

run save '\r'
check "Enter in the name field commits" "~/Pictures/untitled.corkboard" "$(paths)"
check "and creates nothing on disk" "absent" \
    "$([[ -e $tmp/Pictures/untitled.corkboard ]] && echo present || echo absent)"

run save '\x1b'
check "Esc cancels" "cancelled" "$(paths)"

run save '\x15photo.png\x00\r'
check "the first Enter on an existing name only arms it" "<no response>" "$(paths)"
check "and says so where the label was" \
    "Replace photo.png Enter again to replace" "$(row 38)"

run save '\x15photo.png\x00\r\x00\r'
check "the second Enter goes through" "~/Pictures/photo.png" "$(paths)"

run save '\x15photo.png\x00\r\x00x\x00\r'
check "editing the name disarms it" "~/Pictures/photo.pngx" "$(paths)"

run save '\x15deep/er/a.txt\x00\r'
check "committing makes the missing directories" "~/Pictures/deep/er/a.txt" "$(paths)"
check "but still not the file" "absent" \
    "$([[ -d $tmp/Pictures/deep/er && ! -e $tmp/Pictures/deep/er/a.txt ]] && echo absent || echo wrong)"
rm -rf "$tmp/Pictures/deep"

run save '\x0c\x00\x15~/Pictures/Screenshots\x00\r'
check "^L takes a path typed in full" "~/Pictures/Screenshots" "$(row 2)"

run save '\x0c\x00\x15~/Pictures/new.corkboard\x00\r'
check "a file typed into the path bar splits into both answers" \
    "Name new.corkboard" "$(row 38)"

echo
echo "the other modes"

run open ''
check "open mode has no name field" "" "$(row 38)"
check "and says so in the footer" "Enter open ^L path M-h hidden Esc cancel" \
    "$(sed -n '40p' "$tmp/screen.txt" | sed -e 's/^ *//' -e 's/ *$//' -e 's/  */ /g')"

run open '\x1b[B\x1b[B\x00\r'
check "Enter on a file opens it" "~/Pictures/bern_copy.corkboard" "$(paths)"

run open '\r'
check "Enter on a directory still descends" "~/Pictures/ref" "$(row 2)"

run folder '\x13'
check "^S chooses the directory the cursor is on" "~/Pictures/ref" "$(paths)"

run folder '\x1b[A\x00\x13'
check "and .. chooses the one being browsed" "~/Pictures" "$(paths)"

run many '\x13'
check "SaveFiles answers with the application's names" \
    "~/Pictures/ref/one.txt ~/Pictures/ref/two.txt" "$(paths)"
check "the accept label loses its mnemonic and joins the other hints" \
    "^S export here Enter open ^L path M-h hidden Esc cancel" \
    "$(sed -n '40p' "$tmp/screen.txt" | sed -e 's/^ *//' -e 's/ *$//' -e 's/  */ /g')"

echo
printf '%d passed, %d failed\n\n' "$passed" "$failed"
[[ $failed -eq 0 ]]
