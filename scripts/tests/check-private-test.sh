#!/bin/bash
set -eu

cd -- "$(dirname -- "$0")/../.."
case_dir=$(mktemp -d "${TMPDIR:-/tmp}/check-private-test.XXXXXX")
trap 'rm -rf -- "$case_dir"' EXIT
# Keep the private word list independent of local configuration.
export ISTESSO_PRIVATE_WORDS="$case_dir/private-words"

failed=0
check_case() {
    local expected=$1 label=$2 input=$3 actual=0
    local file="$case_dir/$label"
    printf '%s\n' "$input" > "$file"
    scripts/check-private.sh --file "$file" > /dev/null || actual=$?
    if [[ $actual -eq $expected ]]; then
        printf 'PASS %s\n' "$label"
    else
        printf 'FAIL %s (expected exit %d, got %d)\n' "$label" "$expected" "$actual"
        failed=1
    fi
}

check_case 0 github-user-noreply 'Fixture <12345+test-fixture@users.noreply.github.com>'
check_case 0 github-noreply 'noreply@github.com'
check_case 0 github-noreply-full-stop 'Contact noreply@github.com.'
check_case 0 anthropic-trailer 'Co-Authored-By: Fixture <noreply@anthropic.com>'
check_case 0 github-ssh-remote 'git@github.com:owner/repo.git'
check_case 0 loopback '127.0.0.1'
check_case 0 test-net-1 '192.0.2.17'
check_case 0 test-net-2 '198.51.100.17'
check_case 0 test-net-3 '203.0.113.17'
check_case 0 shared-folder '/Users/Shared'
check_case 0 four-part-version 'version 1.2.3.4'

check_case 0 any-address '0.0.0.0'
check_case 0 runner-folder '/home/runner/x'
check_case 0 placeholder-folder '/Users/me/x'
check_case 0 shared-subfolder '/Users/Shared/x'
check_case 0 marked-email 'fixture@example.org # check-private: ok (invented mailbox for the marker test)'

check_case 1 ordinary-email 'fixture@example.org' # check-private: ok (invented mailbox)
check_case 1 joined-at-left 'fixture@noreply@github.com' # check-private: ok (invented joined address)
check_case 1 joined-at-right 'noreply@github.com@example.org' # check-private: ok (invented joined address)
check_case 1 dotted-local-part 'fixture.12345+test-fixture@users.noreply.github.com' # check-private: ok (invented longer local part)
check_case 1 extended-domain 'noreply@github.com.example.org' # check-private: ok (invented domain suffix)
check_case 1 private-ipv4 '10.23.45.67' # check-private: ok (invented private-range address)
check_case 1 mac-address '02:ab:cd:00:00:01' # check-private: ok (invented locally administered address)
check_case 1 linux-home '/home/test-fixture' # check-private: ok (invented home folder)
check_case 1 macos-home '/Users/test-fixture' # check-private: ok (invented home folder)
check_case 1 chat-share 'https://chatgpt.com/share/test-fixture' # check-private: ok (invented share link)

check_case 1 anthropic-dotted-local 'fixture.noreply@anthropic.com' # check-private: ok (invented longer local part)
check_case 1 git-extended-domain 'git@github.com.example.org' # check-private: ok (invented domain suffix)
check_case 1 user-noreply-joined '12345+test-fixture@users.noreply.github.com@example.org' # check-private: ok (invented joined address)
check_case 1 user-noreply-extended-domain '12345+test-fixture@users.noreply.github.com.example.org' # check-private: ok (invented domain suffix)
check_case 1 runner-prefix '/home/runnerx/x' # check-private: ok (invented home folder)
check_case 1 shared-prefix '/Users/Sharedfoo/x' # check-private: ok (invented home folder)
check_case 1 shared-hyphen '/Users/Shared-foo/x' # check-private: ok (invented home folder)
check_case 1 unique-local-ipv6 'fd00:abcd::1' # check-private: ok (invented unique-local address)
check_case 1 chat-session 'https://chatgpt.com/c/test-fixture' # check-private: ok (invented session link)

printf '%s\n' 'synthetic-fixture-word' > "$ISTESSO_PRIVATE_WORDS"
check_case 1 private-word 'Found synthetic-fixture-word here.' # check-private: ok (invented private word)

exit "$failed"
