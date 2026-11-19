# cd-switch.zsh: a prototype for "cd into another machine's folder opens a
# shell there, like a new tab" (ADR 0007, proposal). Opt-in: nothing loads it
# by itself. In an interactive zsh, type
#
#     source ~/.local/share/istesso-dev/cd-switch.zsh
#
# From then on, in that shell only: a `cd` typed at the prompt into a folder
# below the mount place of another machine, $ISTESSO_MOUNT_ROOT/<machine>/<path>
# (~/Istesso/<machine>/<path> unless set), makes this terminal a shell on that
# machine, in ~/<path> there. `exit` there brings this shell back to where it
# was before the cd, as if it had not moved (`cd -` and the directory stack
# too). Nothing else switches: cd in scripts, functions, sourced files,
# subshells and command substitutions, pushd and popd, `cd -q`, and a cd
# whose output is redirected.
#
# Which program opens a shell on which machine: ISTESSO_SHELL_FOR[<machine>]
# (ubuntu: `ish`). A machine with no entry stays local, with a one-line note.
#
# Mounts (ADR 0007, proposal "ish とマウント"): a machine with a mount
# program, ISTESSO_MOUNT_FOR[<machine>] (ubuntu: `ish-mount`, when it is
# installed), shows its folders read-only at $ISTESSO_MOUNT_ROOT/<machine>/
# <folder-id>. A cd typed at the prompt into one (the first command of the
# line, with one folder) mounts it before the cd runs (`<program>
# <folder-id>`), and the cd goes nowhere when it cannot be mounted. The shell
# on that machine opens in the folder the mount shows:
# ISTESSO_MOUNT_HOST_PATH[<machine>/<folder-id>], or ~/<folder-id> there. When
# that shell ends, and at this shell's prompts after that (every
# $ISTESSO_MOUNT_RETRY seconds at most, 30 unless set), the mount is taken off
# unless something on this Mac still uses it (`<program> --release
# <folder-id>`, a plain unmount, which the system refuses while any program
# has a file or a folder in it open).
#
# Remove it from a shell with `istesso_cd_switch_off`.

[[ -o interactive ]] || return 0

zmodload -F zsh/datetime p:EPOCHSECONDS

typeset -g ISTESSO_MOUNT_ROOT=${ISTESSO_MOUNT_ROOT:-$HOME/Istesso}
typeset -gA ISTESSO_SHELL_FOR
(( ${+ISTESSO_SHELL_FOR[ubuntu]} )) || ISTESSO_SHELL_FOR[ubuntu]=ish
typeset -gA ISTESSO_MOUNT_FOR ISTESSO_MOUNT_HOST_PATH
if (( ! ${+ISTESSO_MOUNT_FOR[ubuntu]} && ${+commands[ish-mount]} )); then
    ISTESSO_MOUNT_FOR[ubuntu]=ish-mount
fi
typeset -g ISTESSO_MOUNT_RETRY=${ISTESSO_MOUNT_RETRY:-30}
# The mounts this shell went into, as <machine>/<folder-id>: taken off when
# nothing uses them any more.
typeset -gA _istesso_mount_used
typeset -g _istesso_mount_tried=0
# The mount that could not be made for the command line that runs now.
typeset -g _istesso_mount_failed=

autoload -Uz add-zsh-hook

# The folder that the first command of a typed line goes into, in REPLY, as
# an absolute path: `cd <folder>` (options -q -s -L -P allowed), or a folder
# name alone with AUTO_CD. Nothing (status 1) for anything else, and for a
# folder the shell would still expand (variables, commands, patterns,
# `~user`), which is left alone.
_istesso_cd_target() {
    local -a words first
    words=(${(z)1})
    local word
    for word in $words; do
        [[ $word == (\;|\&\&|\|\||\||\&|\|\&|\&\||\&!) ]] && break
        first+=($word)
    done
    local arg
    if [[ $first[1] == cd ]]; then
        shift first
        while [[ $first[1] =~ '^-[qsLP]+$' ]]; do shift first; done
        [[ $first[1] == -- ]] && shift first
        (( ${#first} == 1 )) || return 1
        arg=$first[1]
    elif (( ${#first} == 1 && ${#words} == 1 )) && [[ -o autocd ]]; then
        arg=$first[1]
    else
        return 1
    fi
    arg=${(Q)arg}
    [[ -n $arg && $arg != - ]] || return 1
    [[ $arg == *[\$\`\*\?\[\]\{\}\<\>\(\)\|\;\&\\\!\#\^]* || $arg == '='* ]] && return 1
    case $arg in
        '~') arg=$HOME ;;
        '~/'*) arg=$HOME/${arg#'~/'} ;;
        '~'*) return 1 ;;
        /*) ;;
        *) arg=$PWD/$arg ;;
    esac
    REPLY=${arg:a}
}

# Where a folder below the mount root is, in reply: (<machine> <folder-id>
# <rest>), with an empty folder id at the machine's own folder. Status 1 for
# a folder that is not below the root, or whose second part is no folder id.
_istesso_mount_place() {
    local root rest machine id sub
    for root in ${ISTESSO_MOUNT_ROOT:a} ${ISTESSO_MOUNT_ROOT:A}; do
        [[ $1 == "$root"/* ]] || continue
        rest=${1#"$root"/}
        machine=${rest%%/*}
        [[ -n $machine ]] || return 1
        id=
        sub=
        if [[ $rest == */* ]]; then
            rest=${rest#*/}
            id=${rest%%/*}
            [[ $rest == */* ]] && sub=${rest#*/}
            [[ -z $id || $id =~ '^[a-z0-9][a-z0-9-]{0,31}$' ]] || return 1
        fi
        reply=("$machine" "$id" "$sub")
        return 0
    done
    return 1
}

# What to give the shell program for <machine> <folder-id> <rest>, in REPLY:
# the folder there that the mount shows, and the rest below it. A path below
# the home there is given as it is below the home (`ish` reads it so).
_istesso_mount_host_path() {
    local there=${ISTESSO_MOUNT_HOST_PATH[$1/$2]:-"~/$2"}
    [[ -n $3 ]] && there=$there/$3
    case $there in
        '~') REPLY=--home ;;
        '~/'*) REPLY=${there#'~/'} ;;
        *) REPLY=$there ;;
    esac
}

# Mounts the folder the typed line goes into, when it is one of a machine
# with a mount program and it is not mounted yet.
_istesso_mount_before() {
    (( ZSH_SUBSHELL == 0 )) || return 0
    [[ -t 0 && -t 1 ]] || return 0
    local REPLY
    local -a reply
    _istesso_cd_target "$1" || return 0
    _istesso_mount_place "$REPLY" || return 0
    local machine=$reply[1] id=$reply[2]
    [[ -n $id ]] || return 0
    local mounter=${ISTESSO_MOUNT_FOR[$machine]}
    [[ -n $mounter ]] || return 0
    if ! ISTESSO_MOUNT_ROOT=${ISTESSO_MOUNT_ROOT:a} ${=mounter} "$id" >/dev/null; then
        _istesso_mount_failed=$machine/$id
        print -u2 -r -- "istesso: $machine/$id is not shown on this Mac; the cd goes nowhere"
        return 0
    fi
    _istesso_mount_used[$machine/$id]=1
}

# Takes off the mount of <machine> <folder-id> unless something uses it.
# Status 0 when it is not mounted any more, 3 while it is used (by this shell,
# too, when it stands in it). With -q, says nothing.
_istesso_mount_release() {
    local quiet=
    [[ $1 == -q ]] && { quiet=1; shift }
    local mounter=${ISTESSO_MOUNT_FOR[$1]}
    if [[ -z $mounter ]]; then
        unset "_istesso_mount_used[$1/$2]"
        return 0
    fi
    local root
    for root in ${ISTESSO_MOUNT_ROOT:a} ${ISTESSO_MOUNT_ROOT:A}; do
        [[ $PWD == "$root/$1/$2" || $PWD == "$root/$1/$2"/* ]] && return 3
    done
    local released
    if [[ -n $quiet ]]; then
        ISTESSO_MOUNT_ROOT=${ISTESSO_MOUNT_ROOT:a} ${=mounter} --release "$2" 2>/dev/null
    else
        ISTESSO_MOUNT_ROOT=${ISTESSO_MOUNT_ROOT:a} ${=mounter} --release "$2"
    fi
    released=$?
    (( released == 0 )) && unset "_istesso_mount_used[$1/$2]"
    return $released
}

# Tries the mounts this shell went into, every ISTESSO_MOUNT_RETRY seconds at
# most.
_istesso_mount_retry() {
    (( ${#_istesso_mount_used} )) || return 0
    (( EPOCHSECONDS - _istesso_mount_tried >= ISTESSO_MOUNT_RETRY )) || return 0
    _istesso_mount_tried=$EPOCHSECONDS
    local key
    for key in ${(k)_istesso_mount_used}; do
        _istesso_mount_release -q "${key%%/*}" "${key#*/}"
    done
    return 0
}

# What the command line was, and where this shell stood, before the command
# ran: the hook after a cd needs both. A folder the line goes into is
# mounted here, before the cd looks for it.
_istesso_cd_preexec() {
    typeset -g _istesso_cd_line=$1
    typeset -g _istesso_cd_from=$PWD
    typeset -g _istesso_cd_oldpwd=$OLDPWD
    typeset -ga _istesso_cd_stack
    _istesso_cd_stack=("${dirstack[@]}")
    _istesso_mount_failed=
    _istesso_mount_before "$1"
}

# A command line is over: a cd that comes later without one (from a key
# binding, say) is not taken for it. Mounts nothing uses any more go.
_istesso_cd_precmd() {
    typeset -g _istesso_cd_line=
    _istesso_mount_retry
}

# Back where this shell was before the cd, as if it had not happened.
_istesso_cd_back() {
    builtin cd -q -- "$_istesso_cd_from"
    OLDPWD=$_istesso_cd_oldpwd
    dirstack=("${_istesso_cd_stack[@]}")
}

_istesso_cd_chpwd() {
    # Only a cd typed at the prompt, in this shell itself, to the terminal.
    (( ZSH_SUBSHELL == 0 )) || return 0
    [[ ${(j.:.)zsh_eval_context} == toplevel:shfunc ]] || return 0
    [[ -n $_istesso_cd_line && -t 0 && -t 1 ]] || return 0
    local -a words
    words=(${(z)_istesso_cd_line})
    if [[ $words[1] != cd ]]; then
        # A folder name alone, with AUTO_CD, is a cd too; pushd and popd are not.
        [[ ${#words} == 1 && -o autocd ]] || return 0
    fi
    # Once for each command line.
    _istesso_cd_line=

    local root=${ISTESSO_MOUNT_ROOT:A}
    [[ $PWD == "$root"/* ]] || root=${ISTESSO_MOUNT_ROOT:a}
    [[ $PWD == "$root"/* ]] || return 0
    local rest=${PWD#"$root"/}
    local machine=${rest%%/*}
    local folder=${rest#"$machine"}
    folder=${folder#/}
    local launcher=${ISTESSO_SHELL_FOR[$machine]}
    if [[ -z $launcher ]]; then
        print -u2 -r -- "istesso: no shell for '$machine' from this Mac; staying here"
        return 0
    fi

    local mounter=${ISTESSO_MOUNT_FOR[$machine]} id= REPLY
    local -a reply
    if [[ -n $mounter && -n $folder ]] && _istesso_mount_place "$root/$machine/$folder"; then
        id=$reply[2]
        if [[ $_istesso_mount_failed == "$machine/$id" ]]; then
            # An empty folder where the mount would be: not the machine's folder.
            _istesso_cd_back
            return 0
        fi
        _istesso_mount_host_path "$machine" "$id" "$reply[3]"
        folder=$REPLY
    fi

    if [[ $folder == --home || -z $folder ]]; then
        ${=launcher} --home
    else
        ${=launcher} "$folder"
    fi
    local shell_status=$?

    _istesso_cd_back
    if (( shell_status == 255 )); then
        print -u2 -r -- "istesso: back on this Mac, in ${PWD/#$HOME/~}"
    fi
    if [[ -n $id ]]; then
        # The shell there is over: the mount goes unless something here uses it.
        _istesso_mount_release "$machine" "$id"
        _istesso_mount_tried=$EPOCHSECONDS
    fi
    return 0
}

# Before this shell ends: mounts nothing uses any more go.
_istesso_mount_zshexit() {
    local key
    for key in ${(k)_istesso_mount_used}; do
        _istesso_mount_release -q "${key%%/*}" "${key#*/}"
    done
}

istesso_cd_switch_off() {
    add-zsh-hook -d preexec _istesso_cd_preexec
    add-zsh-hook -d precmd _istesso_cd_precmd
    add-zsh-hook -d chpwd _istesso_cd_chpwd
    add-zsh-hook -d zshexit _istesso_mount_zshexit
}

add-zsh-hook preexec _istesso_cd_preexec
add-zsh-hook precmd _istesso_cd_precmd
add-zsh-hook chpwd _istesso_cd_chpwd
add-zsh-hook zshexit _istesso_mount_zshexit
