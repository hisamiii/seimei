"""The clean-install check (phase 6 of docs/rust-rewrite.md): the static release binary on a fresh Linux account.
It creates the locked account seimei-cleantest (no password, no extra groups) through doas, runs `seimei setup` as it,
checks the service serves on 127.0.0.1 and takes the password setup set, then removes the account and checks nothing
is left (processes, user units, linger, home, group, runtime dir). The web password is random and lives only in this
process. Build first with contrib/release.sh's cargo line; run by hand, never from pytest: python3 contrib/cleaninstall.py"""
import json
import platform
import secrets
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

U = "seimei-cleantest"
HOME = Path("/home") / U
PORT = 18417
BIN = Path(__file__).resolve().parents[1] / f"target/{platform.machine()}-unknown-linux-musl/release/seimei"
res, nums = [], {}


def check(n, ok, x=""):
    res.append(bool(ok))
    print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:800], flush=True)


def run(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, cwd="/", **kw)


def root(*args, **kw):
    return run(["doas", *args], **kw)


def as_user(uid, *args, **kw):
    env = [f"HOME={HOME}", f"USER={U}", f"LOGNAME={U}", f"PATH={HOME}/.local/bin:/usr/local/bin:/usr/bin",
           f"XDG_RUNTIME_DIR=/run/user/{uid}", f"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus", "LANG=C.UTF-8"]
    return root("-u", U, "env", "-i", *env, *args, **kw)


def http(method, path, body=None):
    req = urllib.request.Request(f"http://127.0.0.1:{PORT}{path}", method=method,
                                 data=json.dumps(body).encode() if body is not None else None,
                                 headers={"Origin": f"http://localhost:{PORT}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=5) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()
    except OSError as e:
        return None, str(e).encode()


def wait(f, t, step=0.5):
    end = time.time() + t
    while time.time() < end:
        if v := f():
            return v
        time.sleep(step)
    return f()


# Before: no such account, nothing at its paths.
check("before: no such user", run(["id", U]).returncode != 0)
check("before: no home, no linger file", not HOME.exists() and not Path(f"/var/lib/systemd/linger/{U}").exists())
if not all(res):
    sys.exit("refusing to start: leftovers from before")
check("binary: static musl build", "static" in run(["file", "-L", str(BIN)]).stdout and "interpreter" not in run(["file", "-L", str(BIN)]).stdout, run(["file", str(BIN)]).stdout)
nums["binary MB"] = round(BIN.stat().st_size / 1e6, 2)

uid = None
try:
    r = root("useradd", "-m", "-s", "/usr/bin/bash", U)
    check("useradd", r.returncode == 0, r.stderr)
    uid = int(run(["id", "-u", U]).stdout)
    ident = run(["id", U]).stdout.strip()
    check("no extra groups", ident.count("(") == 3 and f"groups={run(['id', '-g', U]).stdout.strip()}({U})" in ident, ident)
    status = root("passwd", "-S", U).stdout.split()
    check("locked: no password", len(status) > 1 and status[1] == "L", status)
    r = root("loginctl", "enable-linger", U)
    check("linger on", r.returncode == 0, r.stderr)
    check("its user manager is up", wait(lambda: root("test", "-S", f"/run/user/{uid}/bus").returncode == 0, 30))

    r = as_user(uid, "mkdir", "-p", f"{HOME}/.local/bin")
    r = root("install", "-o", U, "-g", U, "-m", "755", str(BIN), f"{HOME}/.local/bin/seimei")
    check("binary installed in its home", r.returncode == 0, r.stderr)

    pw = secrets.token_urlsafe(18)
    # Port, origin (default), what to call the machine (default), five plugins (no), password twice, start (yes).
    answers = f"{PORT}\n\n\n" + "n\n" * 5 + f"{pw}\n{pw}\n" + "y\n"
    t0 = time.time()
    r = as_user(uid, f"{HOME}/.local/bin/seimei", "setup", input=answers, timeout=120)
    nums["setup s"] = round(time.time() - t0, 2)
    out = (r.stdout + r.stderr).replace(pw, "<pw>")
    print("--- setup output ---\n" + out.strip() + "\n---", flush=True)
    check("setup exits 0", r.returncode == 0, r.returncode)
    check("setup says it runs", "seimei is running." in out)
    t1 = time.time()
    up = wait(lambda: http("GET", "/")[0] == 200, 30, 0.1)
    nums["first 200 after setup s"] = round(time.time() - t1, 2)
    check("serves the web page", up)
    ss = run(["ss", "-ltnH", f"sport = :{PORT}"]).stdout.split()
    locals_ = [w for w in ss if w.endswith(f":{PORT}")]
    check("listens on 127.0.0.1 only", locals_ == [f"127.0.0.1:{PORT}"], ss)
    check("unauthenticated is refused", http("GET", "/api/auth/me")[0] in (401, 403), http("GET", "/api/auth/me"))
    check("wrong password refused", http("POST", "/api/auth/password", {"password": "wrong-one-123"})[0] == 401)
    code, body = http("POST", "/api/auth/password", {"password": pw})
    check("its password logs in", code == 200, (code, body[:200]))
    del pw

    pid = as_user(uid, "systemctl", "--user", "show", "-p", "MainPID", "--value", "seimei").stdout.strip()
    time.sleep(10)
    st = root("cat", f"/proc/{pid}/status").stdout
    rss = next((int(l.split()[1]) for l in st.splitlines() if l.startswith("VmRSS:")), 0)
    nums["idle RSS MB (10 s)"] = round(rss / 1024, 1)
    nums["home after setup MB"] = round(int(root("du", "-sb", str(HOME)).stdout.split()[0]) / 1e6, 2)
    files = root("find", str(HOME), "-type", "f", "-not", "-path", "*/.claude*").stdout.split()
    print("files it wrote:", sorted(f.replace(str(HOME), "~") for f in files), flush=True)
    log = root("journalctl", f"_UID={uid}", "--no-pager", "-o", "cat").stdout.splitlines()
    print("--- its journal (last 15) ---\n" + "\n".join(log[-15:]) + "\n---", flush=True)
    check("unit active", as_user(uid, "systemctl", "--user", "is-active", "seimei").stdout.strip() == "active")
finally:
    if uid is not None:
        as_user(uid, "systemctl", "--user", "disable", "--now", "seimei")
        root("loginctl", "disable-linger", U)
        root("loginctl", "terminate-user", U)
        gone = wait(lambda: run(["pgrep", "-u", str(uid)]).returncode == 1, 30)
        check("cleanup: no processes before userdel (pgrep -u)", gone, run(["pgrep", "-a", "-u", str(uid)]).stdout)
        units = root("systemctl", "list-units", "--all", "--plain", "--no-legend", f"user@{uid}.service",
                     f"user-runtime-dir@{uid}.service").stdout.strip()
        check("cleanup: no user units left", not units or all(" inactive " in l for l in units.splitlines()), units)
        r = root("userdel", "-r", U)
        check("cleanup: userdel -r", r.returncode == 0, r.stderr)
    check("cleanup: linger disabled", not Path(f"/var/lib/systemd/linger/{U}").exists())
    check("cleanup: id fails", run(["id", U]).returncode != 0)
    check("cleanup: no group left", run(["getent", "group", U]).returncode != 0)
    check("cleanup: home gone", not HOME.exists())
    check("cleanup: no mail spool", not Path(f"/var/spool/mail/{U}").exists())
    if uid is not None:
        check("cleanup: no processes after", run(["pgrep", "-u", str(uid)]).returncode == 1)
        check("cleanup: runtime dir gone", root("test", "-e", f"/run/user/{uid}").returncode != 0)
        # userdel -r leaves temp files; Claude Code keeps /tmp/claude-<uid>. Empty dirs go, anything else is reported.
        root("find", "/tmp", "/var/tmp", "/dev/shm", "-xdev", "-uid", str(uid), "-type", "d", "-empty", "-delete")
        left = root("find", "/tmp", "/var/tmp", "/dev/shm", "-xdev", "-uid", str(uid)).stdout.split()
        check("cleanup: no temp files of its uid", not left, left)
    check("cleanup: port free", not run(["ss", "-ltnH", f"sport = :{PORT}"]).stdout.strip())
    print(json.dumps(nums), flush=True)
    print(f"{sum(res)}/{len(res)} passed", flush=True)
