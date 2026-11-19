#!/bin/bash
# Runs scripts/check-translations.sh on made-up READMEs in a repository of their own.
set -eu

script="$(cd -- "$(dirname -- "$0")/.." && pwd)/check-translations.sh"
case_dir=$(mktemp -d "${TMPDIR:-/tmp}/check-translations-test.XXXXXX")
trap 'rm -rf -- "$case_dir"' EXIT
cd "$case_dir"
git init -q .

failed=0
expect() { # expected exit code, label, then the arguments for the script
    local expected=$1 label=$2 actual=0
    shift 2
    "$script" "$@" > "$case_dir/.out" 2>&1 || actual=$?
    if [[ $actual -eq $expected ]]; then
        printf 'PASS %s\n' "$label"
    else
        printf 'FAIL %s (expected exit %d, got %d)\n' "$label" "$expected" "$actual"
        sed 's/^/    /' "$case_dir/.out"
        failed=1
    fi
}
has() { # label, file, fixed line that must be there (or, with !, must not)
    local label=$1 file=$2 line=$3 want=yes
    [[ $line == '!'* ]] && { want=no; line=${line#!}; }
    if grep -q -x -F -- "$line" "$file"; then got=yes; else got=no; fi
    if [[ $got == "$want" ]]; then printf 'PASS %s\n' "$label"; else printf 'FAIL %s\n' "$label"; failed=1; fi
}

stale='> **This translation is out of date.** The English [README.md](README.md) has changed since it was made; read that for the current content.'
today=$(date +%Y-%m-%d)

write_english() {
    cat > README.md <<'EOF'
**English** · [Testisch](README.tt.md) · [Probe](README.pr.md)

# Fixture

Made-up text.

## Status

| Thing | State |
|---|---|
| One | `Verified` 2026-01-02 |
| Two | `Trial` |

## Use

```sh
fixture run
```
EOF
}

# A translation of the current README.md. $1 file, $2 language line, $3 date, $4 extra body text.
write_translation() {
    local blob; blob=$(git hash-object README.md)
    cat > "$1" <<EOF
<!-- translation-of: README.md @ blob $blob ($3) -->
$2

> Translated note: the English [README.md](README.md) is canonical; this follows it as of $3.

# Fixture (translated)

Made-up translated text.$4

## Status (translated)

| Thing | State |
|---|---|
| One | \`Verified\` 2026-01-02 |
| Two | \`Trial\` |

## Use (translated)

\`\`\`sh
fixture run
\`\`\`
EOF
}
tt_line='[English](README.md) · **Testisch** · [Probe](README.pr.md)'
pr_line='[English](README.md) · [Testisch](README.tt.md) · **Probe**'
fresh() { write_english; write_translation README.tt.md "$tt_line" 2026-01-03 ""; write_translation README.pr.md "$pr_line" 2026-01-03 ""; rm -f README.xx.md; }

fresh
expect 0 both-current
expect 64 unknown-option --check
expect 64 stamp-without-file --stamp
expect 64 stamp-the-english --stamp README.md
expect 64 stamp-an-unlisted-file --stamp README.xx.md

echo "Changed." >> README.md
expect 1 english-changed-unmarked
expect 0 mark-stale --mark-stale
has stale-line-added README.tt.md "$stale"
expect 0 marked-stays-zero
expect 0 mark-stale-twice-is-the-same --mark-stale
[[ $(grep -c -x -F -- "$stale" README.pr.md) -eq 1 ]] && echo 'PASS stale-line-once' || { echo 'FAIL stale-line-once'; failed=1; }

expect 0 stamp-one --stamp README.tt.md
has stamp-removes-stale-line README.tt.md "!$stale"
has stamp-dates-the-note README.tt.md "> Translated note: the English [README.md](README.md) is canonical; this follows it as of $today."
has stamp-keeps-the-other-marked README.pr.md "$stale"
[[ $(sed -n 2p README.tt.md) == "$tt_line" ]] && echo 'PASS stamp-drops-the-empty-line' || { echo 'FAIL stamp-drops-the-empty-line'; failed=1; }

fresh
sed -i.bak 's/^fixture run$/fixture walk/' README.tt.md && rm README.tt.md.bak
expect 2 current-but-code-differs
fresh
printf '\n## Extra heading\n' >> README.tt.md
expect 2 current-but-headings-differ
fresh
sed -i.bak 's/`Trial`/`Planned`/' README.pr.md && rm README.pr.md.bak
expect 2 current-but-status-words-differ
fresh
write_translation README.tt.md '**English** · [Testisch](README.tt.md) · [Probe](README.pr.md)' 2026-01-03 ""
expect 2 wrong-language-line
fresh
sed -i.bak 's/as of 2026-01-03/as of 2026-01-01/' README.tt.md && rm README.tt.md.bak
expect 2 note-date-differs-from-marker
fresh
sed -i.bak 1d README.pr.md && rm README.pr.md.bak
expect 2 no-marker
fresh
rm README.pr.md
expect 2 named-file-missing
fresh
cp README.tt.md README.xx.md
expect 2 file-not-in-language-line
fresh
{ sed -n 1p README.tt.md; echo "$stale"; echo; sed 1d README.tt.md; } > README.tt.new && mv README.tt.new README.tt.md
expect 2 current-but-marked-stale

# Stamping a translation that does not have the shape of the English leaves it as it was.
fresh
echo "Changed." >> README.md
expect 0 mark-before-a-bad-stamp --mark-stale
printf '\n## Extra heading\n' >> README.pr.md
before=$(git hash-object README.pr.md)
expect 2 stamp-refused --stamp README.pr.md
[[ $(git hash-object README.pr.md) == "$before" ]] && echo 'PASS refused-stamp-leaves-file' || { echo 'FAIL refused-stamp-leaves-file'; failed=1; }
ls README.pr.md.stamp.* > /dev/null 2>&1 && { echo 'FAIL refused-stamp-leaves-no-temporary-file'; failed=1; } || echo 'PASS refused-stamp-leaves-no-temporary-file'

# Before the first translation: no language line in README.md, and no translation files.
fresh
rm -f README.tt.md README.pr.md
sed -i.bak 1d README.md && rm README.md.bak
expect 0 no-line-no-translations
expect 0 no-line-mark-stale --mark-stale
expect 64 no-line-stamp --stamp README.tt.md
write_translation README.tt.md "$tt_line" 2026-01-03 ""
expect 2 no-line-but-a-translation

exit "$failed"
