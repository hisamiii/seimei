#!/bin/bash
pid=$(ss -ltnpH 'sport = :8431' | grep -o 'pid=[0-9]*' | head -1 | cut -d= -f2)
# Only a QA server (seimei --dev) is stopped: whatever else holds the port is left alone.
if [ -n "$pid" ]; then
    mapfile -d '' args < "/proc/$pid/cmdline"
    if [ "${args[0]##*/}" != seimei ] || [[ " ${args[*]:1} " != *" --dev "* ]]; then
        echo "port 8431 is held by something else: ${args[*]}" >&2
        exit 1
    fi
fi
[ -n "$pid" ] && kill "$pid"
echo stopped $pid
python3 "$(dirname "$0")/reap.py"
