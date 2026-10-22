"""#87 adopt starts the shikigami before it ends the user's session. When the start fails (the fake claude refuses that
session's resume), the session's process still runs and nothing is left behind: no worker, no board task, no summoned
event. When it works, the new process started while the old one still ran, and the old one is gone afterwards.

Only QA stand-ins get adopted: before approving, the target must be a `sleep 7200` writing to the QA server.log."""
import json, os, sys, time, urllib.error, urllib.request
from pathlib import Path

U = "http://localhost:8431"
QA = Path(__file__).resolve().parents[1] / "qa"
FAKE = QA / "fakeclaude"
STATE = QA / "data/seimei/dev"
res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:400], flush=True)


def standin(pid) -> bool:
    try:
        return (Path(f"/proc/{pid}/cmdline").read_bytes() == b"sleep\x007200\x00"
                and os.readlink(f"/proc/{pid}/fd/1") == str(QA / "server.log"))
    except OSError:
        return False


def session(name) -> dict:
    for f in (FAKE / "sessions").glob("*.json"):
        r = json.loads(f.read_text())
        if r.get("name") == name:
            return r
    raise SystemExit(f"no QA session {name}")


class Api:
    def __init__(self):
        req = urllib.request.Request(U + "/api/auth/password", data=b'{"password": "qa-password-123"}',
                                     headers={"Origin": U, "Content-Type": "application/json"})
        self.h = {"Cookie": urllib.request.urlopen(req).headers["Set-Cookie"].split(";")[0], "Origin": U,
                  "Content-Type": "application/json"}

    def call(self, p, body=None):
        r = urllib.request.Request(U + p, headers=self.h, data=None if body is None else json.dumps(body).encode())
        try:
            with urllib.request.urlopen(r) as resp:
                return json.loads(resp.read() or b"null")
        except urllib.error.HTTPError as e:
            return {"status": e.code}

    def last(self):
        return self.call("/api/history?limit=1")["last"]

    def events(self, after):
        return [e for e in self.call("/api/history?limit=200")["events"] if e["id"] > after]

    def adopt(self, name):
        """!call adopt on a stand-in, approve its card, and return the tool's result text."""
        time.sleep(2)
        start = self.last()
        self.call("/api/send", {"text": f"!call adopt {json.dumps({'session': name})}"})
        decided = False
        for _ in range(400):
            time.sleep(0.2)
            evs = self.events(start)
            card = next((e for e in evs if e["type"] == "approval"), None)
            if card and not decided:
                pid = session(name)["pid"]
                if not standin(pid):
                    sys.exit(f"refusing: {name} (pid {pid}) is not a QA stand-in")
                self.call(f"/api/approvals/{card['aid']}", {"decision": "approve"})
                decided = True
            call = next((e for e in evs if e["type"] == "tool" and e.get("name") == "mcp__seimei__adopt"), None)
            if call:
                done = next((e for e in evs if e["type"] == "tool_result" and e.get("tool_id") == call.get("tool_id")), None)
                if done:
                    return done.get("text", ""), evs
        return "(no result)", self.events(start)


def resumes():
    p = FAKE / "resumes.log"
    return [json.loads(x) for x in p.read_text().splitlines()] if p.exists() else []


api = Api()
time.sleep(4)
NAME = "a-session-with-an-extremely-long-name-that-should-truncate-nicely-on-phones"  # its folder exists

# A start that fails: nothing ends.
s = session(NAME)
(FAKE / "fail-resume").write_text(s["sessionId"] + "\n")
before = {w["name"] for w in json.loads((STATE / "workers.json").read_text())}
tasks = len(json.loads((STATE / "tasks.json").read_text())["tasks"]) if (STATE / "tasks.json").exists() else 0
text, evs = api.adopt(NAME)
check("the reply says nothing ended", "did not start" in text and "nothing ended" in text
      and f'session "{NAME}" still runs' in text, text)
check("its process still runs", standin(s["pid"]), s["pid"])
check("the fake was asked to resume it", any(r["sid"] == s["sessionId"] for r in resumes()), resumes())
after = {w["name"] for w in json.loads((STATE / "workers.json").read_text())}
check("no worker is left", after == before, after - before)
check("no task is filed", (len(json.loads((STATE / "tasks.json").read_text())["tasks"]) if (STATE / "tasks.json").exists() else 0) == tasks)
check("no summoned event", not any(e["type"] == "worker" and e.get("event") == "summoned" for e in evs),
      [e for e in evs if e["type"] == "worker"])
check("listing has no such shikigami", not any(w.get("adopted") == NAME or w["name"] == NAME[:32] for w in api.call("/api/workers")["workers"]))

# The same session once the start works: the new process starts first, then the old one ends.
(FAKE / "fail-resume").write_text("")
(FAKE / "resumes.log").write_text("")
text, evs = api.adopt(NAME)
check("adopted", text.startswith(f'Adopted session "{NAME}" as shikigami '), text)
first = next((r for r in resumes() if r["sid"] == s["sessionId"]), None)
check("the new process started while the old one ran", first and first["old_alive"] is True, resumes())
check("the old one is gone after", not standin(s["pid"]), s["pid"])
w = next((w for w in api.call("/api/workers")["workers"] if f"({w['id']})" in text), None)
check("the shikigami is listed", w is not None)
check("one summoned event", sum(e["type"] == "worker" and e.get("event") == "summoned" for e in evs) == 1)
print(f"{sum(res)}/{len(res)} passed")
