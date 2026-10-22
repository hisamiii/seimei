#!/usr/bin/env python3
"""Stands in for the desktop's command line tools in tests (niri, swaymsg, hyprctl, xprop, xdotool, wmctrl, xkill,
grim, import, makoctl, wl-copy, wl-paste), linked under each name into a temp folder first on PATH. Logs each call
as a JSON line to $FAKE_DESKTOP_LOG. `niri msg action ...` succeeds; `niri msg --json <what>` prints <what> from the
$FAKE_DESKTOP_STATE file; everything else fails, so no test reaches a real desktop."""
import json
import os
import sys

name = os.path.basename(sys.argv[0])
args = sys.argv[1:]
with open(os.environ["FAKE_DESKTOP_LOG"], "a") as f:
    f.write(json.dumps([name, *args]) + "\n")
if name == "niri" and args[:2] == ["msg", "action"]:
    sys.exit(0)
if name == "niri" and args[:2] == ["msg", "--json"] and len(args) == 3:
    state = json.load(open(os.environ["FAKE_DESKTOP_STATE"]))
    print(json.dumps(state.get(args[2], {})))
    sys.exit(0)
print(f"fake {name}: not set up", file=sys.stderr)
sys.exit(1)
