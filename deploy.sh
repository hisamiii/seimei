#!/bin/sh
# deploy.sh [rev]: move the production checkout to a commit (default main) and restart seimei once it is idle.
# Production never runs from the working tree, so half-finished edits cannot reach the phone.
#
# The deploy itself runs as its own transient unit, outside seimei's cgroup and the caller's, so the restart it ends
# with cannot cut it off, even when a shikigami (which that restart ends) started it. The caller follows it and prints
# its result; the result also stays in $state/deploy/last.json, which seimei hands to whoever it resumes.
# Tests point it elsewhere with SEIMEI_APP, SEIMEI_STATE, SEIMEI_SERVICE and SEIMEI_DEPLOY_TEST (no installs).
set -eu
rev=${1:-main}
repo=$(dirname "$(readlink -f "$0")")
state=${SEIMEI_STATE:-$HOME/.local/share/seimei}
dir="$state/deploy"
mkdir -p "$dir"

if [ -z "${SEIMEI_DEPLOY_UNIT:-}" ]; then
    unit="seimei-deploy-$(date +%s)"
    set -- --setenv=PATH="$PATH" --setenv=SEIMEI_DEPLOY_UNIT="$unit"
    for v in SEIMEI_APP SEIMEI_STATE SEIMEI_SERVICE SEIMEI_DEPLOY_TEST; do
        eval "val=\${$v:-}"
        [ -n "$val" ] && set -- "$@" --setenv="$v=$val"
    done
    env -u LD_PRELOAD systemd-run --user --collect --quiet --unit "$unit" "$@" "$repo/deploy.sh" "$rev"
    echo "deploying $rev in $unit (log: $dir/$unit.log)"
    # Follow it: the log as it grows, then its result. If the restart ends this caller, the deploy goes on.
    exec python3 - "$dir" "$unit" <<'PY'
import json, sys, time
from pathlib import Path
d, unit = Path(sys.argv[1]), sys.argv[2]
log, at = d / f"{unit}.log", 0

def show():
    global at
    if log.exists():
        with open(log, "rb") as f:
            f.seek(at)
            chunk = f.read()
            at += len(chunk)
        sys.stdout.write(chunk.decode(errors="replace"))
        sys.stdout.flush()

while True:
    time.sleep(1)
    show()
    try:
        last = json.loads((d / "last.json").read_text())
    except (OSError, ValueError):
        continue
    if last.get("unit") == unit and last.get("state") != "running":
        show()
        if last["state"] == "failed":
            print(f"deploy failed at {last.get('step')}: {last.get('result', '')} (log: {log})")
            sys.exit(1)
        sys.exit(0)
PY
fi

# From here on: inside the deploy's own unit.
exec >>"$dir/$SEIMEI_DEPLOY_UNIT.log" 2>&1
status() {
    python3 - "$dir" "$SEIMEI_DEPLOY_UNIT" "$rev" "$@" <<'PY'
import json, os, sys, time
d, unit, rev, state, step = sys.argv[1:6]
result = sys.argv[6] if len(sys.argv) > 6 else ""
path = os.path.join(d, "last.json")
try:
    cur = json.load(open(path))
except (OSError, ValueError):
    cur = {}
if cur.get("unit") != unit:
    cur = {"unit": unit, "rev": rev, "started": time.time(), "log": os.path.join(d, unit + ".log")}
cur.update(state=state, step=step, updated=time.time())
if result:
    cur["result"] = result
if state != "running":
    cur["finished"] = time.time()
with open(path + ".tmp", "w") as f:
    json.dump(cur, f)
os.replace(path + ".tmp", path)
if state != "running":
    with open(os.path.join(d, "history.jsonl"), "a") as f:
        f.write(json.dumps(cur) + "\n")
    # Keep the newest 20 logs.
    logs = sorted((n for n in os.listdir(d) if n.startswith("seimei-deploy-") and n.endswith(".log")),
                  key=lambda n: os.path.getmtime(os.path.join(d, n)))
    for n in logs[:-20]:
        os.unlink(os.path.join(d, n))
PY
}
step=checkout
trap 'rc=$?; [ "$rc" -eq 0 ] || status failed "$step" "exit $rc"' EXIT
status running checkout

app=${SEIMEI_APP:-$HOME/.local/share/seimei-app}
service=${SEIMEI_SERVICE:-seimei}
[ -d "$app" ] || git -C "$repo" worktree add --detach "$app" "$rev"
git -C "$app" checkout -q --detach "$rev"
step=sync
status running sync
(cd "$app" && uv sync -q --frozen)

if [ -z "${SEIMEI_DEPLOY_TEST:-}" ]; then
    step=install
    status running install
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
    # The Discord connector: Vencord with the SeimeiConnector plugin, for Vesktop to load from its Vencord Location.
    "$app/discord/build.sh"
    systemctl --user reenable seimei.service seimei-tunnel.service 2>/dev/null
fi

# Wait up to 5 minutes for 20 quiet seconds and no open Approve/Deny card, so no turn or held call is cut off.
step=wait
status running wait
python3 - "$state/events.jsonl" <<'PY'
import json, sys, time
log = sys.argv[1]
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
step=restart
status running restart
systemctl --user restart "$service"
line="deployed $(git -C "$app" log --oneline -1)"
echo "$line"
status done restart "$line"
