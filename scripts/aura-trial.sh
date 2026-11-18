#!/bin/bash
# PROTOTYPE, for home use only (docs/m8-aura-prototype.md): one application of the Linux host,
# shown on AURA. A trial path of its own, apart from every build that is measured: the host
# script (scripts/m1-step3-host.py) is used as it is and only given other switches. AURA cannot
# decode 4:4:4, so this path sends 4:2:0 (--chroma 420), 72 frames a second (AURA's screen).
# Modelled on the host side of the everyday trial (scripts/daily.sh on branch trial-daily), which
# it does not change and does not share a folder, a port or a process with.
#
#   scripts/aura-trial.sh setup      copy this checkout to the host and build it there (cargo stops
#                                    when the measuring signal turns busy), and copy the compositor
#                                    plugin (read only from where it was built)
#   scripts/aura-trial.sh pair-line  the one command the owner types on the host, in a terminal of
#                                    their own, to pair AURA with the trial's own identity of the
#                                    host (the code is shown there only, never here)
#   scripts/aura-trial.sh open       start the text editor in a hidden compositor, its picture on
#                                    port 47914 (4:2:0, 72 fps, 1920x1080) and an agent for its input
#                                    on port 47924, and a guard
#   scripts/aura-trial.sh status     what runs
#   scripts/aura-trial.sh stop       end all of it, by the numbers written down when it was started
#
# On the host everything is in its own folder (AURA_TRIAL_DIR, ~/.cache/istesso-aura-trial):
# the copy of the source, the plugin, the run files, the application's own settings folders, and
# the trial's own identity of the host and its list of paired devices (config/). The host's
# everyday identity and list of paired devices are never read or written: AURA is paired with
# config/ only, and removing that folder undoes it.
#
# The measuring signal (AGENTS.md) is obeyed: nothing starts while it says busy; a guard on the
# host reads it every 2 s, and when it turns busy the picture is stopped (the encoder given back)
# and every process of the trial is held still; when it is free again, they go on. The signal is
# only read, never written.
#
# Settings: ISTESSO_HOST (ssh name of the host, default istesso-ubuntu), AURA_TRIAL_DIR,
# AURA_PLUGIN_FROM (the built compositor plugin, default the one the everyday trial copies from),
# AURA_SIZE (1920x1080), AURA_FPS (72), LAYOUT (us).
set -u

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo" || exit 1
host_name=${ISTESSO_HOST:-istesso-ubuntu}
host_dir=${AURA_TRIAL_DIR:-'~/.cache/istesso-aura-trial'}
plugin_from=${AURA_PLUGIN_FROM:-'~/.cache/istesso-compositor-wip/gwd-trial10/target/release'}
size=${AURA_SIZE:-1920x1080}
fps=${AURA_FPS:-72}
layout=${LAYOUT:-us}
picture_port=47914
agent_port=47924

refuse_when_busy() {
    "$repo/scripts/measuring.sh" status > /dev/null 2>&1 && return 0
    echo "a latency measurement is running (scripts/measuring.sh status): nothing is started"
    exit 3
}

# What the commands on the host have in common, sent with each of them.
IFS= read -r -d '' host_lib <<'LIB'
D=$(eval echo "$TRIAL_DIR"); src=$D/src; plugin=$D/plugin; run=$D/run/editor; config=$D/config
signal_busy() { [ "$(cut -d' ' -f1 "$HOME/.local/state/istesso/measuring" 2>/dev/null)" = busy ]; }
still_runs() { # FILE: the process written down there is still the one that was started
    [ -s "$1" ] || return 1
    local pid started
    read -r pid started < "$1"
    [ -n "$pid" ] && [ -n "$started" ] && [ "$(echo $(ps -o lstart= -p "$pid" 2>/dev/null))" = "$(echo $started)" ]
}
number_in() { local pid _; read -r pid _ < "$1"; echo "$pid"; }
members() { # PART: every process of the session that part was started in (its own tmux session)
    [ -s "$run/$1.sid" ] || return 0
    local sid started now
    read -r sid started < "$run/$1.sid"
    [ -n "$sid" ] && [ -n "$started" ] || return 0
    now=$(echo $(ps -o lstart= -p "$sid" 2>/dev/null))
    [ -z "$now" ] || [ "$now" = "$(echo $started)" ] || return 0
    ps -o pid= -s "$sid" 2>/dev/null | tr -d ' '
}
to_hold() { # PART: the members without the session's first process (tmux's shell, which only waits)
    local sid _ pid
    read -r sid _ < "$run/$1.sid" 2>/dev/null
    for pid in $(members "$1"); do [ "$pid" = "$sid" ] || echo "$pid"; done
}
said() { sed -E 's/\x1b\[[0-9;]*m//g' "$run/pipeline.log" 2>/dev/null; }
count_said() { said | grep -c "\"istesso\":\"$1\""; }
tell() { echo "$1" > "$run/control"; }
hold() { # the picture stopped and its encoder given back, then everything held still
    [ -s "$run/held" ] && return 0
    local before; before=$(count_said paused)
    tell detach
    for i in $(seq 1 30); do [ "$(count_said paused)" -gt "$before" ] && break; sleep 0.1; done
    echo "$(date +%s) busy" > "$run/held"
    local part pid
    for part in app agent pipeline; do for pid in $(to_hold "$part"); do kill -STOP "$pid" 2>/dev/null; done; done
    echo "$(date '+%H:%M:%S') held: $([ "$(count_said paused)" -gt "$before" ] && echo "picture stopped, encoder given back" || echo "the picture did NOT say that it stopped")"
}
let_go() {
    [ -s "$run/held" ] || return 0
    local part pid
    for part in pipeline agent app; do for pid in $(members "$part"); do kill -CONT "$pid" 2>/dev/null; done; done
    : > "$run/held"
    tell resume
    echo "$(date '+%H:%M:%S') let go on"
}
end_part() { # PART: asked to end by number; what is still there two seconds later is ended
    local pid i
    for pid in $(members "$1"); do kill -CONT "$pid" 2>/dev/null; kill -TERM "$pid" 2>/dev/null; done
    for i in $(seq 1 20); do [ -z "$(members "$1")" ] && return 0; sleep 0.1; done
    for pid in $(members "$1"); do kill -KILL "$pid" 2>/dev/null; done
}
LIB

on_host() { # the commands come on standard input
    ssh -o BatchMode=yes -o ConnectTimeout=10 "$host_name" \
        "export LC_ALL=C TRIAL_DIR=$host_dir PLUGIN_FROM=$plugin_from SIZE=$size FPS=$fps LAYOUT=$layout PICTURE_PORT=$picture_port AGENT_PORT=$agent_port; bash -s" < <(printf '%s\n' "$host_lib"; cat)
}

do_setup() {
    refuse_when_busy
    echo "the host: copying this checkout's files"
    git ls-files -z | COPYFILE_DISABLE=1 tar --null -T - -czf - |
        ssh -o BatchMode=yes -o ConnectTimeout=10 "$host_name" \
            "LC_ALL=C bash -c 'D=\$(eval echo $host_dir); mkdir -p \$D/src \$D/run && chmod 700 \$D && tar -xzf - -C \$D/src 2> \$D/run/copy.log; s=\$?; grep -v \"extended header\" \$D/run/copy.log; exit \$s'" ||
        { echo "error: the copy did not reach the host"; exit 1; }
    on_host <<'HOST'
signal_busy && { echo "host: a latency measurement is running: nothing is started"; exit 3; }
tmux has-session -t '=aura-build' 2>/dev/null && { echo "host: a build of the AURA trial runs already"; exit 1; }
cat > "$D/run/build.sh" <<'SCRIPT'
#!/bin/bash
# Builds the trial's copy; stops cargo within seconds when the measuring signal turns busy.
export PATH=$HOME/.cargo/bin:$PATH LC_ALL=C
D=$1
busy() { [ "$(cut -d' ' -f1 "$HOME/.local/state/istesso/measuring" 2>/dev/null)" = busy ]; }
busy && { echo "build: not started, a measurement is running"; exit 3; }
(cd "$D/src" && exec setsid -w cargo build --release -p istesso-cli -p istesso-media) &
pid=$!
while kill -0 "$pid" 2>/dev/null; do
    if busy; then echo "build: a measurement began, cargo was stopped"; kill -INT -- "-$pid" 2>/dev/null; wait "$pid"; exit 3; fi
    sleep 2
done
wait "$pid"; status=$?
echo "build: exit $status"
exit $status
SCRIPT
chmod +x "$D/run/build.sh"; : > "$D/run/build.log"
tmux new-session -d -s aura-build "bash -c '$D/run/build.sh $D > $D/run/build.log 2>&1'"
for i in $(seq 1 900); do
    tmux has-session -t '=aura-build' 2>/dev/null || break
    sleep 1
done
tail -1 "$D/run/build.log"
grep -q "build: exit 0" "$D/run/build.log" || exit 1
# The compositor plugin: a copy, so that a later build of the plugin elsewhere leaves this as it is.
from=$(eval echo "$PLUGIN_FROM")
[ -e "$from/libgstwaylanddisplaysrc.so" ] && [ -e "$from/libwaylanddisplaycore.so" ] ||
    { echo "host: no built compositor plugin in $PLUGIN_FROM (AURA_PLUGIN_FROM)"; exit 1; }
mkdir -p "$plugin"
for file in libgstwaylanddisplaysrc.so libwaylanddisplaycore.so; do
    cp "$from/$file" "$plugin/$file.new" && mv -f "$plugin/$file.new" "$plugin/$file"
done
chmod a-w "$plugin"/*
mkdir -p "$config"; chmod 700 "$config"
echo "host: built, and the plugin copied"
HOST
}

do_pair_line() {
    # Printed for the owner to type on the host; nothing is run. The code then appears in the
    # owner's terminal only.
    echo "ISTESSO_CONFIG_DIR=$host_dir/config $host_dir/src/target/release/istesso pair host --name host-trial"
}

do_open() {
    refuse_when_busy
    on_host <<'HOST'
signal_busy && { echo "host: a latency measurement is running: nothing is started"; exit 3; }
[ -e "$plugin/libgstwaylanddisplaysrc.so" ] || { echo "host: the compositor plugin is not in $plugin (scripts/aura-trial.sh setup)"; exit 1; }
[ -x "$src/target/release/istesso" ] && [ -e "$src/target/release/libistesso_media.so" ] || { echo "host: the trial's copy is not built (scripts/aura-trial.sh setup)"; exit 1; }
[ -s "$config/peers.json" ] || echo "host: note: nothing is paired with the trial's identity yet (scripts/aura-trial.sh pair-line)"
if still_runs "$run/pipeline.pid"; then echo "host: the AURA trial runs already"; exit 0; fi
if ss -Hltn "( sport = :$PICTURE_PORT or sport = :$AGENT_PORT )" 2>/dev/null | grep -q .; then
    echo "fail port $PICTURE_PORT or $AGENT_PORT is taken on the host by something else"; exit 5
fi
mkdir -p "$run/rt/istesso" "$run/agent-cache"; chmod 700 "$D/run" "$run" "$run/rt" "$run/rt/istesso"
: > "$run/held"
[ -p "$run/control" ] || mkfifo "$run/control"
node=""
for n in /sys/class/drm/renderD*; do
    [ "$(basename "$(readlink -f "$n/device/driver")")" = nvidia ] && node=/dev/dri/$(basename "$n") && break
done
# The picture: the host script as it is, with this trial's switches. 4:2:0 for AURA's decoder,
# 72 frames a second for its screen. The pointer is drawn into the picture (no --cursor).
cat > "$run/pipeline.sh" <<SCRIPT
#!/bin/bash
export GST_PLUGIN_PATH=$plugin GST_REGISTRY=$run/gst-registry.bin XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR
export XKB_DEFAULT_LAYOUT=$LAYOUT
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $run/pipeline.pid
sid=\$(ps -o sid= -p \$\$ | tr -d ' '); echo "\$sid \$(ps -o lstart= -p \$sid)" > $run/pipeline.sid
exec 3<>$run/control
exec python3 -u $src/scripts/m1-step3-host.py --to-channel $PICTURE_PORT --chroma 420 --fps $FPS --size $SIZE --admit paired --input-socket $run/rt/istesso/input.sock --frames cuda ${node:+--render-node $node} --media-lib $src/target/release/libistesso_media.so --config-dir $config --log - <&3 3<&-
SCRIPT
chmod +x "$run/pipeline.sh"; : > "$run/pipeline.log"; : > "$run/pipeline.exit"
tmux new-session -d -s aura-trial-pipe "bash -c '$run/pipeline.sh > $run/pipeline.log 2>&1; echo \$? > $run/pipeline.exit'"
sock=""
for i in $(seq 1 120); do
    sock=$(said | grep -o 'socket_name="[^"]*"' | head -1 | cut -d'"' -f2)
    [ -n "$sock" ] && break
    [ -s "$run/pipeline.exit" ] && break
    sleep 0.25
done
[ -n "$sock" ] || { echo "fail the compositor did not start:"; said | grep -v "DMA formats" | tail -4 | cut -c1-200; end_part pipeline; exit 1; }
# The application: the text editor, Wayland only, with settings folders of its own.
home=$D/apps/editor; mkdir -p "$home/data" "$home/config" "$home/state" "$home/cache" "$D/documents"
[ -e "$D/documents/aura.txt" ] || : > "$D/documents/aura.txt"
cat > "$run/app.sh" <<SCRIPT
#!/bin/bash
unset DISPLAY DBUS_SESSION_BUS_ADDRESS
export WAYLAND_DISPLAY=$sock XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR GDK_BACKEND=wayland XDG_SESSION_TYPE=wayland GTK_USE_PORTAL=0 NO_AT_BRIDGE=1
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $run/app.pid
sid=\$(ps -o sid= -p \$\$ | tr -d ' '); echo "\$sid \$(ps -o lstart= -p \$sid)" > $run/app.sid
exec env XDG_DATA_HOME=$home/data XDG_CONFIG_HOME=$home/config XDG_STATE_HOME=$home/state XDG_CACHE_HOME=$home/cache GSETTINGS_BACKEND=keyfile GIO_USE_VFS=local dbus-run-session -- gnome-text-editor --standalone --ignore-session $D/documents/aura.txt
SCRIPT
chmod +x "$run/app.sh"; : > "$run/app.exit"
tmux new-session -d -s aura-trial-app "bash -c '$run/app.sh > $run/app.log 2>&1; echo \$? > $run/app.exit'"
# The agent of this trial: its own port, no announcing, the trial's identity, this compositor's input.
cat > "$run/agent.sh" <<SCRIPT
#!/bin/bash
export XDG_RUNTIME_DIR=$run/rt ISTESSO_CACHE_DIR=$run/agent-cache ISTESSO_CONFIG_DIR=$config
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $run/agent.pid
sid=\$(ps -o sid= -p \$\$ | tr -d ' '); echo "\$sid \$(ps -o lstart= -p \$sid)" > $run/agent.sid
exec $src/target/release/istesso agent --port $AGENT_PORT --no-discovery
SCRIPT
chmod +x "$run/agent.sh"; : > "$run/agent.exit"
tmux new-session -d -s aura-trial-agent "bash -c '$run/agent.sh > $run/agent.log 2>&1; echo \$? > $run/agent.exit'"
# The guard: every 2 s the signal; it ends when the picture's pipeline does.
cat > "$run/guard.sh" <<SCRIPT
#!/bin/bash
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $run/guard.pid
D=$D src=$src plugin=$plugin run=$run config=$config
$(declare -f signal_busy still_runs number_in members to_hold said count_said tell hold let_go)
while sleep 2; do
    still_runs $run/pipeline.pid || { echo "\$(date '+%H:%M:%S') the pipeline ended: the guard ends"; exit 0; }
    if signal_busy; then hold; else let_go; fi
done
SCRIPT
chmod +x "$run/guard.sh"
tmux new-session -d -s aura-trial-guard "bash -c '$run/guard.sh >> $run/guard.log 2>&1'"
sleep 2
echo "host: started (picture on $PICTURE_PORT, 4:2:0, $FPS fps, $SIZE; input on $AGENT_PORT)"
echo "host: application $([ -n "$(members app)" ] && echo runs || echo "did NOT stay: $(head -2 "$run/app.log" | cut -c1-160)")"
HOST
}

do_status() {
    on_host <<'HOST'
for part in pipeline app agent guard; do
    if still_runs "$run/$part.pid"; then echo "$part: runs"; else echo "$part: not running"; fi
done
[ -s "$run/held" ] && echo "held still (a measurement)"
echo "paired with the trial's identity: $([ -s "$config/peers.json" ] && echo yes || echo no)"
HOST
}

do_stop() {
    on_host <<'HOST'
let_go > /dev/null
still_runs "$run/pipeline.pid" && kill -INT "$(number_in "$run/pipeline.pid")" 2>/dev/null
for i in $(seq 1 40); do still_runs "$run/pipeline.pid" || break; sleep 0.1; done
for part in pipeline app agent; do end_part "$part"; done
still_runs "$run/guard.pid" && kill -TERM "$(number_in "$run/guard.pid")" 2>/dev/null
echo "host: the AURA trial is stopped"
HOST
}

case "${1:-status}" in
setup) do_setup ;;
pair-line) do_pair_line ;;
open) do_open ;;
status) do_status ;;
stop) do_stop ;;
*) echo "usage: scripts/aura-trial.sh setup|pair-line|open|status|stop" >&2; exit 2 ;;
esac
