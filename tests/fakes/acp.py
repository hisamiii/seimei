#!/usr/bin/env python3
"""A fake ACP agent (agent-client-protocol v1, JSON-RPC over stdio) for seimei's ACP client.
It never reaches a model: each prompt plays the script its "!command" line picks (see turn()), and what no script takes is echoed back.
Sessions live in $FAKE_ACP_DIR/sessions/<id>.json; session/load replays one. $FAKE_ACP_NO_LOAD turns loadSession off, $FAKE_ACP_RESUME turns session/resume on (no replay), and $FAKE_ACP_EARLY_MCP names a stdio MCP server whose tools it lists while it opens a session, as Codex does (!early says what it got).
Every message both ways goes to $FAKE_ACP_LOG as JSON lines {"dir": "in" or "out", "pid": ..., "msg": ...}, so a test can check them against the protocol's types."""
import json
import os
import queue
import subprocess
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
DSH_OPTIONS = [{"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"},
               {"optionId": "reject-once", "name": "Reject", "kind": "reject_once"}]

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
    elif cmd == "!slow":
        say(sid, "First half, ")
        time.sleep(1)
        say(sid, "second half.")
    elif cmd == "!short":
        update(sid, {"sessionUpdate": "plan", "entries": [
            {"content": "Only this", "priority": "medium", "status": "in_progress"}]})
        say(sid, "Shorter.")
    elif cmd == "!plan":
        steps = ["Read the code", "Change it", "Test it"]
        update(sid, {"sessionUpdate": "plan", "entries": [
            {"content": s, "priority": "medium", "status": "pending"} for s in steps]})
        update(sid, {"sessionUpdate": "plan", "entries": [
            {"content": s, "priority": "medium", "status": st}
            for s, st in zip(steps, ["completed", "in_progress", "pending"])]})
        say(sid, "Planned.")
    elif cmd in ("!tool", "!dsh", "!dshcall"):
        if cmd == "!dshcall":
            title, _, raw = rest.partition(" ")
            call = {"toolCallId": "call_1", "title": title, "kind": "other", "status": "in_progress",
                    "rawInput": json.loads(raw or "{}")}
            asked, options = {"toolCallId": "call_1"}, DSH_OPTIONS
        elif cmd == "!tool":
            call = {"toolCallId": "call_1", "title": f"Run {rest}", "kind": "execute", "status": "pending",
                    "rawInput": {"command": rest}}
            asked, options = call, OPTIONS
        else:  # as dsh sends it: kind other, the tool's name as title, a request that names only the id
            call = {"toolCallId": "call_1", "title": "bash", "kind": "other", "status": "in_progress",
                    "rawInput": {"command": rest, "description": "a command"}}
            asked, options = {"toolCallId": "call_1"}, DSH_OPTIONS
        update(sid, {"sessionUpdate": "tool_call", **call})
        r = ask("session/request_permission", {"sessionId": sid, "toolCall": asked, "options": options})
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
    elif cmd == "!unasked":
        # Runs a command without asking, then goes on for a second unless it is cancelled.
        update(sid, {"sessionUpdate": "tool_call", "toolCallId": "call_u", "title": f"Run {rest}", "kind": "execute",
                     "status": "in_progress", "rawInput": {"command": rest}})
        update(sid, {"sessionUpdate": "tool_call_update", "toolCallId": "call_u", "status": "completed",
                     "content": [{"type": "content", "content": {"type": "text", "text": f"ran {rest}"}}]})
        say(sid, "Ran it unasked.")
        for _ in range(20):
            if sid in cancelled:
                return "cancelled"
            time.sleep(0.05)
        say(sid, "Went on.")
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
    elif cmd == "!call":
        server, tool, args = rest.split(" ", 2)
        say(sid, json.dumps(mcp_call(sid, server, "tools/call", {"name": tool, "arguments": json.loads(args)})))
    elif cmd == "!early":
        say(sid, json.dumps(sessions[sid].get("early")))
    elif cmd == "!fs":
        r = ask("fs/read_text_file", {"sessionId": sid, "path": "/etc/hostname"})
        say(sid, json.dumps(r.get("error") or r.get("result")))
    elif cmd == "!mode":
        say(sid, f"mode {sessions[sid]['mode']}")
    else:
        say(sid, f"You said: {text}")
    return "end_turn"


def mcp_call(sid: str, server: str, method: str, params: dict):
    """One request on one of the session's stdio MCP servers, started as an agent starts it."""
    m = next((m for m in sessions[sid]["mcp"] if m.get("name") == server), None)
    if m is None:
        return {"error": f"no server {server}"}
    env = dict(os.environ, **{e["name"]: e["value"] for e in m.get("env") or []})
    p = subprocess.Popen([m["command"], *(m.get("args") or [])], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         env=env, text=True)

    def rpc(i, method, params):
        p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": i, "method": method, "params": params}) + "\n")
        p.stdin.flush()
        for line in p.stdout:
            msg = json.loads(line)
            if msg.get("id") == i:
                return msg
        return {"error": "the server exited"}

    try:
        rpc(1, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                              "clientInfo": {"name": "fake-acp", "version": "0.1.0"}})
        p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        r = rpc(2, method, params)
        return r.get("result") or r.get("error")
    finally:
        p.stdin.close()
        p.wait(timeout=10)


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
        if os.environ.get("FAKE_ACP_RESUME"):
            caps["sessionCapabilities"] = {"resume": {}}
        send({"id": i, "result": {"protocolVersion": 1, "agentCapabilities": caps, "authMethods": [],
                                  "agentInfo": {"name": "fake-acp", "version": "0.1.0"}}})
    elif method in ("session/new", "session/load", "session/resume"):
        if not Path(p.get("cwd", "")).is_dir():
            send({"id": i, "error": {"code": -32602, "message": f"no folder {p.get('cwd')}"}})
            return
        sid = p.get("sessionId") or str(uuid.uuid4())
        history = []
        if method != "session/new":
            f = DIR / "sessions" / f"{sid}.json"
            if not f.exists():
                send({"id": i, "error": {"code": -32002, "message": f"no session {sid}"}})
                return
            if method == "session/load":
                history = json.loads(f.read_text())["history"]
        sessions[sid] = {"cwd": p["cwd"], "mcp": p.get("mcpServers") or [], "mode": "ask", "history": []}
        if os.environ.get("FAKE_ACP_EARLY_MCP"):
            sessions[sid]["early"] = mcp_call(sid, os.environ["FAKE_ACP_EARLY_MCP"], "tools/list", {})
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
