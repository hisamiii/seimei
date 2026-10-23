#!/usr/bin/env python3
"""A fake `codex app-server` (Codex's JSON-RPC over stdio, the v2 thread/turn/item API) for seimei's codex client. It
never reaches a model: each turn plays the script its "!command" line picks, and what no script takes is echoed back.

  !think          reasoning, then the answer
  !two            two agent messages, the second one whole (no deltas)
  !plan           a plan of three steps, then the same plan with the first two done
  !tool <cmd>     a command that asks for approval first; accepted it runs, declined it is declined
  !edit <path>    a file change that asks for approval first
  !unasked <cmd>  a command run without asking
  !mcp            tells the MCP servers its thread's config got (names, commands, args, env names, approval mode)
  !wait           a turn that runs until it is interrupted
  !fail           a turn that fails
  !crash          exits mid-turn
  !ask            asks for more permissions and for user input, and tells what came back

Threads live in $FAKE_CODEX_DIR/threads/<id>.json. Every message both ways goes to $FAKE_CODEX_LOG (JSON lines,
{"dir": "in" or "out", "msg": ...})."""
import json
import os
import queue
import sys
import threading
import time
import uuid
from pathlib import Path

DIR = Path(os.environ.get("FAKE_CODEX_DIR") or "/tmp/fake-codex")
LOG = os.environ.get("FAKE_CODEX_LOG")

out_lock = threading.Lock()
waiting: dict = {}  # id of a request to the client -> queue for its answer
threads: dict = {}  # id -> {"cwd", "mcp"}
interrupted: set = set()  # turn ids
next_id = [0]
initialized = [False]


def log(direction: str, msg: dict):
    if LOG:
        with out_lock, open(LOG, "a") as f:
            f.write(json.dumps({"dir": direction, "msg": msg}) + "\n")


def send(msg: dict):
    log("out", msg)
    with out_lock:
        sys.stdout.write(json.dumps(msg) + "\n")
        sys.stdout.flush()


def note(method: str, params: dict):
    send({"method": method, "params": params})


def ask(method: str, params: dict) -> dict:
    next_id[0] += 1
    i, q = f"srv-{next_id[0]}", queue.Queue()
    waiting[i] = q
    send({"id": i, "method": method, "params": params})
    return q.get()


class Turn:
    def __init__(self, tid: str, thread: str):
        self.id, self.thread, self.n = tid, thread, 0

    def item_id(self) -> str:
        self.n += 1
        return f"item_{self.n}"

    def p(self, **kw) -> dict:
        return {"threadId": self.thread, "turnId": self.id, **kw}

    def message(self, text: str, deltas: bool = True):
        iid = self.item_id()
        item = {"type": "agentMessage", "id": iid, "text": "", "phase": None, "memoryCitation": None,
                "delivery": None, "questions": None}
        note("item/started", self.p(item=item, startedAtMs=0))
        if deltas:
            for part in [text[: len(text) // 2], text[len(text) // 2:]]:
                note("item/agentMessage/delta", self.p(itemId=iid, delta=part))
        note("item/completed", self.p(item={**item, "text": text}, completedAtMs=0))

    def command(self, cmd: str, asking: bool) -> str:
        iid = self.item_id()
        item = {"type": "commandExecution", "id": iid, "command": cmd, "cwd": threads[self.thread]["cwd"],
                "processId": None, "source": "agent", "status": "inProgress", "commandActions": [],
                "aggregatedOutput": None, "exitCode": None, "durationMs": None}
        if asking:
            r = ask("item/commandExecution/requestApproval",
                    self.p(itemId=iid, startedAtMs=0, environmentId=None, command=cmd, kind="command",
                           cwd=threads[self.thread]["cwd"]))
            decision = (r.get("result") or {}).get("decision")
        else:
            decision = "accept"
        note("item/started", self.p(item=item, startedAtMs=0))
        if decision == "accept":
            note("item/completed", self.p(item={**item, "status": "completed", "aggregatedOutput": f"ran {cmd}",
                                                "exitCode": 0}, completedAtMs=0))
        else:
            note("item/completed", self.p(item={**item, "status": "declined"}, completedAtMs=0))
        return decision

    def edit(self, path: str) -> str:
        iid = self.item_id()
        item = {"type": "fileChange", "id": iid, "status": "inProgress",
                "changes": [{"path": path, "kind": {"type": "add"}, "diff": "+hello\n"}]}
        note("item/started", self.p(item=item, startedAtMs=0))
        r = ask("item/fileChange/requestApproval", self.p(itemId=iid, startedAtMs=0, reason=None, grantRoot=None))
        decision = (r.get("result") or {}).get("decision")
        status = "completed" if decision == "accept" else "declined"
        note("item/completed", self.p(item={**item, "status": status}, completedAtMs=0))
        return decision


def play(t: Turn, text: str) -> tuple:
    """One turn's script; its status and error."""
    cmd, _, rest = next((ln for ln in text.splitlines() if ln.startswith("!")), "").partition(" ")
    if cmd == "!think":
        iid = t.item_id()
        note("item/started", t.p(item={"type": "reasoning", "id": iid, "summary": [], "content": []}, startedAtMs=0))
        note("item/reasoning/summaryTextDelta", t.p(itemId=iid, delta="Thinking it over.", summaryIndex=0))
        t.message("Done thinking.")
    elif cmd == "!two":
        t.message("First.")
        t.message("Second.", deltas=False)
    elif cmd == "!plan":
        steps = ["Read the code", "Change it", "Test it"]
        note("turn/plan/updated", t.p(explanation=None, plan=[{"step": s, "status": "pending"} for s in steps]))
        note("turn/plan/updated", t.p(explanation=None, plan=[
            {"step": s, "status": st} for s, st in zip(steps, ["completed", "inProgress", "pending"])]))
        t.message("Planned.")
    elif cmd == "!tool":
        d = t.command(rest, asking=True)
        if t.id in interrupted or d == "cancel":
            return "interrupted", None
        t.message("Ran it." if d == "accept" else f"Did not run it ({d}).")
    elif cmd == "!edit":
        d = t.edit(rest)
        t.message("Wrote it." if d == "accept" else f"Did not write it ({d}).")
    elif cmd == "!unasked":
        t.command(rest, asking=False)
        t.message("Ran it unasked.")
    elif cmd == "!mcp":
        t.message(json.dumps(threads[t.thread]["mcp"]), deltas=False)
    elif cmd == "!wait":
        t.message("Working.")
        for _ in range(600):
            if t.id in interrupted:
                return "interrupted", None
            time.sleep(0.05)
        t.message("Gave up waiting.")
    elif cmd == "!fail":
        return "failed", {"message": "the model is out of reach", "codexErrorInfo": None, "additionalDetails": None,
                          "misalignment": None}
    elif cmd == "!crash":
        t.message("Going down.")
        os._exit(3)
    elif cmd == "!ask":
        a = ask("item/permissions/requestApproval", t.p(itemId="p1", environmentId=None, startedAtMs=0,
                                                         cwd=threads[t.thread]["cwd"], reason=None,
                                                         permissions={"network": {"enabled": True}}))
        b = ask("item/tool/requestUserInput", t.p(itemId="q1", questions=[], isBlocking=True))
        c = ask("mcpServer/elicitation/request", {"threadId": t.thread, "turnId": t.id, "serverName": "x",
                                                   "mode": "url", "_meta": None, "message": "m", "url": "u",
                                                   "elicitationId": "e"})
        t.message(json.dumps([a.get("result"), b.get("result"), c.get("result")]), deltas=False)
    else:
        t.message(f"You said: {text}")
    return "completed", None


def turn_obj(tid: str, status: str, error=None) -> dict:
    return {"id": tid, "rootTurnId": tid, "items": [], "itemsView": "notLoaded", "status": status, "error": error,
            "startedAt": None, "completedAt": None, "durationMs": None}


def run_turn(t: Turn, text: str):
    note("turn/started", {"threadId": t.thread, "turn": turn_obj(t.id, "inProgress")})
    status, error = play(t, text)
    note("turn/completed", {"threadId": t.thread, "turn": turn_obj(t.id, status, error)})


def thread_obj(tid: str, cwd: str) -> dict:
    return {"id": tid, "sessionId": tid, "cwd": cwd, "turns": [], "cliVersion": "0.0.1", "createdAt": 0,
            "updatedAt": 0, "ephemeral": False, "modelProvider": "fake", "preview": "", "projectId": None,
            "source": "appServer", "status": {"type": "idle"}}


def open_thread(i, p: dict, method: str):
    cwd = p.get("cwd") or os.getcwd()
    tid = p.get("threadId") or str(uuid.uuid4())
    f = DIR / "threads" / f"{tid}.json"
    if method == "thread/resume" and not f.exists():
        send({"id": i, "error": {"code": -32600, "message": f"no rollout found for thread id {tid}"}})
        return
    cfg = p.get("config") or {}
    mcp = [{"name": k.split(".", 1)[1], "command": v.get("command"), "args": v.get("args"),
            "env": sorted((v.get("env") or {}).keys()), "approval": v.get("default_tools_approval_mode")}
           for k, v in sorted(cfg.items()) if k.startswith("mcp_servers.")]
    threads[tid] = {"cwd": cwd, "mcp": mcp}
    (DIR / "threads").mkdir(parents=True, exist_ok=True)
    f.write_text(json.dumps({"cwd": cwd}))
    send({"id": i, "result": {"thread": thread_obj(tid, cwd), "model": "fake-model", "modelProvider": "fake",
                              "cwd": cwd, "approvalPolicy": p.get("approvalPolicy"), "approvalsReviewer": "user",
                              "sandbox": {"type": "workspaceWrite"}, "reasoningEffort": None}})


def handle(msg: dict):
    i, method, p = msg.get("id"), msg.get("method"), msg.get("params") or {}
    if method is None:
        q = waiting.pop(i, None)
        if q:
            q.put(msg)
        return
    if method == "initialize":
        send({"id": i, "result": {"userAgent": "fake-codex/0.0.1", "codexHome": str(DIR),
                                  "platformFamily": "unix", "platformOs": "linux"}})
    elif method == "initialized":
        initialized[0] = True
    elif not initialized[0]:
        send({"id": i, "error": {"code": -32600, "message": "Not initialized"}})
    elif method in ("thread/start", "thread/resume"):
        open_thread(i, p, method)
    elif method == "turn/start":
        tid = p.get("threadId")
        if tid not in threads:
            send({"id": i, "error": {"code": -32600, "message": f"thread not found: {tid}"}})
            return
        text = "".join(x.get("text", "") for x in p.get("input") or [] if x.get("type") == "text")
        t = Turn(str(uuid.uuid4()), tid)
        send({"id": i, "result": {"turn": turn_obj(t.id, "inProgress")}})
        threading.Thread(target=run_turn, args=(t, text), daemon=True).start()
    elif method == "turn/interrupt":
        interrupted.add(p.get("turnId"))
        for q in list(waiting.values()):  # Codex drops its open approvals when a turn is interrupted
            q.put({"result": {"decision": "cancel"}})
        send({"id": i, "result": {}})
    elif i is not None:
        send({"id": i, "error": {"code": -32601, "message": f"Method not found: {method}"}})


def main():
    for line in sys.stdin:
        if not line.strip():
            continue
        msg = json.loads(line)
        log("in", msg)
        handle(msg)


if __name__ == "__main__":
    main()
