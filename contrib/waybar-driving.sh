#!/bin/sh
# Waybar custom module: a "seimei" dot while seimei drives the desktop.
# seimei writes {"until": <unix time>} to $XDG_RUNTIME_DIR/seimei-driving on every input it sends.
f="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/seimei-driving"
until=$(sed -n 's/.*"until": *\([0-9]*\).*/\1/p' "$f" 2>/dev/null)
if [ -n "$until" ] && [ "$until" -gt "$(date +%s)" ]; then
    printf '{"text":"● seimei","class":"driving","tooltip":"seimei is driving the desktop"}\n'
else
    printf '{"text":"","class":"idle"}\n'
fi
