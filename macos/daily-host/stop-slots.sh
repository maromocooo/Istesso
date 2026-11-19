#!/bin/bash
# Shared by the launcher and the local menu. A PID is valid only while its start time matches.
same_process() {
    local pid=$1 started=$2
    [[ "$pid" =~ ^[1-9][0-9]*$ && "$pid" != 1 && -n "$started" ]] || return 1
    local actual
    actual=$(LC_ALL=C ps -o lstart= -p "$pid" 2>/dev/null) || return 1
    [[ -n "$actual" && "$(echo $actual)" == "$(echo $started)" ]]
}
still_runs() {
    local pid started
    [[ -s "$1" ]] && read -r pid started < "$1" && same_process "$pid" "$started"
}
stop_written_down() {
    local pid started i
    [[ -s "$1" ]] && read -r pid started < "$1" && same_process "$pid" "$started" || return 0
    kill "$pid" 2>/dev/null || return 0
    for ((i=0; i<30; i++)); do same_process "$pid" "$started" || return 0; sleep 0.1; done
}
stop_slots() {
    # Slot by slot, the host program and then its agent: the launcher's stop as it was.
    local root run
    for root in "$@"; do
        for run in "$root"/*/; do
            [[ -d "$run" ]] || continue
            stop_written_down "$run/host.pid"
            stop_written_down "$run/agent.pid"
        done
    done
}

if [[ ${BASH_SOURCE[0]:-} == "$0" ]]; then
    stop_slots "$@"
fi
