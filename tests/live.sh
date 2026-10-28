#!/bin/bash
# tests/live.sh NAME...: run the ignored live tests (headless sessions on haiku or dsh) in a scrubbed environment.
# No desktop, no job variables of the calling session, auto-memory off, and the test Claude config dir, so test sessions stay out of the user's list.
# Log it in once: `CLAUDE_CONFIG_DIR=<dir> claude /login`.
# NAME: dispatcher (the dispatcher with its tools and plugins), acp (a shikigami on dsh).
set -eu
root=$(dirname "$(dirname "$(readlink -f "$0")")")
cd "$root"
cfg=${SEIMEI_TEST_CLAUDE_DIR:-$HOME/.local/share/seimei-test/claude}
for v in $(env | grep -o '^\(CLAUDE\|SEIMEI_\)[A-Za-z0-9_]*'); do unset "$v"; done
unset DISPLAY WAYLAND_DISPLAY NIRI_SOCKET SWAYSOCK HYPRLAND_INSTANCE_SIGNATURE KITTY_LISTEN_ON KITTY_WINDOW_ID \
    XDG_CURRENT_DESKTOP LD_PRELOAD
export CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 CLAUDE_CONFIG_DIR="$cfg"

run() { cargo test -q -p "$1" --test it -- --ignored "$2" --nocapture; }

[ $# -gt 0 ] || { echo "usage: tests/live.sh dispatcher|acp ..."; exit 2; }
for name in "$@"; do
    case $name in
    dispatcher) run seimei live_dispatcher:: ;;
    acp)
        # dsh runs in a temp HOME: the commands it is asked to run touch nothing of the user's.
        home=$(mktemp -d /tmp/seimei-acphome-XXXXXX)
        real=$HOME rc=0
        HOME=$home DSH_HOME=${DSH_HOME:-$real/.dsh} CARGO_HOME=${CARGO_HOME:-$real/.cargo} \
            RUSTUP_HOME=${RUSTUP_HOME:-$real/.rustup} run seimei live_acp:: || rc=$?
        case $home in /tmp/seimei-acphome-?*) rm -r -- "$home" ;; esac
        [ "$rc" -eq 0 ] || exit "$rc"
        ;;
    *) echo "unknown live test $name"; exit 2 ;;
    esac
done
