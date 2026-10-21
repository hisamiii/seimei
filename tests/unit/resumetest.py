"""Auto-resume after a restart: running workers get a continue (or their task if never started), asking/laptop wait,
a crash loop is capped, one chat line and one dispatcher line list them."""
import asyncio, json, tempfile, time
from pathlib import Path
import seimei.workers as W
from seimei.events import EventLog

res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, x, flush=True)

async def main():
    st = Path(tempfile.mkdtemp())
    now = time.time()
    recs = [
        {"id": "r1", "name": "runner", "state": "running", "started": True, "task": "t1", "cwd": "/tmp", "model": "haiku", "session_id": "s1"},
        {"id": "r2", "name": "fresh", "state": "running", "started": False, "task": "do the thing", "cwd": "/tmp", "model": "haiku", "session_id": "s2"},
        {"id": "r3", "name": "asker", "state": "asking", "started": True, "task": "t3", "cwd": "/tmp", "model": "haiku", "session_id": "s3"},
        {"id": "r4", "name": "onlap", "state": "laptop", "started": True, "task": "t4", "cwd": "/tmp", "model": "haiku", "session_id": "s4"},
        {"id": "r5", "name": "looper", "state": "running", "started": True, "task": "t5", "cwd": "/tmp", "model": "haiku", "session_id": "s5",
         "resumes": [now - 100, now - 200, now - 300]},
        {"id": "r6", "name": "broken", "state": "running", "started": True, "task": "t6", "cwd": "/tmp", "model": "haiku", "session_id": "s6"},
    ]
    (st / "workers.json").write_text(json.dumps(recs))
    events = EventLog(st / "events.jsonl")
    pool = W.Pool(events, st)
    sent, told = [], []
    async def fake_send(self, text, shown=None, files=None):
        if self.name == "broken":
            raise RuntimeError("no session")
        sent.append((self.name, text, shown))
        self._set("running")
    W.Worker.send = fake_send
    pool.notify = lambda line, urgent=False, key=None: told.append(line)
    pool.report = (lambda orig: lambda w, ev, text: (told.append(f"report {w.name} {ev}"), orig(w, ev, text)))(pool.report)
    pool.tell = lambda *a, **k: None
    resumed = pool.startup()
    await asyncio.sleep(0.2)
    st_ = {w.name: w.rec["state"] for w in pool.workers.values()}
    check("resumed list", resumed == ["runner", "fresh", "broken"], resumed)
    check("started worker got continue", ("runner", W.RESUME, None) in sent, sent)
    check("never-started worker got its task", any(n == "fresh" and t == "do the thing" for n, t, _ in sent), sent)
    check("asking and laptop untouched", st_["asker"] == "asking" and st_["onlap"] == "laptop" and not any(n in ("asker", "onlap") for n, _, _ in sent), st_)
    check("crash loop capped: looper interrupted, not resumed", st_["looper"] == "interrupted" and "report looper interrupted" in told, (st_, told))
    check("failed resume falls back to interrupted", st_["broken"] == "interrupted" and "report broken interrupted" in told, (st_, told))
    check("running ones stay running", st_["runner"] == "running" and st_["fresh"] == "running", st_)
    lines = [l for l in told if l.startswith("seimei restarted")]
    check("one dispatcher line listing the ones that went on", len(lines) == 1 and "runner, fresh" in lines[0] and "broken" not in lines[0], lines)
    evs = [json.loads(l) for l in open(st / "events.jsonl")]
    sysl = [e for e in evs if e["type"] == "system" and e["text"].startswith("Resumed after the restart")]
    check("one chat line", len(sysl) == 1 and sysl[0]["text"] == "Resumed after the restart: runner, fresh.", sysl)
    check("resume times recorded", len(pool.workers["r1"].rec["resumes"]) == 1)
    # A second restart soon after still resumes runner (2 of 3), a fourth start within the hour does not.
    for _ in range(2):
        pool.workers["r1"].rec["state"] = "running"; pool.startup(); await asyncio.sleep(0.05)
    pool.workers["r1"].rec["state"] = "running"
    again = pool.startup(); await asyncio.sleep(0.05)
    check("fourth restart in the hour: capped", "runner" not in again and pool.workers["r1"].rec["state"] == "interrupted", again)
    print(f"{sum(res)}/{len(res)} passed")

asyncio.run(main())
