"""#55: the task board store and seimei's tasks tool: numbering, updates, done, ticks (the user's own only),
trimming, persistence and the live snapshot on every change."""
import asyncio, json, tempfile
from pathlib import Path
from seimei import tasks as T

res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300])

class Ev:
    def __init__(self): self.sent = []
    def flash(self, ev): self.sent.append(ev)

async def main():
    d = Path(tempfile.mkdtemp(prefix="tasks-"))
    ev = Ev()
    b = T.Board(d / "tasks.json", ev)
    ids = [t["id"] for t in b.data["tasks"]]
    check("seeded with today's items", ids[:6] == ["#47", "#48", "#52", "#53", "#54", "#55"] and len(ids) == 13, ids)
    check("seed on disk", json.loads((d / "tasks.json").read_text())["next"] == 56)
    t = b.add("Next thing", "seimei", "dev")
    check("numbering continues at #56", t["id"] == "#56" and ev.sent[-1]["type"] == "tasks", t)
    u = b.add("Buy cable", "desktop", "you", numbered=False)
    check("unnumbered gets t8", u["id"] == "t8", u)
    b.update("56", state="running", worker="abc")
    check("update by bare number", b.find("#56")["state"] == "running" and b.find("#56")["worker"] == "abc")
    b.update("#56", state="done")
    check("done stamps done_at", b.find("#56")["done_at"])
    b.update("#56", state="open")
    check("reopen clears done_at", b.find("#56")["done_at"] is None)
    try:
        b.tick("#56", True); check("tick refuses others' items", False)
    except PermissionError:
        check("tick refuses others' items", True)
    b.tick("t8", True)
    check("tick the user's own", b.find("t8")["state"] == "done")
    b.tick("t8", False)
    check("untick", b.find("t8")["state"] == "open")
    try:
        b.update("#999", state="done"); check("unknown id", False)
    except KeyError:
        check("unknown id", True)
    try:
        b.update("#56", state="bogus"); check("bad state", False)
    except ValueError:
        check("bad state", True)
    b.update("#56", project="new proj")
    check("moving creates the project", any(p["name"] == "new proj" for p in b.data["projects"]))
    b.update("t8", project="new proj")
    check("emptied project drops", not any(p["name"] == "desktop" for p in b.data["projects"])
          or any(t["project"] == "desktop" for t in b.data["tasks"]))
    for i in range(T.DONE_KEEP + 5):
        b.add(f"old {i}", "bulk", "dev", state="done")
    check("done trimmed per project", sum(t["project"] == "bulk" for t in b.data["tasks"]) == T.DONE_KEEP)
    b2 = T.Board(d / "tasks.json")
    check("persists", b2.find("#56")["project"] == "new proj" and b2.data["next"] == b.data["next"])

    tool = T.make_task_tools(b)[0]
    call = tool.handler
    r = await call({"action": "add", "title": "Tool item", "project": "seimei", "owner": "dev", "state": "running", "worker": "w1"})
    tid = r["content"][0]["text"].split()[1]
    check("tool add", tid.startswith("#") and not r["is_error"], r)
    r = await call({"action": "done", "id": tid})
    check("tool done", "done" in r["content"][0]["text"] and b.find(tid)["state"] == "done", r)
    r = await call({"action": "update", "id": tid, "owner": "you"})
    check("tool update owner", b.find(tid)["owner"] == "you", r)
    r = await call({"action": "project", "name": "seimei", "paused": True, "color": "#ff8800"})
    p = b.project("seimei")
    check("tool project pause+color", p["paused"] and p["color"] == "#ff8800", r)
    r = await call({"action": "project", "name": "nope"})
    check("tool project unknown", r["is_error"], r)
    r = await call({"action": "add", "title": "x"})
    check("tool add needs fields", r["is_error"], r)
    r = await call({"action": "list"})
    check("tool list", "seimei (paused) [color #ff8800]" in r["content"][0]["text"], r)
    check("snapshot shape", set(ev.sent[-1]) == {"type", "tasks", "projects"})

    # #57: remove drops an item for good; an emptied project leaves the board; the change goes out live.
    n = len(ev.sent)
    r = await call({"action": "remove", "id": "t5"})
    check("remove", b.find("t5") is None and not r["is_error"], r)
    check("remove goes out live", len(ev.sent) == n + 1 and not any(x["id"] == "t5" for x in ev.sent[-1]["tasks"]))
    check("emptied project leaves the board", not any(p["name"] == "desktop" for p in ev.sent[-1]["projects"]) and "left the board" in r["content"][0]["text"], r)
    r = await call({"action": "remove", "id": "t1"})
    check("remove keeps a project that still has items", any(p["name"] == "seimei" for p in b.data["projects"]) and "left" not in r["content"][0]["text"], r)
    r = await call({"action": "remove", "id": "t1"})
    check("remove unknown", r["is_error"], r)
    r = await call({"action": "remove"})
    check("remove needs an id", r["is_error"], r)
    check("removal persists", T.Board(d / "tasks.json").find("t5") is None)

asyncio.run(main())
print(f"{sum(res)}/{len(res)}")
