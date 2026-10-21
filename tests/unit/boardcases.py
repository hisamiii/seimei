"""tests/cases/tasks.json on the Python board: the store, the tasks tool, following shikigami, queues and drift."""
import asyncio, json, tempfile, time
from pathlib import Path
from seimei import tasks as T

CASES = json.loads((Path(__file__).resolve().parents[1] / "cases/tasks.json").read_text())
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:400])

def fits(want, got) -> bool:
    if want is None:
        return got is None
    if want == "<set>":
        return got is not None
    if isinstance(want, dict):
        return isinstance(got, dict) and all(fits(v, got.get(k)) for k, v in want.items())
    return want == got

async def run(s: dict):
    d = Path(tempfile.mkdtemp(prefix="board-"))
    if "start" in s:
        (d / "tasks.json").write_text(json.dumps(s["start"]))
    b = T.Board(d / "tasks.json")
    tool = T.make_task_tools(b)[0].handler
    for i, step in enumerate(s["steps"]):
        do, a, want = step["do"], step.get("args") or {}, step["expect"]
        name = f"{s['what']} [{i}] {do} {json.dumps(a)[:80]}"
        out, err = None, None
        try:
            if do == "reload":
                b = T.Board(d / "tasks.json")
                tool = T.make_task_tools(b)[0].handler
            elif do == "add":
                out = b.add(a["title"], a["project"], a["owner"], a.get("state", "open"), a.get("worker"), a.get("numbered", True), a.get("tid"))
            elif do == "add_many":
                for n in range(a["n"]):
                    b.add(f"{a['title']} {n}", a["project"], a["owner"], a["state"])
            elif do == "update":
                out = b.update(a["id"], **{k: v for k, v in a.items() if k != "id"})
            elif do == "remove":
                out = b.remove(a["id"])
            elif do == "tick":
                out = b.tick(a["id"], a["done"])
            elif do == "order":
                out = b.order(a["ids"])
            elif do == "attach":
                out = b.attach(a["worker"], a["owner"], a["task"], a["project"], a["title"], a.get("link"))
            elif do == "follow":
                b.follow(a["worker"], a["wstate"])
            elif do == "start":
                out = b.start(a["worker"], a["name"], a["wstate"], a["id"])
            elif do == "ship":
                out = b.ship(a["worker"], a["name"], a["id"])
            elif do == "step":
                b.step(a["worker"], a["sid"], a.get("title"), a.get("state"))
            elif do == "age":
                b.find(a["id"])["since"] = time.time() - a["since_ago"]
            elif do == "drift":
                out = b.drift({k: tuple(v) for k, v in a["workers"].items()})
            elif do == "tool":
                r = await tool(a)
                out = {"text": r["content"][0]["text"], "error": bool(r["is_error"])}
            elif do != "look":
                raise SystemExit(f"unknown step {do}")
        except (KeyError, ValueError, PermissionError) as e:
            err = str(e).strip("'")
        if "error" in want:
            check(name, err == want["error"], err)
        elif err:
            check(name, False, f"failed: {err}")
            continue
        if "result" in want:
            check(name + " result", fits(want["result"], out), out)
        tasks = {t["id"]: t for t in b.data["tasks"]}
        projects = {p["name"]: p for p in b.data["projects"]}
        for tid, sub in (want.get("tasks") or {}).items():
            check(f"{name} task {tid}", (tid not in tasks) if sub is None else fits(sub, tasks.get(tid)), tasks.get(tid))
        for pn, sub in (want.get("projects") or {}).items():
            check(f"{name} project {pn}", (pn not in projects) if sub is None else fits(sub, projects.get(pn)), projects.get(pn))
        for pn, n in (want.get("count") or {}).items():
            got = sum(t["project"] == pn for t in b.data["tasks"])
            check(f"{name} count {pn}", got == n, got)
        if "data" in want:
            check(name + " data", fits(want["data"], {k: b.data[k] for k in ("next", "other")}), b.data["next"])
        for w, tid in (want.get("current") or {}).items():
            got = (b.current(w) or {}).get("id")
            check(f"{name} current {w}", got == tid, got)
        for owner, ids in (want.get("queue") or {}).items():
            got = [t["id"] for t in sorted((t for t in b.data["tasks"] if t["owner"] == owner and t["state"] != "done"), key=lambda t: t["pos"])]
            check(f"{name} queue {owner}", got == ids, got)
        if "first_ids" in want:
            got = [t["id"] for t in b.data["tasks"]][:len(want["first_ids"])]
            check(name + " first ids", got == want["first_ids"], got)
        if "total" in want:
            check(name + " total", len(b.data["tasks"]) == want["total"], len(b.data["tasks"]))
        nexts = b.nexts()
        for tid in want.get("nexts_has") or []:
            check(f"{name} next {tid}", tid in nexts, nexts)
        for tid in want.get("nexts_not") or []:
            check(f"{name} not next {tid}", tid not in nexts, nexts)
        text = out["text"] if isinstance(out, dict) and "text" in out else out if isinstance(out, str) else ""
        for part in want.get("text_has") or []:
            check(f"{name} has {part!r}", part in text, text)
        for part in want.get("text_lacks") or []:
            check(f"{name} lacks {part!r}", part not in text, text)

async def main():
    for s in CASES["scenarios"]:
        await run(s)

asyncio.run(main())
print(f"{sum(res)}/{len(res)}")
