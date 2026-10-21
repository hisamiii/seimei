#!/bin/bash
pid=$(ss -ltnpH 'sport = :8431' | grep -o 'pid=[0-9]*' | head -1 | cut -d= -f2)
[ -n "$pid" ] && kill "$pid"
echo stopped $pid
python3 "$(dirname "$0")/reap.py"
