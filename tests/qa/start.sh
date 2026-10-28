#!/bin/bash
# Start (or restart) the QA server on 8431: own config and state under tests/qa, the fake agent CLI (tests/fakes), a fake kitty and terminal.
# Nothing reaches a real agent session, kitty window or phone.
# SEIMEI_SERVER is the server to run (default: this checkout's debug build); flags go to seed.py.
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$(dirname "$here")")
pid=$(ss -ltnpH 'sport = :8431' | grep -o 'pid=[0-9]*' | head -1 | cut -d= -f2)
[ -n "$pid" ] && kill "$pid" && sleep 2
cd "$root" || exit 1
empty=
for a in "$@"; do [ "$a" = --empty ] && empty=-empty; done
: > tests/qa/server.log
python3 tests/qa/seed.py "$@" || exit 1
# The test env scrubs CLAUDE_* and SEIMEI_*: everything the server needs is set here.
for v in $(env | grep -o '^CLAUDE[A-Z_]*'); do unset "$v"; done
export XDG_CONFIG_HOME="$here/cfg" XDG_DATA_HOME="$here/data$empty" XDG_RUNTIME_DIR="$here/run" \
    CLAUDE_CONFIG_DIR="$here/fakeclaude$empty" CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 \
    SEIMEI_CLAUDE="$root/tests/fakes/claude.py" SEIMEI_BATCH=0.5 PATH="$root/tests/fakes/bin:$PATH" \
    FAKE_CLAUDE_LOG="$here/delivered.log" FAKE_KITTEN_LOG="$here/typed.log" FAKE_TERMINAL_LOG="$here/released.log"
server=${SEIMEI_SERVER:-"$root/target/debug/seimei"}
login=; keep=
for a in "$@"; do [ "$a" = --login ] && login=1; [ "$a" = --keep ] && keep=1; done
[ -n "$login" ] && [ -z "$keep" ] && printf 'qa-password-123\nqa-password-123\n' | $server passwd --dev --keep-sessions >/dev/null
setsid nice -n 5 $server --dev >> tests/qa/server.log 2>&1 < /dev/null &
for i in $(seq 1 40); do curl -s -o /dev/null localhost:8431/ && break; sleep 0.5; done
echo "qa server up"
