#!/bin/sh
# deploy.sh [rev]: move the production checkout to a commit (default main) and restart seimei once it is idle.
# Production never runs from the working tree, so half-finished edits cannot reach the phone.
set -eu
rev=${1:-main}
app="$HOME/.local/share/seimei-app"
repo=$(dirname "$(readlink -f "$0")")
[ -d "$app" ] || git -C "$repo" worktree add --detach "$app" "$rev"
git -C "$app" checkout -q --detach "$rev"
(cd "$app" && uv sync -q --frozen)
units="$HOME/.config/systemd/user"
for u in seimei.service seimei-tunnel.service; do
    cp --remove-destination "$app/$u" "$units/$u"
done
systemctl --user daemon-reload
# The Waterfox connector: its native-messaging host runs from the production checkout; the extension is packed
# for the user to install once (about:addons, Install Add-on From File).
hosts="$HOME/.mozilla/native-messaging-hosts"
mkdir -p "$hosts"
printf '{"name": "seimei", "description": "seimei connector", "path": "%s", "type": "stdio", "allowed_extensions": ["seimei@localhost"]}\n' \
    "$app/browser/host.py" > "$hosts/seimei.json"
python3 - "$app/browser" "$HOME/.local/share/seimei/seimei-waterfox.xpi" <<'PY'
import sys, zipfile
from pathlib import Path
src, out = Path(sys.argv[1]), Path(sys.argv[2])
tmp = out.with_suffix(".tmp")
with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED) as z:
    for f in ("manifest.json", "background.js", "read.js", "icon.svg"):
        z.write(src / f, f)
tmp.replace(out)
PY
systemctl --user reenable seimei.service seimei-tunnel.service 2>/dev/null
# Wait up to 5 minutes for 20 quiet seconds and no open Approve/Deny card, so no turn or held call is cut off.
python3 - <<'PY'
import json, os, time
log = os.path.expanduser("~/.local/share/seimei/events.jsonl")
for _ in range(60):
    try:
        with open(log, "rb") as f:
            start = max(f.seek(0, 2) - (1 << 20), 0)
            f.seek(start)
            tail = [json.loads(l) for l in f.read().splitlines()[1 if start else 0:]]
        last = tail[-1]
    except (OSError, IndexError, ValueError):
        break
    done = {e["aid"] for e in tail if e["type"] == "approval_done"}
    open_cards = [e for e in tail if e["type"] == "approval" and e["aid"] not in done]
    if time.time() - last["t"] > 20 and not open_cards:
        break
    time.sleep(5)
PY
systemctl --user restart seimei
echo "deployed $(git -C "$app" log --oneline -1)"
