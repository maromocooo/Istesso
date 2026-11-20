#!/usr/bin/env bash
# Demonstrates the istesso-scope prototype on a Linux host (docs/research/agent-scope-proto.md).
#
#   scripts/agent-scope-demo.sh <istesso-scope binary> <work folder> [timing] [no-escapes]
#
# Everything is created inside a new run folder under <work folder>. Nothing outside it is
# written, except: the same-user escape checks send ONE read-only request to the Docker daemon
# (GET /_ping) and start ONE transient user service that reads a file of the run folder.
# "no-escapes" leaves those two out. Ports used: 26880 to 26886. Servers end by themselves.
# Respects the measuring signal (AGENTS.md): stops when it reads "busy".

set -u
export LC_ALL=C

BIN_IN=${1:?istesso-scope binary}
WORK=${2:?work folder}
TIMING=no
ESCAPES=yes
for a in "${@:3}"; do
	case $a in
	timing) TIMING=timing ;;
	no-escapes) ESCAPES=no ;;
	esac
done

signal_file="$HOME/.local/state/istesso/measuring"
guard() {
	local word=free
	[ -f "$signal_file" ] && word=$(cut -d' ' -f1 "$signal_file")
	if [ "$word" != free ]; then
		echo "measuring signal is $word: stopping"
		exit 3
	fi
}
guard

RUN="$WORK/demo-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$RUN/bin" "$RUN/project" "$RUN/state" "$RUN/outside" "$RUN/relay-root"
cp "$BIN_IN" "$RUN/bin/istesso-scope"
BIN="$RUN/bin/istesso-scope"
echo "outside-marker" >"$RUN/outside/secret.txt"
echo "run folder: (work folder)/$(basename "$RUN")"

pids=()
cleanup() {
	for p in "${pids[@]}"; do kill "$p" 2>/dev/null; done
}
trap cleanup EXIT

# Stand-ins: the relay (an HTTP server that answers 404 to proxy requests), a port that is not the
# relay, and an abstract UNIX socket outside the scope.
timeout 600 python3 -m http.server 26880 --bind 127.0.0.1 --directory "$RUN/relay-root" >"$RUN/relay.log" 2>&1 &
pids+=($!)
"$BIN" probe serve-tcp 26881 300 not-the-relay &
pids+=($!)
"$BIN" probe serve-abstract istesso-scope-demo 300 outside-abstract &
pids+=($!)
sleep 1

scope() { # scope <level> [launcher options] -- command...
	local level=$1
	shift
	"$BIN" run --level "$level" --project "$RUN/project" --state "$RUN/state" \
		--relay-port 26880 --read "$RUN/bin" --quiet "$@"
}
step() {
	guard
	echo
	echo "== $*"
}
probe() { # probe <level> <probe args...>
	local level=$1
	shift
	scope "$level" -- "$BIN" probe "$@" 2>/dev/null
}

step "what the launcher says when it starts (level 2)"
"$BIN" run --level 2 --project "$RUN/project" --state "$RUN/state" --relay-port 26880 -- /bin/true 2>&1 |
	sed "s#$HOME#~#g"

step "level 1: files"
probe 1 read "$RUN/outside/secret.txt"
probe 1 read "$HOME/.profile"
probe 1 write "$RUN/outside/new.txt"
probe 1 write "/tmp/istesso-scope-demo-$$"
probe 1 write "$RUN/project/written.txt"
scope 1 -- /bin/ls "$HOME" >/dev/null 2>&1 && echo "list home: ok" || echo "list home: refused"

step "level 1: network"
probe 1 connect 127.0.0.1:26880
probe 1 connect 127.0.0.1:26881
probe 1 connect 127.0.0.1:22
probe 1 connect 192.0.2.1:443
probe 1 connect 192.0.2.1:26880
probe 1 bind 26882
probe 1 udp 127.0.0.1:26881
scope 1 -- /usr/bin/curl -s -o /dev/null -w 'curl through the relay: HTTP %{http_code}\n' http://example.invalid/ 2>/dev/null
scope 1 -- /usr/bin/curl -sS --noproxy '*' -m 3 -o /dev/null https://192.0.2.1/ 2>"$RUN/curl.err"
echo "curl around the relay: exit $? ($(tail -1 "$RUN/curl.err"))"

step "level 1: a development server inside, reached from inside and outside"
# The server ends by itself; the launcher ends when it does.
scope 1 -- /usr/bin/timeout 20 /usr/bin/python3 -m http.server 26883 --bind 127.0.0.1 --directory "$RUN/project" >/dev/null 2>&1 &
sleep 1
scope 1 -- /usr/bin/curl -sS -o /dev/null --noproxy '*' http://127.0.0.1:26883/ 2>"$RUN/curl.err"
echo "from inside, port not allowed: exit $? ($(tail -1 "$RUN/curl.err"))"
scope 1 --connect-port 26883 -- /usr/bin/curl -s -o /dev/null -w 'from inside, --connect-port 26883: HTTP %{http_code}\n' --noproxy '*' http://127.0.0.1:26883/ 2>/dev/null
/usr/bin/curl -s -o /dev/null -w 'from outside (as a port forward would): HTTP %{http_code}\n' --noproxy '*' http://127.0.0.1:26883/

step "level 1: listening stays on loopback (bind and listen go through the supervisor)"
mkdir -p "$RUN/project/public"
probe 1 bind-any 26885
probe 1 bind-any 26885 --v6
scope 1 --listen-anywhere -- "$BIN" probe bind-any 26885 2>/dev/null | sed "s/^/--listen-anywhere: /"
# Only when the move to loopback works: a server started without --bind, serving an empty folder.
if probe 1 bind-any 26886 | grep -q 'bound to 127.0.0.1'; then
	scope 1 -- /usr/bin/timeout 15 /usr/bin/python3 -m http.server 26886 --directory "$RUN/project/public" >/dev/null 2>"$RUN/listen.err" &
	sleep 2
	echo "listening sockets on port 26886: $(ss -ltnH 'sport = :26886' | awk '{print $4}' | tr '\n' ' ')"
	/usr/bin/curl -s -o /dev/null -w 'from outside on loopback: HTTP %{http_code}\n' --noproxy '*' 'http://127.0.0.1:26886/'
	grep -h 'loopback only' "$RUN/listen.err"
fi

step "level 1: scopes (signals, abstract sockets, other processes' environment)"
probe 1 signal $$
probe 1 environ $$
probe 1 abstract istesso-scope-demo

step "level 1: tools"
scope 1 -- /bin/sh -c 'cd "$ISTESSO_SCOPE_PROJECT" && cargo init --vcs none --name scopedemo -q && cargo build -q && ./target/debug/scopedemo' 2>&1 | tail -3
scope 1 -- /usr/bin/python3 -c 'import json, sqlite3, ssl; print("python3:", json.dumps({"ok": True}))' 2>&1 | tail -2
scope 1 -- /usr/bin/node -e 'console.log("node:", process.version.length > 0 ? "ok" : "no")' 2>&1 | tail -2
scope 1 -- /bin/sh -c 'cd "$ISTESSO_SCOPE_PROJECT" && git init -q && git add -A && git -c user.name=scope -c user.email=scope commit -qm demo && echo "git: $(git log --oneline | wc -l) commit"' 2>&1 | tail -2
scope 1 -- /usr/bin/nvidia-smi --query-gpu=count --format=csv,noheader 2>&1 | head -1 | sed 's/^/nvidia-smi at level 1: /'

step "level 2: devices"
scope 2 -- /usr/bin/nvidia-smi --query-gpu=count --format=csv,noheader 2>&1 | head -1 | sed 's/^/nvidia-smi at level 2, GPUs: /'
scope 2 --connect-port 26884 -- /bin/sh -c 'adb -P 26884 start-server >/dev/null 2>&1; adb -P 26884 devices | head -1; adb -P 26884 kill-server' 2>&1 | sed 's/^/adb at level 2: /'

if [ "$ESCAPES" = yes ]; then
	step "same-user ways out, still open (one read-only request each)"
	scope 2 -- /usr/bin/curl -s --unix-socket /var/run/docker.sock http://localhost/_ping 2>&1 | sed 's/^/docker socket from inside: /'
	echo
	scope 2 -- /bin/sh -c "XDG_RUNTIME_DIR=/run/user/$(id -u) systemd-run --user --pipe --quiet --unit=istesso-scope-demo-$$ /bin/cat '$RUN/outside/secret.txt'" 2>&1 | sed 's/^/systemd-run --user from inside reads the outside file: /'
fi

step "hold and ask (seccomp user notification, stand-in answer)"
scope 1 --hold-files --ask-cmd 'sleep 1; exit 0' -- /bin/cat "$RUN/outside/secret.txt" 2>&1 | grep -v '^istesso-scope: \(level\|enforced\|NOT\|WARNING\)'
scope 1 --hold-files --ask-cmd 'exit 1' -- /bin/cat "$RUN/outside/secret.txt" 2>&1 | grep -v '^istesso-scope: \(level\|enforced\|NOT\|WARNING\)'

if [ "$TIMING" = timing ]; then
	step "timing: clean cargo build of a project with 3 dependencies (vendored), 3 times each"
	# The dependencies are copied from this user's cargo cache (offline) into the project.
	BENCH="$RUN/project/bench"
	mkdir -p "$BENCH/src" "$BENCH/.cargo"
	cat >"$BENCH/Cargo.toml" <<-'TOML'
		[package]
		name = "scopebench"
		version = "0.1.0"
		edition = "2021"

		[dependencies]
		clap = { version = "4", features = ["derive"] }
		serde = { version = "1", features = ["derive"] }
		serde_json = "1"

		[workspace]
	TOML
	cat >"$BENCH/src/main.rs" <<-'RUST'
		use clap::Parser;
		#[derive(Parser)]
		struct Args {
		    #[arg(long)]
		    n: Option<u32>,
		}
		#[derive(serde::Serialize)]
		struct Out {
		    n: u32,
		}
		fn main() {
		    let a = Args::parse();
		    println!("{}", serde_json::to_string(&Out { n: a.n.unwrap_or(1) }).unwrap());
		}
	RUST
	if (cd "$BENCH" && cargo vendor --offline -q vendor >/dev/null 2>"$RUN/vendor.err"); then
		printf '[source.crates-io]\nreplace-with = "vendored-sources"\n\n[source.vendored-sources]\ndirectory = "vendor"\n' >"$BENCH/.cargo/config.toml"
		TIMEFORMAT=%R
		build='cd "$ISTESSO_SCOPE_PROJECT/bench" && cargo build -q --offline && ./target/debug/scopebench --n 2 && cargo clean -q'
		for mode in none level1 level1-hold; do
			for i in 1 2 3; do
				guard
				case $mode in
				none) t=$( { time env ISTESSO_SCOPE_PROJECT="$RUN/project" CARGO_HOME="$RUN/state/home/.cargo" /bin/sh -c "$build" >"$RUN/bench-$mode-$i.out" 2>&1 ; } 2>&1 ) ;;
				level1) t=$( { time scope 1 -- /bin/sh -c "$build" >"$RUN/bench-$mode-$i.out" 2>&1 ; } 2>&1 ) ;;
				level1-hold) t=$( { time scope 1 --hold-files -- /bin/sh -c "$build" >"$RUN/bench-$mode-$i.out" 2>&1 ; } 2>&1 ) ;;
				esac
				echo "$mode run $i: ${t} s ($(grep -c '"n":2' "$RUN/bench-$mode-$i.out") of 1 runs printed the result)"
			done
		done
		grep -h 'hold-files:' "$RUN"/bench-level1-hold-*.out | sed 's/^/  /'
	else
		echo "skipped: cargo vendor --offline failed ($(tail -1 "$RUN/vendor.err"))"
	fi
	step "timing: python3 importing 20 standard modules, 5 times each"
	py='import json, email, http.client, asyncio, unittest, sqlite3, ssl, xml.dom.minidom, csv, decimal, argparse, logging, subprocess, tempfile, pathlib, typing, dataclasses, urllib.request, zipfile, tarfile'
	for mode in none level1 level1-hold; do
		for i in 1 2 3 4 5; do
			guard
			case $mode in
			none) t=$( { time /usr/bin/python3 -c "$py" ; } 2>&1 ) ;;
			level1) t=$( { time scope 1 -- /usr/bin/python3 -c "$py" 2>/dev/null ; } 2>&1 ) ;;
			level1-hold) t=$( { time scope 1 --hold-files -- /usr/bin/python3 -c "$py" 2>/dev/null ; } 2>&1 ) ;;
			esac
			echo "$mode run $i: ${t} s"
		done
	done
fi

echo
echo "done"
