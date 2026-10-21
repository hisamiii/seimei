"""QA dev server: port 8431, own state, fake ~/.claude, fake dispatcher + workers, kitty stubbed.
Nothing here reaches a real Claude session, kitty window or phone."""
import asyncio, json, os, subprocess, sys, time, uuid
from pathlib import Path

QA = Path(__file__).resolve().parent
HOME = str(Path.home())
EMPTY = "--empty" in sys.argv
STATE_ROOT = QA / ("data-empty" if EMPTY else "data")
FAKE = QA / ("fakeclaude-empty" if EMPTY else "fakeclaude")
RUN = QA / "run"
FRESH = "--keep" not in sys.argv
BOARD = "--board" in sys.argv
if BOARD:
    sys.argv.remove("--board")

import seimei.__main__ as m
import seimei.sessions as S
import seimei.kitty as K
import seimei.link as L
try:  # #61: the connectors moved into plugins
    import seimei.plugins.browser.connector as B
    import seimei.plugins.discord.connector as D
except ImportError:
    import seimei.browser as B
    import seimei.discord as D
import seimei.workers as W
import seimei.dispatcher as DP
import seimei.push as PU

async def _fake_fcm(self, device_token, data, ttl):
    with (QA / "pushes.log").open("a") as f:
        f.write(json.dumps({"t": time.time(), "data": data, "ttl": ttl}) + "\n")
    return "qa-fake"
PU.Fcm.send = _fake_fcm

m.CONFIG = QA / "cfg/seimei/config.toml"
m.STATE = STATE_ROOT
S.CLAUDE = FAKE
import seimei.cost as CO
CO.Cost.__init__.__defaults__ = (FAKE / "projects",)
RUN.mkdir(exist_ok=True)
B.runtime_path = D.runtime_path = lambda name, dev: RUN / f"{name}.sock"

# kitty: pretend reachable for sessions whose name has no "noreach"; typing just logs.
TYPED = QA / "typed.log"
def _reach(pid):
    try:
        rec = next(json.loads(f.read_text()) for f in (FAKE / "sessions").glob("*.json") if json.loads(f.read_text())["pid"] == pid)
    except StopIteration:
        return "The session is gone."
    return "This session does not run in a kitty window." if "noreach" in rec["name"] else None
K.reachable = _reach
async def _typed(pid, *a):
    with TYPED.open("a") as f:
        f.write(json.dumps({"pid": pid, "args": a}) + "\n")
K.type_message = _typed
K.answer = _typed
K.answer_text = _typed
def _no(*a, **k):
    raise K.Unreachable("stubbed")
K.screen_sync = _no
K.window = _no

def ts(off):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() - off))

def seed():
    if not FRESH:
        return
    import shutil
    for p in (STATE_ROOT, FAKE):  # both are this server's own scratch dirs under tests/qa
        if p.exists():
            shutil.rmtree(p)
    state = STATE_ROOT / "dev"
    (state / "workers").mkdir(parents=True)
    (FAKE / "sessions").mkdir(parents=True)
    if EMPTY:
        return
    now = time.time()
    # Usage windows
    (state / "usage.json").write_text(json.dumps({"windows": {
        "five_hour": {"utilization": 0.82, "resets_at": int(now + 5400)},
        "seven_day": {"utilization": 0.37, "resets_at": int(now + 3 * 86400)},
        "seven_day_opus": {"utilization": 0.12, "resets_at": int(now + 3 * 86400)}},
        "warned": {}, "seen": {}, "status": "allowed", "updated": now - 120,
        "today": {"date": time.strftime("%Y-%m-%d"), "sessions": {
            "seimei": {"turns": 41, "input": 9120, "output": 220000, "cache_read": 31000000, "cache_write": 900000},
            "shikigami app-tester": {"turns": 3, "input": 800, "output": 12000, "cache_read": 2000000, "cache_write": 90000}}}}))
    # Workers
    recs = []
    def worker(name, st, cwd=f"{HOME}/src/seimei", model="opus", summary="", question=None, age=600, tokens=None, evs=()):
        wid = uuid.uuid4().hex[:8]
        sid = str(uuid.uuid4())
        r = {"id": wid, "name": name, "task": f"Task for {name}: do the thing carefully and report back.\nSecond line of task.",
             "cwd": cwd, "model": model, "effort": "high", "context_cap": "400k", "subagents": "haiku", "session_id": sid,
             "state": st, "created": now - age - 300, "updated": now - age, "summary": summary, "started": bool(tokens)}
        if question:
            r["question"] = question
        recs.append(r)
        if tokens:
            proj = FAKE / "projects" / cwd.replace("/", "-").replace(".", "-")
            proj.mkdir(parents=True, exist_ok=True)
            (proj / f"{sid}.jsonl").write_text(json.dumps({"type": "assistant", "timestamp": ts(age), "message": {
                "model": "claude-opus-5-5", "id": f"msg_{wid}", "content": [{"type": "text", "text": summary or "working"}],
                "usage": {"input_tokens": tokens, "output_tokens": 100}}}) + "\n")
        log = state / "workers" / f"{wid}.jsonl"
        rows = [{"type": "user", "text": r["task"]}] + list(evs)
        log.write_text("".join(json.dumps({**e, "id": i + 1, "t": now - age - 300 + i * 5}) + "\n" for i, e in enumerate(rows)))
        return r
    md = "Done. Summary:\n\n- **3 bugs** fixed in `chat.js`\n- added a test\n\n```bash\nuv run pytest -q tests/test_chat.py::test_optimistic_send_reconciles_with_server_echo\n```"
    worker("app-tester", "done", summary="QA report written to tmp/qa-report.md", tokens=120000,
           evs=[{"type": "tool", "tool_id": "t1", "name": "Bash", "summary": "uv run python -m seimei --dev"},
                {"type": "tool_result", "tool_id": "t1", "error": False, "text": "dev server up\n" * 30},
                {"type": "assistant", "text": md}])
    worker("asr-bench", "failed", cwd=f"{HOME}/src/asr", summary="GPU open timed out behind claude-queue after 3 retries", tokens=310000,
           evs=[{"type": "tool", "tool_id": "t2", "name": "Bash", "summary": "python bench.py --gpu"},
                {"type": "tool_result", "tool_id": "t2", "error": True, "text": "Traceback (most recent call last):\n  File bench.py, line 3\nRuntimeError: hipErrorOutOfMemory"}])
    worker("wallpaper-grade-final-pass-very-long-name-x", "asking", cwd=f"{HOME}/wallpaper/shards", summary="",
           question="Should the glass plane stay at 1.6x lift (q2), or do you want the darker q1? I rendered both into final/ and the difference is mostly in the top-left corner where the blue shard catches the light.",
           tokens=380000, age=60)
    worker("stopped-one", "stopped", summary="Stopped.", age=3000)
    worker("interrupted-one", "interrupted", summary="Interrupted by a seimei restart.", age=4000)
    for i in range(10):
        worker(f"bulk-{i:02d}", "done", cwd=f"{HOME}/src/proj{i}", model=["opus", "sonnet", "haiku"][i % 3],
               summary=f"Finished bulk job {i} with a summary line that is long enough to wrap onto a second line on a phone",
               age=900 + i * 2000, tokens=20000 * (i + 1))
    # Ended over 12 hours ago: archived on their own (#43).
    worker("prefill-sweep", "done", cwd=f"{HOME}/src/yah", summary="Report: pp2048 1612 ms at matched clock; GPU held 41 min", age=2 * 86400)
    worker("kv-gate-run", "done", cwd=f"{HOME}/src/yah", summary="Report: long-context gate passed; GPU free after 3 min", age=3 * 86400)
    worker("old-render", "failed", cwd=f"{HOME}/wallpaper/shards", summary="Blender crashed at frame 9", age=20 * 3600)
    worker("runner", "done", summary="(will be set running live)", tokens=200000, age=30)
    (state / "workers.json").write_text(json.dumps(recs))
    # Chat history
    ev = []
    def add(e, off):
        ev.append({**e, "id": len(ev) + 1, "t": now - off})
    add({"type": "user", "text": "hey, what's on the laptop right now?"}, 7200)
    add({"type": "assistant", "text": "Three things are running: the **ASR bench**, a wallpaper render and your `kitty` session in ~/src/yah."}, 7190)
    add({"type": "worker", "worker": recs[0]["id"], "name": "app-tester", "event": "summoned", "text": recs[0]["task"], "cwd": recs[0]["cwd"],
         "model": "opus", "effort": "high", "context_cap": "400k", "subagents": "haiku"}, 7000)
    add({"type": "tool", "tool_id": "x1", "name": "mcp__seimei__screenshot", "summary": "whole screen"}, 6990)
    add({"type": "tool_result", "tool_id": "x1", "error": False, "text": "ok"}, 6989)
    add({"type": "worker", "worker": recs[0]["id"], "name": "app-tester", "event": "done", "text": md, "lane": "t"}, 6000)
    turn = {"lane": "t", "turn": len(ev) + 1, "why": "app-tester finished"}
    add({"type": "tool", "tool_id": "x2", "name": "mcp__seimei__peek", "summary": "app-tester", **turn}, 5990)
    add({"type": "tool_result", "tool_id": "x2", "error": False, "text": "ok", **turn}, 5989)
    add({"type": "assistant", "text": "app-tester is done: 3 bugs fixed in chat.js and a test added; the run passes.", **turn}, 5980)
    add({"type": "worker", "worker": recs[1]["id"], "name": "asr-bench", "event": "failed", "text": "GPU open timed out behind claude-queue after 3 retries.\n\nLog: /tmp/x.log", "lane": "t"}, 5000)
    add({"type": "system", "level": "warn", "text": "5-hour limit at 80%, resets 23:50", "lane": "t"}, 4000)
    add({"type": "system", "level": "error", "text": "Session stopped (Claude Code exited), reconnecting.", "lane": "t"}, 3990)
    add({"type": "system", "text": "Session is back.", "lane": "t"}, 3980)
    add({"type": "user", "text": "nice. what about the wallpaper?"}, 70)
    asked = len(ev)
    add({"type": "worker", "worker": recs[2]["id"], "name": recs[2]["name"], "event": "asking", "text": recs[2]["question"]}, 60)
    add({"type": "assistant", "text": "It is waiting on you: pick q1 or q2 on the card above.", "lane": "c", "re": asked}, 50)
    (state / "events.jsonl").write_text("".join(json.dumps(e) + "\n" for e in ev))
    # Fake user sessions
    proj = FAKE / "projects" / "-home-q-src-yah"
    proj.mkdir(parents=True, exist_ok=True)
    def session(name, status, rows, waiting=None, cwd=f"{HOME}/src/yah", tokens=None, idle=300):
        p = subprocess.Popen(["sleep", "7200"])
        start = Path(f"/proc/{p.pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
        sid = str(uuid.uuid4())
        if tokens:
            rows = rows[:1] + [{"type": "assistant", "timestamp": ts(500), "message": {"model": "claude-opus-5-5", "content": [], "usage": {"input_tokens": tokens}}}] + rows[1:]
        d = FAKE / "projects" / cwd.replace("/", "-")
        d.mkdir(parents=True, exist_ok=True)
        (d / f"{sid}.jsonl").write_text("\n".join(json.dumps({"uuid": str(uuid.uuid4()), **r}) for r in [{"type": "x"}] + rows) + "\n")
        rec = {"pid": p.pid, "sessionId": sid, "cwd": cwd, "procStart": start, "kind": "interactive", "entrypoint": "cli",
               "name": name, "status": status, "updatedAt": now * 1000, "statusUpdatedAt": (now - idle) * 1000}
        if waiting:
            rec["waitingFor"] = waiting
        (FAKE / "sessions" / f"{p.pid}.json").write_text(json.dumps(rec))
    prompt = {"type": "user", "message": {"role": "user", "content": "profile the prefill GEMMs and tell me what's left"}, "timestamp": ts(900)}
    said = {"type": "assistant", "message": {"content": [{"type": "text", "text": "## Plan\n\n1. Run `prof/crun.py`\n2. Compare at matched clock\n\n| kernel | ms |\n|---|---|\n| qkv | 12.1 |\n| ffn | 40.3 |"}]}, "timestamp": ts(800)}
    ask = {"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_ask1", "name": "AskUserQuestion", "input": {"questions": [{
        "question": "Which variant should I keep for the next round?", "header": "Variant", "multiSelect": False,
        "options": [{"label": "KT2 32-key tiles", "description": "faster at 32K but +3 VGPRs"}, {"label": "Baseline", "description": "known good"},
                    {"label": "Both, A/B tonight", "description": "costs one more thermal soak"}]}]}}]}, "timestamp": ts(700)}
    bash = {"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_b1", "name": "Bash", "input": {"command": "python prof/crun.py --pp 2048 --rounds 1"}}]}, "timestamp": ts(120)}
    session("prefill-tuning", "busy", [prompt, said, bash], tokens=600000)
    session("kv-gate", "waiting", [prompt, said, ask], waiting="AskUserQuestion", tokens=150000)
    session("noreach-terminal", "idle", [prompt, said], tokens=40000)
    session("a-session-with-an-extremely-long-name-that-should-truncate-nicely-on-phones", "idle", [prompt], cwd=f"{HOME}/src/seimei")
    session("trust-dialog", "waiting", [prompt], waiting="dialog open")
    session("yesterdays-notes", "idle", [prompt, said], cwd=f"{HOME}/src/tmc", idle=15 * 3600)

seed()
if BOARD and FRESH:
    # #58: a board with every state, a queue and steps, for the Tasks lane tests.
    from seimei import tasks as TK
    b = TK.seed()
    ts = {t["id"]: t for t in b["tasks"]}
    ts["#55"]["steps"] = [{"id": "1", "title": "Read the board code", "state": "completed"},
                          {"id": "2", "title": "Follow worker states", "state": "in_progress"},
                          {"id": "3", "title": "Mirror the steps", "state": "pending"}]
    ts["t4"]["state"] = "asks"
    now = time.time()
    for i, (tid, title, st, steps) in enumerate([
            ("#56", "Drift line", "review", [{"id": "1", "title": "Write it", "state": "completed"}, {"id": "2", "title": "Test it", "state": "completed"}]),
            ("#57", "Queue order", "open", []), ("#59", "After the queue", "open", [])]):
        b["tasks"].append({"id": tid, "title": title, "project": "seimei", "owner": "dev", "state": st, "done_at": None,
                           "worker": None, "steps": steps, "since": now - 600, "created": now + i, "updated": now})
    (STATE_ROOT / "dev" / "tasks.json").write_text(json.dumps(b))


class FakeDispatcher(DP.Dispatcher):
    files = uploads = approvals = None
    task = None

    async def run(self):
        self._set(connected=True)
        while True:
            await asyncio.sleep(3600)

    async def _deliver(self, text, images=(), src=None):
        with (QA / 'delivered.log').open('a') as f:
            f.write(json.dumps(text) + '\n')
        self._set(busy=True)
        if src and src[0] == "user":
            self.events.turn = {"lane": "c", "re": src[1]}
        if self.task and not self.task.done():
            return  # folds into the running turn
        self.task = asyncio.ensure_future(self._respond(text, images))

    async def _event_turn(self):
        """What a shikigami report waking seimei looks like: the report, then a turn of its own in the trajectory."""
        await asyncio.sleep(1.5)
        w = POOL.find("runner")
        self.events.add({"type": "worker", "event": "done", "worker": w.id, "name": w.name, "text": "Rendered all 12 frames."})
        self._set(busy=True)
        self.events.turn = {"lane": "t", "turn": self.events.last_id + 1, "why": "runner finished"}
        self.events.add({"type": "tool", "tool_id": "e1", "name": "mcp__seimei__peek", "summary": "runner"})
        self.events.add({"type": "tool_result", "tool_id": "e1", "error": False, "text": "ok"})
        await self._stream("runner finished the render; all 12 frames are in final/.")
        self.events.turn = {}
        self._set(busy=False)

    def wake(self, text):
        with (QA / 'wakes.log').open('a') as f:
            f.write(text + '\n')

    async def interrupt(self):
        if self.task and not self.task.done():
            self.task.cancel()
            self.events.add({"type": "system", "text": "Stopped."})
            self._set(busy=False)

    async def _stream(self, text, delay=0.02):
        self.events.flash({"type": "thinking"})
        await asyncio.sleep(0.6)
        for i in range(0, len(text), 12):
            self.events.flash({"type": "delta", "text": text[i:i + 12], "lane": self.events.turn.get("lane", "c")})
            await asyncio.sleep(delay)
        self.events.add({"type": "assistant", "text": text})

    async def _respond(self, text, images):
        t = text.lower()
        t = t.split()[0][1:] if t.startswith("!") else ""
        try:
            if t == "md":
                await self._stream(MD)
            elif t == "long":
                await self._stream(LONG, 0.002)
            elif t == "slow":
                await self._stream("Thinking slowly about this. " * 40, 0.4)
            elif t == "img":
                for f, cap in (("wide.png", "a wide render"), ("tall.png", "a tall one"), ("tiny.png", ""), ("photo.jpg", "photo with a caption that is fairly long and descriptive")):
                    name, _ = self.files.save((QA / "img" / f).read_bytes())
                    self.events.add({"type": "image", "file": name, "caption": cap})
                    await asyncio.sleep(0.3)
            elif t == "file":
                for f in ("notes.txt", "blob.bin"):
                    info = self.uploads.add(QA / "img" / f)
                    self.events.add({"type": "file", "file": info["id"], **{k: info[k] for k in ("name", "size", "kind")}, "caption": "here you go"})
            elif t == "tool":
                self.events.add({"type": "tool", "tool_id": "q1", "name": "Bash", "summary": "ls ~/src"})
                await asyncio.sleep(1.5)
                self.events.add({"type": "tool_result", "tool_id": "q1", "error": False, "text": "seimei\nyah\nhrx"})
                self.events.add({"type": "tool", "tool_id": "q2", "name": "Read", "summary": "/etc/shadow"})
                self.events.add({"type": "tool_result", "tool_id": "q2", "error": True, "text": "blocked by guard: credentials"})
                await self._stream("Listed it.")
            elif t == "approve":
                self.events.add({"type": "tool", "tool_id": "q3", "name": "Bash", "summary": "claude stop kv-gate"})
                out = await self.approvals.hold({"action": "command", "target": "stop a session", "detail": "claude stop kv-gate\n" + "x" * 300}, "seimei", None)
                self.events.add({"type": "tool_result", "tool_id": "q3", "error": out != "approved", "text": out})
                await self._stream(f"The card came back {out}.")
            elif t in ("perm", "permseimei"):
                # #64: Claude Code's permission prompts as cards (built the way permissions.callback builds them).
                from seimei import permissions as PM
                from claude_agent_sdk import PermissionUpdate as PU
                from claude_agent_sdk.types import PermissionRuleValue as RV
                w = POOL.find("runner")
                def card(tool, inp, sugg, sub=None):
                    what = PM.describe(tool, inp, None)
                    opts, _ = PM.options(tool, sugg, "default")
                    c = {**what, "tool": tool, "options": opts, **({"sub": sub} if sub else {})}
                    if t == "permseimei":
                        return self.approvals.card(c, "seimei", None, wait=PM.DISPATCHER_WAIT)
                    return self.approvals.card(c, w.name, w.id, wait=None, remind=PM.REMIND)
                edits = PU(type="setMode", mode="acceptEdits", destination="session")
                jobs = [card("Bash", {"command": "uv run pytest -q tests/"},
                             [PU(type="addRules", destination="localSettings", behavior="allow", rules=[RV("Bash", "uv run pytest:*")]),
                              edits, PU(type="addRules", destination="localSettings", behavior="allow", rules=[RV("Bash", "uv run:*")])],
                             "Run the unit tests")]
                if t == "perm":
                    jobs += [card("Edit", {"file_path": f"{HOME}/src/yah/yah/router.py", "old_string": "    cost = dust(seed) * 1.0\n    return cost",
                                           "new_string": "    cost = dust(seed) * weight(seed)\n    return cost"}, [edits]),
                             card("ExitPlanMode", {"plan": "## Fix the flaky router test\n\n1. Seed the RNG in `conftest.py`\n"
                                                   "2. Weight the dust cost per seed in `router.py`\n3. Run the suite three times"}, [])]
                if t == "perm":
                    w.waiting_card("wants to run a command: uv run pytest -q tests/")
                outs = await asyncio.gather(*jobs)
                if t == "perm":
                    w.waiting_card(None)
                await self._stream("Cards came back: " + ", ".join(f"{o[0]} ({o[1]}{', ' + o[2] if o[2] else ''})" for o in outs))
            elif t == "longreport":
                w = POOL.find("runner")
                self.events.add({"type": "worker", "event": "done", "worker": w.id, "name": w.name,
                                 "text": "\n\n".join(f"Paragraph {i}: " + "the report goes on. " * 12 for i in range(8))})
            elif t == "event":
                await self._stream("An event turn follows.")
                asyncio.ensure_future(self._event_turn())
            elif t == "error":
                self.events.add({"type": "system", "level": "error", "text": "Turn failed: overloaded_error"})
            elif t == "ask":
                w = POOL.find("runner")
                w._set("asking", question="Do you want the long version or the short one?")
                POOL.report(w, "asking", "Do you want the long version or the short one?")
                await self._stream("runner asks something.")
            elif t == "run":
                w = POOL.find("runner")
                w._set("running", question=None, doing={"text": "Bash: make -j16 render"})
                w._add({"type": "tool", "tool_id": "r1", "name": "Bash", "summary": "make -j16 render"})
                await self._stream("runner is running again.")
            else:
                await self._stream(f"Echo: {text[:300]}" + (f" ({len(images)} images)" if images else ""))
        except asyncio.CancelledError:
            raise
        finally:
            self.events.turn = {}
            self._set(busy=False)


MD = """# Heading one
## Heading two
### Heading three

Some **bold**, *italic*, ~~strike~~, `inline code`, and a [link](https://example.com). A very long URL: https://example.com/a/very/long/path/that/goes/on/and/on/and/never/breaks/naturally?with=query&and=more

- bullet one
- bullet two with a much longer line that will need to wrap around on a narrow phone screen to see how indentation holds
  - nested bullet
1. first
2. second

> a blockquote with **bold** inside

```python
def very_long_function_name_for_testing(argument_one, argument_two, argument_three, argument_four, argument_five):
    return {"key": argument_one, "other": argument_two}  # a long trailing comment that overflows horizontally
```

| col a | col b | col c | col d | col e |
|---|---|---|---|---|
| 1 | two | three three | four four four | five five five five |
| x | y | z | w | v |

---

Final line with a footnote-ish thing [1] and an emoji-free :3 face.
"""
LONG = "\n\n".join(f"Paragraph {i}: " + "lorem ipsum dolor sit amet consectetur " * 25 for i in range(30))

DP.Dispatcher = FakeDispatcher
m.Dispatcher = FakeDispatcher


async def fake_send(self, text, shown=None, files=None):
    if self.rec["state"] == "released":
        raise RuntimeError(f"{self.name} is the user's own session now")
    # Logs like Worker.send: what the user wrote, files as chips.
    self._add({"type": "user", "text": text if shown is None else shown, **({"files": files} if files else {})})
    self._set("running", question=None, doing={"text": "Thinking"})
    async def later():
        await asyncio.sleep(2)
        self._add({"type": "assistant", "text": f"(fake) got: {text[:200]}"})
        self._set("done", summary=f"replied to: {text[:60]}")
        self.pool.report(self, "done", f"(fake) got: {text[:200]}")
    asyncio.ensure_future(later())

async def fake_stop(self):
    if self.rec["state"] in W.ACTIVE:
        self._set("stopped", summary="Stopped.")
        self.pool.report(self, "stopped", "")

async def fake_hand(self, *a):
    raise RuntimeError("QA stub: no laptop hand-over")


async def fake_to_laptop(self, *a):
    # Release (#59) opens no window in QA; it records what it would have run.
    extra = a[-1] if isinstance(a[-1], str) else "".join(" " + x for x in a[-1])
    with (QA / "released.log").open("a") as f:
        f.write(f"claude-queue {self.name} --resume {self.rec['session_id']}{extra}\n")
    return {"id": 99}
W.Worker._to_laptop = fake_to_laptop

W.Worker.send = fake_send
W.Worker.stop = fake_stop
W.Worker.hand_over = fake_hand

POOL = None
_orig_pool_init = W.Pool.__init__
def pool_init(self, *a, **k):
    global POOL
    _orig_pool_init(self, *a, **k)
    POOL = self
W.Pool.__init__ = pool_init
m.Pool = W.Pool

_orig_make_tools = m.make_tools
def make_tools(pool, files, events, extra, uploads, groups=()):
    FakeDispatcher.files, FakeDispatcher.uploads = files, uploads
    return None
m.make_tools = make_tools

_orig_approvals = m.Approvals
class A(_orig_approvals):
    def __init__(self, *a):
        super().__init__(*a)
        FakeDispatcher.approvals = self
m.Approvals = A

os.environ.pop("CLAUDE_CODE_SESSION_ID", None)
if "--login" in sys.argv:
    from seimei.auth import Auth
    Auth(STATE_ROOT / "dev", "http://localhost:8431").set_password("qa-password-123")
sys.argv = ["seimei", "--dev"] + [a for a in sys.argv[1:] if a not in ("--keep", "--empty")]
m.main()
