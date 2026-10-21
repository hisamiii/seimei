#!/bin/sh
# Waybar custom module: a red "cast" chip while seimei streams the screen to the phone or page (#30).
# seimei's cast.py writes {"casts": [{"pid": ...}, ...]} to $XDG_RUNTIME_DIR/seimei-cast.json while a capture runs
# and signals the bar (SIGRTMIN+9). Only a live castd counts, so a crash never leaves the chip on.
f="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/seimei-cast.json"
for pid in $(grep -o '"pid": *[0-9]*' "$f" 2>/dev/null | grep -o '[0-9]*$'); do
    if grep -q castd "/proc/$pid/cmdline" 2>/dev/null; then
        printf '{"text":"live","class":"casting","tooltip":"seimei is streaming this screen"}\n'
        exit 0
    fi
done
printf '{"text":"","class":"idle"}\n'
