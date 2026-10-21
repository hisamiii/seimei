"""#45 restart-safe deploys: a deploy started inside the service it restarts survives that restart and its result
reaches the resumed shikigami; a failing deploy says where it failed. Throwaway unit and state, no installs."""
import json, os, subprocess, time
from pathlib import Path

R = Path(__file__).resolve().parents[2]
S = Path.home() / "yah-scratch/seimei-deploytest/state"
APP = Path.home() / "yah-scratch/seimei-deploytest/app"
UNIT = "seimei-deploytest"
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:400], flush=True)

def wait(f, t):
    end = time.time() + t
    while time.time() < end:
        if v := f():
            return v
        time.sleep(0.5)

def alive(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except OSError:
        return False

S.mkdir(parents=True)
rec = {"id": "w1", "name": "deployer", "task": "deploy", "cwd": "/tmp", "model": "haiku", "session_id": "s1", "state": "running",
       "created": time.time() - 60, "updated": time.time() - 5, "summary": "", "started": True}
(S / "workers.json").write_text(json.dumps([rec]))
(S / "workers").mkdir()
(S / "events.jsonl").write_text(json.dumps({"type": "system", "text": "x", "id": 1, "t": time.time() - 60}) + "\n")
env = {"SEIMEI_STATE": str(S), "SEIMEI_APP": str(APP), "SEIMEI_SERVICE": UNIT, "SEIMEI_DEPLOY_TEST": "1",
       "DEPLOY_REV": subprocess.run(["git", "-C", str(R), "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()}
subprocess.run(["env", "-u", "LD_PRELOAD", "systemd-run", "--user", "--quiet", "--unit", UNIT, "-p", "Restart=no",
                *[f"--setenv={k}={v}" for k, v in env.items()], f"--setenv=PATH={os.environ['PATH']}",
                str(R / ".venv/bin/python"), str(R / "tests/fakes/deployfake.py")], check=True)
try:
    caller = int(wait(lambda: (S / "caller.pid").exists() and (S / "caller.pid").read_text(), 15))
    check("the shikigami's deploy started", caller)
    done = wait(lambda: (S / "resumed.txt").exists(), 240)
    last = json.loads((S / "deploy/last.json").read_text())
    check("the restart ended the caller", not alive(caller) and not (S / "caller.exit").exists())
    check("the deploy went on and finished", last["state"] == "done" and last["result"].startswith(f"deployed {env['DEPLOY_REV']}"), last)
    check("its unit is not the service's", last["unit"].startswith("seimei-deploy-") and UNIT not in last["unit"], last)
    told = (S / "resumed.txt").read_text() if done else ""
    check("the resumed shikigami reads the result", told.startswith("continue: seimei restarted") and f"finished: deployed {env['DEPLOY_REV']}" in told, told)
    check("the service started twice", (S / "starts").read_text() == "2")
    out = (S / "caller.out").read_text()
    check("the caller saw it start", "deploying" in out and "log:" in out, out)
finally:
    subprocess.run(["systemctl", "--user", "stop", UNIT], capture_output=True)
    subprocess.run(["systemctl", "--user", "reset-failed", UNIT], capture_output=True)

# A failing deploy: the caller hears where it failed and exits non-zero; last.json and the resume note say so.
p = subprocess.run([str(R / "deploy.sh"), "no-such-rev"], capture_output=True, text=True, timeout=120,
                   env={**os.environ, **{k: v for k, v in env.items() if k != "DEPLOY_REV"}})
last = json.loads((S / "deploy/last.json").read_text())
check("failing: caller exits 1 and names the step", p.returncode == 1 and "deploy failed at checkout" in p.stdout, (p.returncode, p.stdout[-300:]))
check("failing: last.json says failed", last["state"] == "failed" and last["step"] == "checkout", last)
import sys; sys.path.insert(0, str(R))
from seimei import deploys
n = deploys.note(S)
check("failing: the resume note says where and gives the log", "failed at its checkout step" in n and last["log"] in n, n)
check("history keeps both", len((S / "deploy/history.jsonl").read_text().splitlines()) == 2)
print(f"{sum(res)}/{len(res)} passed")
