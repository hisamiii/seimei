#!/bin/bash
# tests/live.sh NAME...: run the ignored live tests (headless sessions on haiku, dsh or Codex) in a scrubbed environment.
# No desktop, no job variables of the calling session, auto-memory off, and the test Claude config dir, so test sessions
# never show up in the user's session list or seimei's events. Log it in once: `CLAUDE_CONFIG_DIR=<dir> claude /login`.
# NAME: claude (proto), perm, tune (core), dispatcher, plugin, acp (seimei), codex (the schema check, then a live run).
set -eu
root=$(dirname "$(dirname "$(readlink -f "$0")")")
cd "$root"
cfg=${SEIMEI_TEST_CLAUDE_DIR:-$HOME/.local/share/seimei-test/claude}
for v in $(env | grep -o '^\(CLAUDE\|SEIMEI_\)[A-Za-z0-9_]*'); do unset "$v"; done
unset DISPLAY WAYLAND_DISPLAY NIRI_SOCKET SWAYSOCK HYPRLAND_INSTANCE_SIGNATURE KITTY_LISTEN_ON KITTY_WINDOW_ID \
    XDG_CURRENT_DESKTOP LD_PRELOAD
export CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 CLAUDE_CONFIG_DIR="$cfg"

run() { cargo test -q -p "$1" --test it -- --ignored "$2" --nocapture; }

[ $# -gt 0 ] || { echo "usage: tests/live.sh claude|perm|tune|dispatcher|plugin|acp|codex ..."; exit 2; }
for name in "$@"; do
    case $name in
    claude) run seimei-proto live:: ;;
    perm | tune) run seimei-core "live_$name::" ;;
    dispatcher | plugin) run seimei "live_$name::" ;;
    acp)
        # dsh runs in a temp HOME whose only key is a decoy, so a read the guard failed to stop would leak nothing.
        home=$(mktemp -d /tmp/seimei-acphome-XXXXXX)
        mkdir -m 700 "$home/.ssh"
        echo "DECOY-$(head -c 8 /dev/urandom | od -An -tx1 | tr -d ' \n')" > "$home/.ssh/id_ed25519"
        real=$HOME rc=0
        HOME=$home DSH_HOME=${DSH_HOME:-$real/.dsh} CARGO_HOME=${CARGO_HOME:-$real/.cargo} \
            RUSTUP_HOME=${RUSTUP_HOME:-$real/.rustup} run seimei live_acp:: || rc=$?
        case $home in /tmp/seimei-acphome-?*) rm -r -- "$home" ;; esac
        [ "$rc" -eq 0 ] || exit "$rc"
        ;;
    codex)
        python3 tests/codex-schema.py
        run seimei-proto live_codex::
        ;;
    *) echo "unknown live test $name"; exit 2 ;;
    esac
done
