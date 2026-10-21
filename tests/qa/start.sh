#!/bin/bash
# Start (or restart) the QA server on 8431: own state, fake ~/.claude, fake dispatcher and workers, kitty stubbed.
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$(dirname "$here")")
pid=$(ss -ltnpH 'sport = :8431' | grep -o 'pid=[0-9]*' | head -1 | cut -d= -f2)
[ -n "$pid" ] && kill "$pid" && sleep 2
cd "$root"
PYTHONPATH="$root" setsid nice -n 5 .venv/bin/python tests/qa/run.py "$@" > tests/qa/server.log 2>&1 < /dev/null &
for i in $(seq 1 40); do curl -s -o /dev/null localhost:8431/ && break; sleep 0.5; done
echo "qa server up"
