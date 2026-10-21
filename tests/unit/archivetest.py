"""#43 archive rules: auto after 12 h, manual hide until the item's own activity, unarchive keeps it 12 h more;
the pool's listing and the dispatcher's workers tool."""
import asyncio, sys, tempfile, time
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from seimei.archive import AUTO, Archive  # noqa: E402
from seimei.events import EventLog  # noqa: E402
from seimei import workers as W  # noqa: E402
from seimei import tools as T  # noqa: E402

res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else x)

d = Path(tempfile.mkdtemp(prefix="archtest-"))
a = Archive(d / "archive.json")
now = time.time()
w = lambda st, age, wid="a": {"id": wid, "state": st, "updated": now - age}
check("ended 11 h ago stays", not a.worker(w("done", 11 * 3600), now))
check("ended 13 h ago archives itself", a.worker(w("failed", 13 * 3600), now))
check("running never archives", not a.worker(w("running", 30 * 3600), now))
check("asking never archives", not a.worker(w("asking", 30 * 3600), now))
a.set("w:b", True)
check("manual archive hides at once", a.worker(w("done", 60, "b")))
check("waking it brings it back", not a.worker({"id": "b", "state": "running", "updated": time.time() + 1}))
check("ended again after waking: still visible", not a.worker({"id": "b", "state": "done", "updated": time.time() + 1}))
a.set("w:c", False)
check("unarchived: 12 more hours", not a.worker(w("done", 40 * 3600, "c")))
check("unarchived: hides again after 12 h", a.worker(w("done", 40 * 3600, "c"), time.time() + AUTO + 5))
check("marks persist", Archive(d / "archive.json").marks.keys() >= {"w:b", "w:c"})

s = lambda st, idle, q=False, sid="s1": {"session_id": sid, "status": st, "since": now - idle, "question": q}
check("idle session 13 h archives itself", a.session(s("idle", 13 * 3600), now))
check("busy session never archives", not a.session(s("busy", 13 * 3600), now))
check("a question brings it back", not a.session(s("idle", 13 * 3600, True), now))
check("waiting on a prompt brings it back", not a.session(s("waiting", 13 * 3600), now))
a.set("s:s2", True)
check("manual session archive hides", a.session(s("idle", 60, sid="s2")))
check("its next turn brings it back", not a.session({"session_id": "s2", "status": "idle", "since": time.time() + 1, "question": False}))


async def pool_part():
    events = EventLog(d / "events.jsonl")
    pool = W.Pool(events, d)
    for wid, st, age in (("w1", "done", 60), ("w2", "done", 14 * 3600), ("w3", "running", 5)):
        pool.workers[wid] = W.Worker(pool, {"id": wid, "name": f"n-{wid}", "task": "t", "cwd": "/tmp", "model": "opus",
                                            "session_id": wid, "state": st, "created": now - age, "updated": now - age, "summary": f"s {wid}"})
    flags = {x["id"]: x["archived"] for x in pool.listing()["list"]}
    check("listing flags the 14 h one", flags == {"w1": False, "w2": True, "w3": False}, flags)
    pool.set_archived(pool.workers["w1"], True)
    check("set_archived flags it", pool.workers["w1"].info()["archived"])
    got = {}
    T.create_sdk_mcp_server = lambda name, tools, **k: got.update({t.name: t for t in tools})
    T.always_load = lambda x, *a: x
    T.make_tools(pool, None, events, [], None)
    tools = got
    out = (await tools["workers"].handler({}))["content"][0]["text"]
    check("workers tool leaves archived out", "n-w3" in out and "n-w1" not in out and "n-w2" not in out, out)
    out = (await tools["workers"].handler({"archived": True}))["content"][0]["text"]
    check("archived: true lists only those", "n-w1" in out and "n-w2" in out and "n-w3" not in out, out)

asyncio.run(pool_part())
print(f"{sum(res)}/{len(res)} passed")
