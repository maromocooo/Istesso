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
# Remove it from a shell with `istesso_cd_switch_off`.

[[ -o interactive ]] || return 0

typeset -g ISTESSO_MOUNT_ROOT=${ISTESSO_MOUNT_ROOT:-$HOME/Istesso}
typeset -gA ISTESSO_SHELL_FOR
(( ${+ISTESSO_SHELL_FOR[ubuntu]} )) || ISTESSO_SHELL_FOR[ubuntu]=ish

autoload -Uz add-zsh-hook

# What the command line was, and where this shell stood, before the command
# ran: the hook after a cd needs both.
_istesso_cd_preexec() {
    typeset -g _istesso_cd_line=$1
    typeset -g _istesso_cd_from=$PWD
    typeset -g _istesso_cd_oldpwd=$OLDPWD
    typeset -ga _istesso_cd_stack
    _istesso_cd_stack=("${dirstack[@]}")
}

# A command line is over: a cd that comes later without one (from a key
# binding, say) is not taken for it.
_istesso_cd_precmd() {
    typeset -g _istesso_cd_line=
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
    [[ $PWD == $root/* ]] || return 0
    local rest=${PWD#$root/}
    local machine=${rest%%/*}
    local folder=${rest#$machine}
    folder=${folder#/}
    local launcher=${ISTESSO_SHELL_FOR[$machine]}
    if [[ -z $launcher ]]; then
        print -u2 -r -- "istesso: no shell for '$machine' from this Mac; staying here"
        return 0
    fi

    if [[ -n $folder ]]; then
        ${=launcher} "$folder"
    else
        ${=launcher} --home
    fi
    local shell_status=$?

    # Back where this shell was, as if the cd had not happened.
    builtin cd -q -- "$_istesso_cd_from"
    OLDPWD=$_istesso_cd_oldpwd
    dirstack=("${_istesso_cd_stack[@]}")
    if (( shell_status == 255 )); then
        print -u2 -r -- "istesso: back on this Mac, in ${PWD/#$HOME/~}"
    fi
    return 0
}

istesso_cd_switch_off() {
    add-zsh-hook -d preexec _istesso_cd_preexec
    add-zsh-hook -d precmd _istesso_cd_precmd
    add-zsh-hook -d chpwd _istesso_cd_chpwd
}

add-zsh-hook preexec _istesso_cd_preexec
add-zsh-hook precmd _istesso_cd_precmd
add-zsh-hook chpwd _istesso_cd_chpwd
