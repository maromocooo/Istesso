# Tests of the parts of cd-switch.zsh that decide what to mount and where the
# shell opens. No mount, no network: the mount program is a function here.
# Needs a terminal (the hooks act only on a terminal):
#
#     script -q /dev/null zsh -f -i scripts/dev/cd-switch-test.zsh
#
# Prints one line for each failure and `cd-switch-test: N passed, M failed`;
# ends with 1 when anything failed.

emulate -L zsh
typeset -i passed=0 failed=0
here=${0:A:h}

check() {
    # check <what> <expected> <got>
    if [[ $2 == "$3" ]]; then
        (( passed++ ))
    else
        (( failed++ ))
        print -r -- "FAIL $1: expected [$2], got [$3]"
    fi
}

# A place of the test's own; nothing is created in it.
ISTESSO_MOUNT_ROOT=/nonexistent/ish-test/Crucial\ X9/Istesso
typeset -gA ISTESSO_SHELL_FOR ISTESSO_MOUNT_FOR ISTESSO_MOUNT_HOST_PATH
ISTESSO_SHELL_FOR[ubuntu]=fake_shell
ISTESSO_MOUNT_FOR[ubuntu]=fake_mount
source $here/cd-switch.zsh
# The hooks are called by hand below: an interactive shell that reads this
# file would run them before each line of it, too.
istesso_cd_switch_off

target() {
    local REPLY=
    if _istesso_cd_target "$1"; then print -r -- "$REPLY"; else print -r -- "-"; fi
}

builtin cd -q /
r=$ISTESSO_MOUNT_ROOT
check "plain cd" "$r/ubuntu/proj" "$(target "cd $r:q/ubuntu/proj")"
check "quoted, with a space" "$r/ubuntu/proj/sub" "$(target "cd \"$r/ubuntu/proj/sub\"")"
check "single quotes" "$r/ubuntu/proj" "$(target "cd '$r/ubuntu/proj'")"
check "backslash space" "/a b" "$(target 'cd /a\ b')"
check "home" "$HOME/Istesso/ubuntu/proj" "$(target 'cd ~/Istesso/ubuntu/proj')"
check "home alone" "$HOME" "$(target 'cd ~')"
check "relative" "/x/y" "$(target 'cd x/y')"
check "dots are taken out" "/Istesso/ubuntu/q" "$(target 'cd /Istesso/ubuntu/p/../q/.')"
check "options" "/x" "$(target 'cd -q x')"
check "options together" "/x" "$(target 'cd -qP x')"
check "end of options" "/-x" "$(target 'cd -- -x')"
check "first command of a line" "/x" "$(target 'cd x && make')"
check "first command, then ;" "/x" "$(target 'cd x; ls')"
check "first command, then |" "/x" "$(target 'cd x | cat')"
check "a variable" "-" "$(target 'cd $HOME/x')"
check "a variable in quotes" "-" "$(target 'cd "$HOME/x"')"
check "a command" "-" "$(target 'cd $(pwd)')"
check "backquotes" "-" "$(target 'cd `pwd`')"
check "a pattern" "-" "$(target 'cd /x/*')"
check "another user's home" "-" "$(target 'cd ~root/x')"
check "an equals word" "-" "$(target 'cd =ls')"
check "cd alone" "-" "$(target 'cd')"
check "cd -" "-" "$(target 'cd -')"
check "two folders" "-" "$(target 'cd a b')"
check "another command" "-" "$(target 'ls x')"
check "pushd" "-" "$(target 'pushd x')"
check "the second command" "-" "$(target 'ls; cd x')"
check "a folder alone, no AUTO_CD" "-" "$(target 'x')"
setopt autocd
check "a folder alone, AUTO_CD" "/x" "$(target 'x')"
check "a folder and more, AUTO_CD" "-" "$(target 'x y')"
check "a folder and a command, AUTO_CD" "-" "$(target 'x; ls')"
unsetopt autocd

place() {
    local -a reply
    if _istesso_mount_place "$1"; then print -r -- "${(j:|:)reply}"; else print -r -- "-"; fi
}
check "a folder deep in a mount" "ubuntu|proj|src/main" "$(place "$r/ubuntu/proj/src/main")"
check "the mount itself" "ubuntu|proj|" "$(place "$r/ubuntu/proj")"
check "the machine's folder" "ubuntu||" "$(place "$r/ubuntu")"
check "the root" "-" "$(place "$r")"
check "beside the root" "-" "$(place "${r}X/ubuntu/proj")"
check "elsewhere" "-" "$(place /tmp/ubuntu/proj)"
check "no folder id: capitals" "-" "$(place "$r/ubuntu/Proj")"
check "no folder id: a dash first" "-" "$(place "$r/ubuntu/-p")"
check "no folder id: an underscore" "-" "$(place "$r/ubuntu/a_b")"
check "no folder id: too long" "-" "$(place "$r/ubuntu/${(l:33::a:)}")"
check "a folder id of 32" "ubuntu|${(l:32::a:)}|" "$(place "$r/ubuntu/${(l:32::a:)}")"

host_path() {
    local REPLY
    _istesso_mount_host_path "$@"
    print -r -- "$REPLY"
}
check "the same folder below the home there" "proj" "$(host_path ubuntu proj '')"
check "and below it" "proj/src/x" "$(host_path ubuntu proj src/x)"
ISTESSO_MOUNT_HOST_PATH[ubuntu/proj]='~/work/the-project'
check "a folder named for it, below the home" "work/the-project/src" "$(host_path ubuntu proj src)"
ISTESSO_MOUNT_HOST_PATH[ubuntu/proj]=/srv/data
check "a folder named for it, elsewhere" "/srv/data/a b" "$(host_path ubuntu proj 'a b')"
ISTESSO_MOUNT_HOST_PATH[ubuntu/proj]='~'
check "the home there" "--home" "$(host_path ubuntu proj '')"
check "below the home there" "sub" "$(host_path ubuntu proj sub)"
unset 'ISTESSO_MOUNT_HOST_PATH[ubuntu/proj]'

# The mount program, as a function: what it was asked, and its answer.
typeset -ga asked
typeset -g answer=0
fake_mount() {
    asked+=("$ISTESSO_MOUNT_ROOT|$*")
    return answer
}

asked=()
_istesso_cd_preexec "cd $r:q/ubuntu/proj/src"
check "mounted before the cd" "$r|proj" "${asked[1]-}"
check "remembered for taking it off" "1" "${_istesso_mount_used[ubuntu/proj]-}"
check "no failure" "" "$_istesso_mount_failed"
asked=()
_istesso_cd_preexec "cd /tmp"
check "nothing for a folder elsewhere" "0" "${#asked}"
_istesso_cd_preexec "cd $r:q/ubuntu"
check "nothing for the machine's folder" "0" "${#asked}"
_istesso_cd_preexec "cd $r:q/studio/proj"
check "nothing for a machine without a mount program" "0" "${#asked}"
answer=1
_istesso_cd_preexec "cd $r:q/ubuntu/other" 2>/dev/null
check "asked, and refused" "$r|other" "${asked[1]-}"
check "the failure is kept for the cd" "ubuntu/other" "$_istesso_mount_failed"
check "not remembered" "" "${_istesso_mount_used[ubuntu/other]-}"
_istesso_cd_preexec "ls"
check "the next line forgets it" "" "$_istesso_mount_failed"
answer=0

# Taking it off.
asked=()
builtin cd -q /
answer=3
_istesso_mount_release -q ubuntu proj
check "busy: status 3" "3" "$?"
check "busy: asked to release" "$r|--release proj" "${asked[1]-}"
check "busy: still remembered" "1" "${_istesso_mount_used[ubuntu/proj]-}"
answer=0
_istesso_mount_release -q ubuntu proj
check "released: status 0" "0" "$?"
check "released: forgotten" "" "${_istesso_mount_used[ubuntu/proj]-}"

# Never while this shell stands in it (the folder is made for this check only
# when it can be: the root above does not exist).
_istesso_mount_used[ubuntu/proj]=1
asked=()
PWD_SAVED=$PWD
PWD=$r/ubuntu/proj/src
_istesso_mount_release -q ubuntu proj
check "standing in it: status 3" "3" "$?"
check "standing in it: not asked" "0" "${#asked}"
PWD=$PWD_SAVED

# The prompts try again, but not more often than told.
ISTESSO_MOUNT_RETRY=30
_istesso_mount_tried=$EPOCHSECONDS
asked=()
_istesso_cd_precmd
check "not again so soon" "0" "${#asked}"
_istesso_mount_tried=0
_istesso_cd_precmd
check "again after a while" "$r|--release proj" "${asked[1]-}"
check "and then forgotten" "0" "${#_istesso_mount_used}"
asked=()
_istesso_mount_tried=0
_istesso_cd_precmd
check "nothing to try" "0" "${#asked}"

print -r -- "cd-switch-test: $passed passed, $failed failed"
(( failed == 0 ))
exit $?
