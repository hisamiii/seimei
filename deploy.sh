#!/bin/sh
# deploy.sh [rev]: move the production checkout to a commit (default main) and restart seimei once it is idle.
# Production never runs from the working tree, so half-finished edits cannot reach the phone.
# It builds the Rust server here and swaps the binary in right before the restart.
#
# The deploy itself runs detached (a transient unit under systemd, its own session under OpenRC), so the restart it
# ends with cannot cut it off, even when a shikigami (which that restart ends) started it. The caller follows it and prints
# its result; the result also stays in $state/deploy/last.json, which seimei hands to whoever it resumes.
# Tests point it elsewhere with SEIMEI_APP, SEIMEI_STATE, SEIMEI_SERVICE and SEIMEI_DEPLOY_TEST (no installs).
set -eu
rev=${1:-main}
repo=$(dirname "$(readlink -f "$0")")
state=${SEIMEI_STATE:-$HOME/.local/share/seimei}
dir="$state/deploy"
mkdir -p "$dir"
systemd=
[ "$(cat /proc/1/comm 2>/dev/null)" = systemd ] && systemd=1

if [ -z "${SEIMEI_DEPLOY_UNIT:-}" ]; then
    unit="seimei-deploy-$(date +%s)"
    set -- --setenv=PATH="$PATH" --setenv=SEIMEI_DEPLOY_UNIT="$unit"
    for v in SEIMEI_APP SEIMEI_STATE SEIMEI_SERVICE SEIMEI_DEPLOY_TEST; do
        eval "val=\${$v:-}"
        [ -n "$val" ] && set -- "$@" --setenv="$v=$val"
    done
    if [ -n "$systemd" ]; then
        env -u LD_PRELOAD systemd-run --user --collect --quiet --unit "$unit" "$@" "$repo/deploy.sh" "$rev"
    else
        # No subreaper above it: it goes to init, out of reach of seimei's stop and the caller's process group.
        SEIMEI_DEPLOY_UNIT=$unit env -u LD_PRELOAD setsid -f "$repo/deploy.sh" "$rev" </dev/null >/dev/null 2>&1
    fi
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

if [ -z "${SEIMEI_DEPLOY_TEST:-}" ]; then
    step=build
    status running build
    # Its own target dir, so a build in the working tree never races this one. Swapped in right before the restart.
    (cd "$app" && nice -n 19 cargo build -q --release --locked -p seimei)
    mkdir -p "$app/bin"
    cp "$app/target/release/seimei" "$app/bin/seimei.new"
    step=install
    status running install
    # Under OpenRC the user services live in /etc/user/init.d, outside this checkout.
    if [ -n "$systemd" ]; then
        units="$HOME/.config/systemd/user"
        cp --remove-destination "$app/seimei.service" "$units/seimei.service"
        cp --remove-destination "$app/seimei-tunnel.service" "$units/seimei-tunnel.service"
        systemctl --user daemon-reload
    fi
    # The browser connector: its native-messaging host runs from the production checkout, listed where Firefox looks
    # (~/.mozilla, or ~/.config/mozilla with XDG paths); the extension (signed when sign.py made it) is for the user to
    # install once (about:addons, Install Add-on From File).
    cfg=${XDG_CONFIG_HOME:-$HOME/.config}
    python3 "$app/browser/install.py" "$app/browser" "$HOME/.local/share/seimei/seimei-waterfox.xpi" \
        "$cfg/seimei/config.toml" "$HOME/.mozilla/native-messaging-hosts" "$cfg/mozilla/native-messaging-hosts" >/dev/null
    # The Discord connector: Vencord with the SeimeiConnector plugin, for Vesktop to load from its Vencord Location.
    # Without pnpm the last build stays in place; the rest of the deploy goes on.
    if command -v pnpm >/dev/null; then
        "$app/discord/build.sh"
    else
        echo "deploy: no pnpm, the Discord connector build is skipped" >&2
    fi
    [ -z "$systemd" ] || systemctl --user reenable seimei.service seimei-tunnel.service 2>/dev/null
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
if [ -e "$app/bin/seimei.new" ]; then
    mv -f "$app/bin/seimei.new" "$app/bin/seimei"
fi
if [ -n "$systemd" ]; then
    systemctl --user restart "$service"
else
    rc-service --user "$service" restart
fi
line="deployed $(git -C "$app" log --oneline -1)"
echo "$line"
status done restart "$line"
