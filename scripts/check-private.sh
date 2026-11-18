#!/bin/bash
# Looks for private details in what is about to become public: names, addresses, home folders.
#
#   scripts/check-private.sh                 the commits of this branch that origin/main does not have
#   scripts/check-private.sh RANGE           a git range, e.g. origin/m1-latency..HEAD (diff and messages)
#   scripts/check-private.sh --file FILE…    files as they are (a PR body, a record before it is committed)
#
# Ends with 0 when nothing was found, 1 when something was (the lines are printed), 2 on wrong use.
# Only added lines of a diff are looked at, and commit messages whole. A line that carries the words
# `check-private: ok` is passed over: for an invented address in a test that has to look like a real
# one, with the reason next to it.
#
# What it knows by itself: IPv4 addresses (not loopback, not the ranges kept for documentation),
# IPv6 addresses of a local network, hardware addresses, home folders with a user's name in them,
# e-mail addresses other than GitHub's noreply ones, links to AI sessions.
# What only this machine knows — the person's and the machines' names, network names, serial
# numbers — is read from a file that is never committed: one extended regular expression a line in
# ~/.config/istesso/private-words (or the file ISTESSO_PRIVATE_WORDS names), matched anywhere and in
# either case; and, for words that are also ordinary words (network names), one a line in the file
# of the same name ending in -exact, matched as whole words in their own case. Without those files
# the check still runs and says that it ran without them.
set -u
words=${ISTESSO_PRIVATE_WORDS:-$HOME/.config/istesso/private-words}

known='(^|[^0-9.])((1[0-9]{2}|2[0-4][0-9]|25[0-5]|[1-9]?[0-9])\.){3}(1[0-9]{2}|2[0-4][0-9]|25[0-5]|[1-9]?[0-9])([^0-9.]|$)'
known="$known|(^|[^0-9a-fA-F:])f[cde][0-9a-fA-F]{2}:[0-9a-fA-F:]{4,}"
known="$known|(^|[^0-9a-fA-F:])([0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}([^0-9a-fA-F:]|$)"
known="$known|/Users/[A-Za-z0-9._-]+|/home/[a-z][a-z0-9._-]*"
known="$known|[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}"
known="$known|claude\.ai/(chat|code|share)|chatgpt\.com/(c|share)/"  # check-private: ok (the pattern itself)
# Not private: loopback, "any address", the documentation ranges, version-like numbers in lock
# files, GitHub's and Anthropic's noreply addresses (GitHub's own one commits the merges made on
# its site), and the folders the scripts name on purpose. GitHub's two kinds count only as a whole
# address: a longer address that merely contains one, or goes on after it, stays visible.
end='(\.?([^A-Za-z0-9_.%+-]|$))'
allowed='127\.0\.0\.[0-9]+|0\.0\.0\.0|192\.0\.2\.[0-9]+|198\.51\.100\.[0-9]+|203\.0\.113\.[0-9]+|255\.255\.255\.[0-9]+'
allowed="$allowed|[0-9+]*[A-Za-z0-9-]*@users\.noreply\.github\.com$end|(^|[^A-Za-z0-9_.%+-])noreply@github\.com$end"
allowed="$allowed|noreply@anthropic\.com|git@github\.com"
allowed="$allowed|02:00:00:00:00:[0-9a-fA-F]{2}"
# A version number of four parts, said to be one; the folder every user of a Mac shares.
allowed="$allowed|(firmware|version|版は|版（) ?[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+|/Users/Shared"
allowed="$allowed|/home/(u|user|you|me|runner)([/ \"'\`]|$)|/Users/(u|user|you|me|runner)([/ \"'\`]|$)"

look() { # stdin: lines to look at; $1: what they are
    local hits
    local text; text=$(grep -v 'check-private: ok' | sed -E "s~$allowed~~g")
    hits=$( { echo "$text" | grep -n -E -e "$known"
              [ -n "$have_words" ] && echo "$text" | grep -n -E -i -f "$words"
              [ -n "$have_exact" ] && echo "$text" | grep -n -E -f "$words-exact"
            } | sort -t: -k1,1n -u | cut -c1-200)
    [ -z "$hits" ] && return 0
    echo "--- in $1:"; echo "$hits"; return 1
}

found=0
have_words=""; have_exact=""
[ -s "$words-exact" ] && have_exact=1
if [ -s "$words" ]; then have_words=1; else echo "note: no private words file at this machine ($words); only the general patterns are checked"; fi

if [ "${1:-}" = --file ]; then
    shift
    [ $# -gt 0 ] || { echo "usage: scripts/check-private.sh --file FILE…" >&2; exit 2; }
    for file in "$@"; do
        [ -f "$file" ] || { echo "no such file: $file" >&2; exit 2; }
        look "$(basename "$file")" < "$file" || found=1
    done
else
    range=${1:-origin/main..HEAD}
    git rev-parse --git-dir >/dev/null 2>&1 || { echo "not in a git repository" >&2; exit 2; }
    git log --format='%an <%ae>%n%cn <%ce>%n%B' "$range" >/dev/null 2>&1 || { echo "not a range git knows: $range" >&2; exit 2; }
    git diff "${range/../...}" -- . ':(exclude)Cargo.lock' | grep -E '^\+' | grep -v -E '^\+\+\+ ' | look "the added lines of $range" || found=1
    git log --format='%an <%ae>%n%cn <%ce>%n%B' "$range" | look "the commit messages and authors of $range" || found=1
fi
[ "$found" = 0 ] && echo "nothing private found (general patterns${have_words:+ and the words of this machine})"
exit "$found"
