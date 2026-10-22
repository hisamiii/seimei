#!/usr/bin/env python3
"""A fake ACP agent (agent-client-protocol v1, JSON-RPC over stdio) for seimei's ACP client. It never reaches a model:
each prompt plays the script its "!command" line picks, and what no script takes is echoed back.

  !think          a thought, then the answer
  !chunks         the answer in three chunks
  !plan           a plan of three steps, then the same plan with the first two done
  !tool <cmd>     an execute tool call that asks permission first; allowed it runs, rejected it fails
  !wait           a turn that runs until the client cancels it (stop reason cancelled)
  !crash          exits mid-turn
  !mcp            tells the MCP servers its session got (names, commands, args, env names)
  !fs             asks the client to read a file and tells what came back
  !mode           tells the session's mode

Sessions live in $FAKE_ACP_DIR/sessions/<id>.json; session/load replays one as updates before it answers.
$FAKE_ACP_NO_LOAD turns the loadSession capability off. Every message both ways goes to $FAKE_ACP_LOG (JSON lines,
{"dir": "in" or "out", "pid": ..., "msg": ...}), so a test can check them against the protocol's types."""
import json
import os
import queue
import sys
import threading
import time
import uuid
from pathlib import Path

DIR = Path(os.environ.get("FAKE_ACP_DIR") or "/tmp/fake-acp")
LOG = os.environ.get("FAKE_ACP_LOG")
MODES = [{"id": "ask", "name": "Ask"}, {"id": "code", "name": "Code"}]
OPTIONS = [{"optionId": "allow-once", "name": "Allow", "kind": "allow_once"},
           {"optionId": "allow-always", "name": "Always allow", "kind": "allow_always"},
           {"optionId": "reject-once", "name": "Reject", "kind": "reject_once"},
           {"optionId": "reject-always", "name": "Always reject", "kind": "reject_always"}]

out_lock = threading.Lock()
waiting: dict = {}  # id of a request to the client -> queue for its answer
cancelled: set = set()  # sessions whose turn the client cancelled
sessions: dict = {}  # id -> {"cwd", "mcp", "mode", "history"}
next_id = [0]


def log(direction: str, msg: dict):
    if LOG:
        with out_lock, open(LOG, "a") as f:
            f.write(json.dumps({"dir": direction, "pid": os.getpid(), "msg": msg}) + "\n")


def send(msg: dict):
    msg["jsonrpc"] = "2.0"
    log("out", msg)
    with out_lock:
        sys.stdout.write(json.dumps(msg) + "\n")
        sys.stdout.flush()


def ask(method: str, params: dict) -> dict:
    """A request to the client; its answer (result or error)."""
    next_id[0] += 1
    i, q = next_id[0], queue.Queue()
    waiting[i] = q
    send({"id": i, "method": method, "params": params})
    return q.get()


def update(sid: str, u: dict):
    send({"method": "session/update", "params": {"sessionId": sid, "update": u}})


def say(sid: str, text: str, kind: str = "agent_message_chunk"):
    update(sid, {"sessionUpdate": kind, "content": {"type": "text", "text": text}})
    if kind != "agent_thought_chunk":
        sessions[sid]["history"].append([kind, text])


def save(sid: str):
    s = sessions[sid]
    (DIR / "sessions").mkdir(parents=True, exist_ok=True)
    (DIR / "sessions" / f"{sid}.json").write_text(json.dumps({"cwd": s["cwd"], "history": s["history"]}))


def turn(sid: str, text: str) -> str:
    """Plays one prompt; its stop reason."""
    cmd, _, rest = next((ln for ln in text.splitlines() if ln.startswith("!")), "").partition(" ")
    if cmd == "!think":
        say(sid, "Thinking it over.", "agent_thought_chunk")
        say(sid, "Done thinking.")
    elif cmd == "!chunks":
        for part in ["one ", "two ", "three"]:
            say(sid, part)
    elif cmd == "!plan":
        steps = ["Read the code", "Change it", "Test it"]
        update(sid, {"sessionUpdate": "plan", "entries": [
            {"content": s, "priority": "medium", "status": "pending"} for s in steps]})
        update(sid, {"sessionUpdate": "plan", "entries": [
            {"content": s, "priority": "medium", "status": st}
            for s, st in zip(steps, ["completed", "in_progress", "pending"])]})
        say(sid, "Planned.")
    elif cmd == "!tool":
        call = {"toolCallId": "call_1", "title": f"Run {rest}", "kind": "execute", "status": "pending",
                "rawInput": {"command": rest}}
        update(sid, {"sessionUpdate": "tool_call", **call})
        r = ask("session/request_permission", {"sessionId": sid, "toolCall": call, "options": OPTIONS})
        outcome = (r.get("result") or {}).get("outcome") or {}
        if outcome.get("outcome") == "cancelled" or sid in cancelled:
            update(sid, {"sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "failed"})
            return "cancelled"
        if outcome.get("optionId", "").startswith("allow"):
            update(sid, {"sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "in_progress"})
            update(sid, {"sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "completed",
                         "content": [{"type": "content", "content": {"type": "text", "text": f"ran {rest}"}}]})
            say(sid, f"Ran it ({outcome['optionId']}).")
        else:
            update(sid, {"sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "failed",
                         "content": [{"type": "content", "content": {"type": "text", "text": "denied"}}]})
            say(sid, f"Did not run it ({outcome.get('optionId') or r.get('error')}).")
    elif cmd == "!wait":
        say(sid, "Working.")
        for _ in range(600):
            if sid in cancelled:
                return "cancelled"
            time.sleep(0.05)
        say(sid, "Gave up waiting.")
    elif cmd == "!crash":
        say(sid, "Going down.")
        sys.stdout.flush()
        os._exit(3)
    elif cmd == "!mcp":
        got = [{"name": m.get("name"), "command": m.get("command"), "args": m.get("args"),
                "env": sorted(e["name"] for e in m.get("env") or [])} for m in sessions[sid]["mcp"]]
        say(sid, json.dumps(got))
    elif cmd == "!fs":
        r = ask("fs/read_text_file", {"sessionId": sid, "path": "/etc/hostname"})
        say(sid, json.dumps(r.get("error") or r.get("result")))
    elif cmd == "!mode":
        say(sid, f"mode {sessions[sid]['mode']}")
    else:
        say(sid, f"You said: {text}")
    return "end_turn"


def prompt(i, p: dict):
    sid = p["sessionId"]
    if sid not in sessions:
        send({"id": i, "error": {"code": -32602, "message": f"unknown session {sid}"}})
        return
    text = "".join(b.get("text", "") for b in p["prompt"] if b.get("type") == "text")
    sessions[sid]["history"].append(["user_message_chunk", text])
    cancelled.discard(sid)
    stop = turn(sid, text)
    save(sid)
    send({"id": i, "result": {"stopReason": stop}})


def handle(msg: dict):
    i, method, p = msg.get("id"), msg.get("method"), msg.get("params") or {}
    if method is None:  # an answer to one of ours
        q = waiting.pop(msg.get("id"), None)
        if q:
            q.put(msg)
        return
    if method == "initialize":
        caps = {"loadSession": not os.environ.get("FAKE_ACP_NO_LOAD"), "promptCapabilities": {"image": True},
                "mcpCapabilities": {"http": False, "sse": False}}
        send({"id": i, "result": {"protocolVersion": 1, "agentCapabilities": caps, "authMethods": [],
                                  "agentInfo": {"name": "fake-acp", "version": "0.1.0"}}})
    elif method in ("session/new", "session/load"):
        if not Path(p.get("cwd", "")).is_dir():
            send({"id": i, "error": {"code": -32602, "message": f"no folder {p.get('cwd')}"}})
            return
        sid = p.get("sessionId") or str(uuid.uuid4())
        history = []
        if method == "session/load":
            f = DIR / "sessions" / f"{sid}.json"
            if not f.exists():
                send({"id": i, "error": {"code": -32002, "message": f"no session {sid}"}})
                return
            history = json.loads(f.read_text())["history"]
        sessions[sid] = {"cwd": p["cwd"], "mcp": p.get("mcpServers") or [], "mode": "ask", "history": []}
        for kind, text in history:
            say(sid, text, kind)
        res = {"modes": {"currentModeId": "ask", "availableModes": MODES}}
        if method == "session/new":
            res["sessionId"] = sid
            save(sid)
        send({"id": i, "result": res})
    elif method == "session/prompt":
        threading.Thread(target=prompt, args=(i, p), daemon=True).start()
    elif method == "session/cancel":
        cancelled.add(p.get("sessionId"))
    elif method == "session/set_mode":
        sid, mode = p.get("sessionId"), p.get("modeId")
        if mode not in [m["id"] for m in MODES]:
            send({"id": i, "error": {"code": -32602, "message": f"no mode {mode}"}})
            return
        sessions[sid]["mode"] = mode
        update(sid, {"sessionUpdate": "current_mode_update", "currentModeId": mode})
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
