"""Seeds the QA server's state before start.sh launches it (either implementation): workers, chat history, usage,
the board (--board), the login (--login), and the user's own sessions as `sleep` stand-ins with fake kitty windows.
Everything lives under tests/qa; nothing here reaches a real Claude session, kitty window or phone."""
import json, os, subprocess, sys, time, uuid
from pathlib import Path

QA = Path(__file__).resolve().parent
HOME = str(Path.home())
EMPTY = "--empty" in sys.argv
STATE_ROOT = QA / ("data-empty" if EMPTY else "data") / "seimei"
FAKE = QA / ("fakeclaude-empty" if EMPTY else "fakeclaude")
RUN = QA / "run"
FRESH = "--keep" not in sys.argv
BOARD = "--board" in sys.argv
# The user's sessions type through this fake kitty (tests/fakes/bin/kitten takes only this password).
KITTY_SOCK = RUN / "kitty.sock"
KITTY_PASSWORD = "qa-kitty"

def ts(off):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() - off))

def seed():
    if not FRESH:
        return
    import shutil
    for p in (STATE_ROOT.parent, FAKE):  # both are this server's own scratch dirs under tests/qa
        if p.exists():
            shutil.rmtree(p)
    state = STATE_ROOT / "dev"
    (state / "workers").mkdir(parents=True)
    (FAKE / "sessions").mkdir(parents=True)
    RUN.mkdir(exist_ok=True)
    KITTY_SOCK.touch()
    (STATE_ROOT / "kitty-rc.conf").write_text(f'remote_control_password "{KITTY_PASSWORD}" ls send-text send-key get-text\n')
    # The guard's rules: the example's, all on (the Rust server reads them; the Python one has its rules built in).
    example = (QA.parent.parent / "contrib/guard.example.toml").read_text().splitlines()
    on = [ln[1:] if ln.startswith("#") and ln[1:2] not in ("", " ") else ln for ln in example]
    (QA / "cfg/seimei/guard.toml").write_text("\n".join(on) + "\n")
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
    def worker(name, st, cwd=f"{HOME}/src/seimei", model="opus", summary="", question=None, age=600, tokens=None, evs=(), wid=None):
        wid = wid or uuid.uuid4().hex[:8]
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
           tokens=380000, age=60, wid="3905ef87")  # the default board's #55 runs on it: no drift line
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
        # A session in a kitty window carries that window in its environment; "noreach" ones run outside kitty.
        kitty = {} if "noreach" in name else {"KITTY_WINDOW_ID": str(len(name)), "KITTY_PUBLIC_KEY": "qa",
                                               "KITTY_LISTEN_ON": f"unix:{KITTY_SOCK}"}
        with open(QA / "server.log", "a") as log:
            p = subprocess.Popen(["sleep", "7200"], stdout=log, stderr=log, stdin=subprocess.DEVNULL,
                                 env={"PATH": os.environ.get("PATH", "/usr/bin"), **kitty}, start_new_session=True)
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
    session("yesterdays-notes", "idle", [prompt, said], cwd=f"{HOME}/src/tmc", idle=15 * 3600)

seed()
if FRESH:
    # The board as it stood on 2026-10-21 (the board cases' fixture): new boards start empty.
    b = json.loads((Path(__file__).resolve().parents[1] / "cases/tasks.json").read_text())["seed"]
    for t in b["tasks"]:  # the fixture's fixed times, moved to now
        t["done_at"] = t["done_at"] and time.time() - (t["created"] - t["done_at"])
        t["created"] = t["updated"] = time.time()
    if BOARD:
        # #58: every state, a queue and steps, for the Tasks lane tests.
        byid = {t["id"]: t for t in b["tasks"]}
        byid["#55"]["steps"] = [{"id": "1", "title": "Read the board code", "state": "completed"},
                              {"id": "2", "title": "Follow worker states", "state": "in_progress"},
                              {"id": "3", "title": "Mirror the steps", "state": "pending"}]
        byid["t4"]["state"] = "asks"
        now = time.time()
        for i, (tid, title, st, steps) in enumerate([
                ("#56", "Drift line", "review", [{"id": "1", "title": "Write it", "state": "completed"}, {"id": "2", "title": "Test it", "state": "completed"}]),
                ("#57", "Queue order", "open", []), ("#59", "After the queue", "open", [])]):
            b["tasks"].append({"id": tid, "title": title, "project": "seimei", "owner": "dev", "state": st, "done_at": None,
                               "worker": None, "steps": steps, "since": now - 600, "created": now + i, "updated": now})
    (STATE_ROOT / "dev" / "tasks.json").write_text(json.dumps(b))


if "--login" in sys.argv and FRESH:
    from seimei.auth import Auth
    Auth(STATE_ROOT / "dev", "http://localhost:8431").set_password("qa-password-123")
