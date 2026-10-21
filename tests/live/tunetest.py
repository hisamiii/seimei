"""#74 live: tune a running shikigami's effort and model without restarting its task (real Pool, sonnet then haiku,
scratch folder). Mid-turn: the model switches at once, the effort waits for the turn's end; the /effort command's own
result is no report; idle: the effort applies at once. The transcript shows each turn's effort and model."""
import os; os.environ["CLAUDE_CODE_DISABLE_AUTO_MEMORY"] = "1"  # test sessions never write the user's memories
import asyncio, json, shutil, sys, tempfile, time
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
for k in [k for k in os.environ if k.startswith("CLAUDE") and k != "CLAUDE_CODE_DISABLE_AUTO_MEMORY"]:
    os.environ.pop(k)
from seimei import workers as W  # noqa: E402
from seimei import metrics  # noqa: E402
from seimei.approvals import Approvals  # noqa: E402
from seimei.events import EventLog  # noqa: E402
from seimei.guard import Guard  # noqa: E402

WORK = Path(__import__("tempfile").gettempdir() + "/seimei-live/t74/live")
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:600], flush=True)

class Disp:
    busy = False
    lines = []
    def wake(self, t): self.lines.append(t)
    def note(self, t, key=None): self.lines.append(t)
    def session_id(self): return None
    def rate_limit(self, *a, **k): pass
    usage = None

async def until(f, t=180, step=0.5):
    end = time.time() + t
    while time.time() < end:
        if v := f():
            return v
        await asyncio.sleep(step)

def turns(w):
    f = Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude") / "projects" / str(WORK).replace("/", "-") / f"{w.rec['session_id']}.jsonl"
    out = []
    for l in f.read_text().splitlines():
        r = json.loads(l)
        if r.get("type") == "assistant" and not r.get("isSidechain"):
            out.append((r.get("effort"), (r.get("message") or {}).get("model", "")))
    return out

async def main():
    if WORK.exists():
        shutil.rmtree(WORK)
    WORK.mkdir(parents=True)
    state = Path(tempfile.mkdtemp(prefix="tunetest-"))
    ev = EventLog(state / "events.jsonl")
    pool = W.Pool(ev, state)
    pool.dispatcher = Disp()
    pool.guard = Guard(ev, Approvals(ev), state)
    pool.mode = "bypassPermissions"
    reports = []
    real = pool.report
    pool.report = lambda w, e, t: (reports.append((e, t)), real(w, e, t))

    w = await pool.summon("tune", "Run the bash command `sleep 12`, then reply with just the word one.", str(WORK), "sonnet", "low")
    await until(lambda: any(x[0] == "low" for x in turns(w)) if (Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude") / "projects" / str(WORK).replace("/", "-") / f"{w.rec['session_id']}.jsonl").exists() else None, 60)
    check("running", w.rec["state"] == "running", w.rec["state"])
    changed = await w.tune("high", "haiku")
    check("tune mid-turn says what changed", changed == ["model sonnet → haiku", "effort low → high"], changed)
    check("the effort waits for the turn's end", w.rec.get("effort_next") == "high" and w.rec.get("effort") == "low", w.rec)
    await until(lambda: w.rec["state"] == "done", 120)
    await until(lambda: not w.rec.get("effort_next") and w.rec.get("effort") == "high", 30)
    await asyncio.sleep(3)
    check("after the turn the effort applied", w.rec.get("effort") == "high" and not w.rec.get("effort_next"), w.rec)
    check("one done report, none for /effort", [e for e, _ in reports] == ["done"], reports)
    check("still done (the /effort result is not a turn)", w.rec["state"] == "done", w.rec["state"])

    await w.send("Reply with just the word two.")
    await until(lambda: w.rec["state"] == "done" and len(reports) == 2, 120)
    t = turns(w)
    print("turns (effort, model):", t)
    check("the next turn ran on haiku at high", t[-1][0] == "high" and "haiku" in t[-1][1], t)
    check("the first turn ran on sonnet at low", t[0][0] == "low" and "sonnet" in t[0][1], t)

    # Idle: applies at once.
    await w.tune("medium")
    await asyncio.sleep(3)
    check("idle: applied at once", w.rec.get("effort") == "medium" and not w.rec.get("effort_next"), w.rec)
    check("no report from the idle /effort", len(reports) == 2, reports)
    await w.send("Reply with just the word three.")
    await until(lambda: len(reports) == 3, 120)
    check("the next turn ran at medium", turns(w)[-1][0] == "medium", turns(w))

    m = metrics.worker(w.rec, w.log.before(None, 1000), [], None)
    check("metrics keep the changes", [x["what"] for x in m["tunes"]] == ["model sonnet → haiku, effort low → high", "effort high → medium"], m["tunes"])
    check("its log says so", sum(1 for e in w.log.before(None, 1000) if e.get("type") == "system" and "by the user" in e.get("text", "")) == 2)
    try:
        await w.tune("huge")
        check("a bad effort is refused", False)
    except ValueError:
        check("a bad effort is refused", True)
    await pool.close()
    print(f"{sum(res)}/{len(res)} passed")

asyncio.run(main())
