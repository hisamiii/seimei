"""#47 metrics rules: follow-ups after the first done (resume notes not), who sent them, reopened within 24 h unless a
bare thanks, asks, busy vs wall time, spend from cost."""
from seimei import metrics as M
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else x)
T = 1_000_000.0
rec = {"id": "w", "name": "x", "model": "opus", "effort": "high", "context_cap": "400k", "cwd": "/tmp", "state": "done",
       "created": T, "updated": T + 4000}
ev = lambda e, t, **k: {"type": "worker", "event": e, "t": t, **k}
log = [{"type": "user", "text": "the task", "t": T},
       {"type": "user", "text": "Thanks!", "t": T + 1100},
       {"type": "user", "text": "continue: seimei restarted, check your state first", "t": T + 1200},
       {"type": "user", "text": "now also do Y", "t": T + 2000},
       {"type": "user", "text": "ok", "t": T + 3600}]
events = [ev("asking", T + 300), ev("done", T + 1000), ev("replied", T + 2001), ev("done", T + 2500), ev("done", T + 3700)]
m = M.worker(rec, log, events, {"input": 5, "cache_read": 10, "cache_write": 2, "output": 3, "usd": 1.23456, "calls": 4}, now=T + 9999)
check("follow-ups skip the task and resume notes", m["followups"] == 3, m)
check("who: a reply event marks the user's", m["followups_by"] == {"seimei": 2, "user": 1}, m["followups_by"])
check("reopened by a real request", m["reopened"])
check("asks counted", m["asks"] == 1)
check("wall: to the last end", m["wall"] == 3700, m["wall"])
# busy: task T..ask 300; thanks 1100..done? (no pause before 2500 except none) -> 1100..2500 incl. resume; 3600..3700
check("busy: only stretches of work", abs(m["busy"] - (300 + 1400 + 100)) < 1e-6, m["busy"])
check("tokens and $ from cost", m["tokens"] == {"input": 5, "cache_read": 10, "cache_write": 2, "output": 3} and m["usd"] == 1.2346, m)
m2 = M.worker(rec, log[:3] + log[4:], events, None, now=T + 9999)
check("a bare thanks/ok is not reopened", not m2["reopened"] and m2["followups"] == 2, m2)
late = log[:1] + [{"type": "user", "text": "one more thing", "t": T + 1000 + 25 * 3600}]
check("past 24 h is not reopened", not M.worker(rec, late, events[:2], None, now=T + 2e5)["reopened"])
run = M.worker({**rec, "state": "running"}, log[:1], [], None, now=T + 50)
check("running: wall and busy up to now", run["wall"] == 50 and run["busy"] == 50, run)
txt = M.text([m])
check("the tool text has the totals", "Total $1.23 over 1 shikigami; reopened 1 of 1" in txt and "busy" in txt, txt)
print(f"{sum(res)}/{len(res)} passed")

# The workers tool's metrics option, against a pool with one worker and a stub cost.
import asyncio, json, tempfile, time
from pathlib import Path
from seimei import tools as TL, workers as W
from seimei.events import EventLog
d = Path(tempfile.mkdtemp())
(d / "workers").mkdir()
(d / "workers.json").write_text(json.dumps([{**rec, "id": "ab12", "name": "metric-me", "task": "t", "session_id": "S1", "summary": ""}]))
got = {}
TL.create_sdk_mcp_server = lambda name, tools, **k: got.update({t.name: t for t in tools})
TL.always_load = lambda x, *a: x
pool = W.Pool(EventLog(d / "events.jsonl"), d)
class Cost:
    scanned = time.time()
    def scan(self): pass
    def sessions(self, start): return {"S1": {"input": 1000, "cache_read": 0, "cache_write": 0, "output": 50, "usd": 0.5}}
class Usage: cost = Cost()
class Disp: usage = Usage()
pool.dispatcher = Disp()
TL.make_tools(pool, None, EventLog(d / "ev2.jsonl"), [], None)
out = asyncio.run(got["workers"].handler({"metrics": True, "worker": "metric-me"}))["content"][0]["text"]
check("workers tool: metrics for one by name", "metric-me (ab12)" in out and "$0.50" in out and "1k / 0 / 0 / 50" in out, out)
r = asyncio.run(got["workers"].handler({"metrics": True, "worker": "nope"}))
check("workers tool: unknown name is an error", r.get("is_error"), r)
print(f"{sum(res)}/{len(res)} passed (with tool)")
