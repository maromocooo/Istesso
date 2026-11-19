#!/bin/bash
# Checks that every translation of README.md says which English version it follows, and that a
# translation whose English has changed since is marked as out of date where readers can see it.
#
#   scripts/check-translations.sh                 check every README.<language>.md
#   scripts/check-translations.sh --mark-stale    add the visible "out of date" line to each translation
#                                                 whose English has changed, then check
#   scripts/check-translations.sh --stamp FILE    after FILE was translated again and reviewed: point its
#                                                 marker at the current README.md, take the "out of date"
#                                                 line away, then check
#
# Ends with 0 when every translation is current, or out of date with the visible line; 1 when a
# translation is out of date without it; 2 when a marker is missing or malformed, a file named in the
# language line is missing (or one is there that the line does not name), or a translation that says
# it is current does not have the shape of the English; 64 on wrong use. One line per translation.
# Before the first translation, README.md has no language line and there is no README.<language>.md:
# that is 0, with nothing to check.
#
# The head of a translation (README.md itself starts with the language line):
#
#   <!-- translation-of: README.md @ blob <git hash-object README.md> (YYYY-MM-DD) -->
#   [the out-of-date line and an empty line, only while the English is newer]
#   <the language line: this language in bold, every other one a link>
#
#   > <a note in this language: a translation, the English is canonical (a link to README.md),
#   >  follows the English of YYYY-MM-DD — the same date as the marker>
#
# The marker holds the hash of the English file's content, not a commit, so that it survives merges
# that rewrite commits and can be compared in a clone without history. `git diff <blob> HEAD:README.md`
# shows what changed in the English since. A translation that says it is current must have, compared
# with the English: the same number of headings, the code blocks letter for letter, the status words
# (`Verified`, `Trial`, `Planned`, `Out of scope`) in the same order, and the language line of its own.
# Needs bash, git, grep, sed, awk and nothing else.
set -u

stale_line='> **This translation is out of date.** The English [README.md](README.md) has changed since it was made; read that for the current content.'
marker_re='^<!-- translation-of: README\.md @ blob ([0-9a-f]{40}) \(([0-9]{4}-[0-9]{2}-[0-9]{2})\) -->$'
item_re='(\*\*[^*]+\*\*|\[[^]]+\]\(README(\.[A-Za-z-]+)?\.md\))'
langline_re="^$item_re( · $item_re)+\$"

usage() { echo "usage: scripts/check-translations.sh [--mark-stale | --stamp README.<language>.md]" >&2; exit 64; }

mode=check; stamp_file=""
case $# in
    0) ;;
    1) [ "$1" = --mark-stale ] && mode=mark || usage ;;
    2) [ "$1" = --stamp ] || usage; mode=stamp; stamp_file=${2#./} ;;
    *) usage ;;
esac

root=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "not in a git repository" >&2; exit 64; }
cd "$root" || exit 64
[ -f README.md ] || { echo "no README.md at $root" >&2; exit 2; }
english_blob=$(git hash-object README.md) || exit 2

# The language line of README.md names every language and its file, in order. Its own entry is the
# bold one, and its file is README.md.
# Without a language line there must be no translations yet: the line goes in together with them,
# so that no link points at a file that is not there.
english_line=$(grep -m1 -E "$langline_re" README.md)
if [ -z "$english_line" ]; then
    [ "$mode" = stamp ] && { echo "README.md has no language line, so there is nothing to stamp" >&2; exit 64; }
    found=""
    for f in README.*.md; do [ -f "$f" ] && found="$found $f"; done
    if [ -n "$found" ]; then echo "broken: README.md has no language line, but there are translations:$found"; exit 2; fi
    echo "no translations yet (README.md has no language line and there is no README.<language>.md)"
    exit 0
fi
names=(); files=()
rest=$english_line
while [ -n "$rest" ]; do
    item=${rest%% · *}
    if [ "$item" = "$rest" ]; then rest=""; else rest=${rest#* · }; fi
    case $item in
        \*\**) names+=("$(printf '%s' "$item" | sed -E 's/^\*\*(.*)\*\*$/\1/')"); files+=(README.md) ;;
        *) names+=("$(printf '%s' "$item" | sed -E 's/^\[(.*)\]\((.*)\)$/\1/')")
           files+=("$(printf '%s' "$item" | sed -E 's/^\[(.*)\]\((.*)\)$/\2/')") ;;
    esac
done

# The language line a given file should carry: itself in bold, the others as links.
expected_line() {
    local own=$1 i out=""
    for i in "${!files[@]}"; do
        [ -n "$out" ] && out="$out · "
        if [ "${files[$i]}" = "$own" ]; then out="$out**${names[$i]}**"; else out="$out[${names[$i]}](${files[$i]})"; fi
    done
    printf '%s' "$out"
}

is_listed() { local f; for f in "${files[@]}"; do [ "$f" = "$1" ] && return 0; done; return 1; }

# The number of headings outside code blocks; the code blocks themselves; the status words in order.
headings() { awk '/^ *(```|~~~)/ { fence = !fence; next } !fence && /^#+ / { n++ } END { print n + 0 }' "$1"; }
code_blocks() { awk '/^ *(```|~~~)/ { print; fence = !fence; next } fence { print }' "$1"; }
status_words() { grep -o -E '`(Verified|Trial|Planned|Out of scope)`' "$1"; }

# Prints the state of one translation and returns 0 (current, or out of date with the line),
# 1 (out of date without it) or 2 (broken). $1 is the file to read, $2 the name it has in the line.
check_one() {
    local path=$1 name=$2 first blob date note
    first=$(sed -n 1p "$path")
    if ! printf '%s\n' "$first" | grep -q -E "$marker_re"; then
        echo "broken: $name (the first line is not a translation-of marker)"; return 2
    fi
    blob=$(printf '%s\n' "$first" | sed -E "s/$marker_re/\1/")
    date=$(printf '%s\n' "$first" | sed -E "s/$marker_re/\2/")
    if [ "$blob" != "$english_blob" ]; then
        if [ "$(sed -n 2p "$path")" = "$stale_line" ]; then
            echo "stale (marked): $name (follows blob ${blob:0:12}; README.md is now ${english_blob:0:12})"; return 0
        fi
        echo "stale (unmarked): $name (follows blob ${blob:0:12}; README.md is now ${english_blob:0:12}; run --mark-stale)"; return 1
    fi
    if grep -q -x -F -- "$stale_line" "$path"; then
        echo "broken: $name (the marker is current but the out-of-date line is still there)"; return 2
    fi
    if [ "$(grep -m1 -E "$langline_re" "$path")" != "$(expected_line "$name")" ]; then
        echo "broken: $name (the language line is not: $(expected_line "$name"))"; return 2
    fi
    note=$(grep -m1 -E '^> ' "$path")
    case $note in
        *"(README.md)"*"$date"* | *"$date"*"(README.md)"*) ;;
        *) echo "broken: $name (the first '> ' note does not link README.md and say $date)"; return 2 ;;
    esac
    if [ "$(headings "$path")" != "$(headings README.md)" ]; then
        echo "broken: $name ($(headings "$path") headings; README.md has $(headings README.md))"; return 2
    fi
    if [ "$(code_blocks "$path")" != "$(code_blocks README.md)" ]; then
        echo "broken: $name (its code blocks differ from those of README.md)"; return 2
    fi
    if [ "$(status_words "$path")" != "$(status_words README.md)" ]; then
        echo "broken: $name (its status words differ from those of README.md, or come in another order)"; return 2
    fi
    echo "current: $name"; return 0
}

if [ "$mode" = stamp ]; then
    case $stamp_file in README.md | */*) usage ;; esac
    is_listed "$stamp_file" || { echo "not a translation named in the language line of README.md: $stamp_file" >&2; exit 64; }
    [ -f "$stamp_file" ] || { echo "no such file: $stamp_file" >&2; exit 2; }
    old=$(sed -n 1p "$stamp_file")
    printf '%s\n' "$old" | grep -q -E "$marker_re" || { echo "broken: $stamp_file (no marker to update)"; exit 2; }
    old_date=$(printf '%s\n' "$old" | sed -E "s/$marker_re/\2/")
    today=$(date +%Y-%m-%d)
    new="$stamp_file.stamp.$$"
    trap 'rm -f -- "$new"' EXIT
    {
        printf '<!-- translation-of: README.md @ blob %s (%s) -->\n' "$english_blob" "$today"
        # Drop the out-of-date line and the empty line after it; put the new date in the note.
        awk -v stale="$stale_line" -v old="$old_date" -v new="$today" '
            NR == 1 { next }
            NR == 2 && $0 == stale { skip = 1; next }
            NR == 3 && skip && $0 == "" { next }
            !noted && /^> / { i = index($0, old); if (i) $0 = substr($0, 1, i - 1) new substr($0, i + length(old)); noted = 1 }
            { print }' "$stamp_file"
    } > "$new"
    # Write it only if the result has the shape of the English.
    check_one "$new" "$stamp_file" > /dev/null
    if [ $? -ne 0 ]; then
        echo "not stamped: $stamp_file would not match README.md:"
        check_one "$new" "$stamp_file"
        exit 2
    fi
    mv -f -- "$new" "$stamp_file"
    trap - EXIT
fi

if [ "$mode" = mark ]; then
    for i in "${!files[@]}"; do
        f=${files[$i]}
        [ "$f" = README.md ] || [ ! -f "$f" ] && continue
        first=$(sed -n 1p "$f")
        printf '%s\n' "$first" | grep -q -E "$marker_re" || continue
        [ "$(printf '%s\n' "$first" | sed -E "s/$marker_re/\1/")" = "$english_blob" ] && continue
        [ "$(sed -n 2p "$f")" = "$stale_line" ] && continue
        tmp="$f.mark.$$"
        { printf '%s\n%s\n\n' "$first" "$stale_line"; sed 1d "$f"; } > "$tmp" && mv -f -- "$tmp" "$f"
    done
fi

worst=0
note() { [ "$1" -gt "$worst" ] && worst=$1; return 0; }
for i in "${!files[@]}"; do
    f=${files[$i]}
    [ "$f" = README.md ] && continue
    if [ ! -f "$f" ]; then echo "broken: $f (named in the language line of README.md, but missing)"; note 2; continue; fi
    check_one "$f" "$f"; note $?
done
for f in README.*.md; do
    [ -f "$f" ] || continue
    is_listed "$f" || { echo "broken: $f (not named in the language line of README.md)"; note 2; }
done
exit "$worst"
