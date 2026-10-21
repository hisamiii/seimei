#!/bin/bash
# Start (or restart) the QA server on 8431: own config and state under tests/qa, the fake claude (tests/fakes), a
# fake kitty and terminal. Nothing reaches a real Claude session, kitty window or phone.
# SEIMEI_SERVER is the server to run (default: this checkout's Python seimei); flags go to seed.py.
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$(dirname "$here")")
pid=$(ss -ltnpH 'sport = :8431' | grep -o 'pid=[0-9]*' | head -1 | cut -d= -f2)
[ -n "$pid" ] && kill "$pid" && sleep 2
cd "$root" || exit 1
empty=
for a in "$@"; do [ "$a" = --empty ] && empty=-empty; done
: > tests/qa/server.log
PYTHONPATH="$root" .venv/bin/python tests/qa/seed.py "$@" || exit 1
# The test env scrubs CLAUDE_* and SEIMEI_*: everything the server needs is set here.
for v in $(env | grep -o '^CLAUDE[A-Z_]*'); do unset "$v"; done
export XDG_CONFIG_HOME="$here/cfg" XDG_DATA_HOME="$here/data$empty" XDG_RUNTIME_DIR="$here/run" \
    CLAUDE_CONFIG_DIR="$here/fakeclaude$empty" CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 \
    SEIMEI_CLAUDE="$root/tests/fakes/claude.py" SEIMEI_BATCH=0.5 PATH="$root/tests/fakes/bin:$PATH" \
    FAKE_CLAUDE_LOG="$here/delivered.log" FAKE_KITTEN_LOG="$here/typed.log" FAKE_TERMINAL_LOG="$here/released.log" \
    PYTHONPATH="$root"
server=${SEIMEI_SERVER:-"$root/.venv/bin/python -m seimei"}
setsid nice -n 5 $server --dev >> tests/qa/server.log 2>&1 < /dev/null &
for i in $(seq 1 40); do curl -s -o /dev/null localhost:8431/ && break; sleep 0.5; done
echo "qa server up"
