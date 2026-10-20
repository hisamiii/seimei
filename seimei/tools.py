"""The dispatcher's own tools, served in-process as the `seimei` MCP server."""

import asyncio
import os
import subprocess
import time
from pathlib import Path

from claude_agent_sdk import create_sdk_mcp_server, tool

from . import queue as cq
from . import sessions as sess
from .events import EventLog
from .files import MAX_BYTES, Files
from .workers import ACTIVE, Pool

MAX_RUNNING = 5


def text(t: str, error: bool = False) -> dict:
    return {"content": [{"type": "text", "text": t}], "is_error": error}


def ago(t: float) -> str:
    s = int(time.time() - t)
    return f"{s // 60} min ago" if s < 3600 else f"{s // 3600} h ago" if s < 86400 else f"{s // 86400} d ago"


def repo_of(path: str) -> str:
    """The checkout a folder belongs to (a git worktree counts as its own), else the folder itself."""
    try:
        r = subprocess.run(["git", "-C", path, "rev-parse", "--show-toplevel"], capture_output=True, text=True, timeout=5)
    except (OSError, subprocess.TimeoutExpired):
        return path
    return r.stdout.strip() if r.returncode == 0 and r.stdout.strip() else path


def writers(pool: Pool, w, own: dict[str, str]) -> str | None:
    """A heads-up when someone else already works in the new worker's checkout: an active shikigami there, or a
    busy session of the user's in that project. Home is no checkout, so a worker in ~ gets none."""
    home = str(Path.home())
    if w.rec["cwd"] == home:
        return None
    root = repo_of(w.rec["cwd"])
    busy = [f"{x.name} (shikigami, {x.rec['state']})" for x in list(pool.workers.values())
            if x.id != w.id and x.rec["state"] in ACTIVE and x.rec["cwd"] != home
            and (x.rec["cwd"] == w.rec["cwd"] or repo_of(x.rec["cwd"]) == root)]
    project = sess.project_of(root + "/x")
    try:
        busy += [f"{s['name']} (the user's session, busy)" for s in sess.listing(own) if s["status"] == "busy"
                 and (s["cwd"] == root or s["cwd"].startswith(root + "/") or (project and s.get("project") == project))]
    except OSError:
        pass
    if not busy:
        return None
    short = root.replace(home, "~", 1)
    return (f"Heads-up: {', '.join(busy)} also {'works' if len(busy) == 1 else 'work'} in {short}. Two writers in one "
            f"checkout can overwrite each other's changes. If both edit files there, stop this one and summon it in "
            f"its own worktree (git -C {short} worktree add ../{Path(root).name}-{w.name}).")


# Tools the dispatcher rarely needs: they stay behind tool search (their names show, the full schema loads on demand).
# Every other seimei tool is loaded up front, so the everyday ones never cost a ToolSearch round trip.
DEFERRED = {"discord_search", "discord_edit", "discord_delete", "discord_react", "discord_mark_read", "browser_read",
            "browser_switch", "browser_download", "browser_downloads", "scroll", "notifications", "send_file",
            "restore_desktop", "unlock_screen"}


def always_load(server: dict, deferred: set[str]) -> dict:
    """Mark a server's tools, except the deferred ones, with the MCP _meta flag Claude Code reads to keep a tool in
    the prompt instead of behind tool search. The SDK's tool() has no field for it, so the listing is wrapped."""
    srv = server["instance"]
    entry = srv.get_request_handler("tools/list")

    async def listed(ctx, params):
        res = await entry.handler(ctx, params)
        for t in res.tools:
            if t.name not in deferred:
                t.meta = {**(t.meta or {}), "anthropic/alwaysLoad": True}
        return res

    srv.add_request_handler("tools/list", entry.params_type, listed)
    return server


def make_tools(pool: Pool, files: Files, events: EventLog, extra: list = (), uploads=None):
    @tool("summon", "Start a shikigami: a worker Claude Code session that works on a task on its own and reports back. "
          "Pick a short lowercase name, the folder the task is about, and a model (opus for real work, sonnet for "
          "routine jobs, haiku for quick lookups). Context is where it compacts: 400k by default, 256k for small jobs, "
          "1m only when the task really needs it. Returns at once; the report arrives later.", {
              "type": "object",
              "properties": {
                  "name": {"type": "string", "description": "short lowercase name, like npu-dma or readme-fix"},
                  "task": {"type": "string", "description": "the full instructions; the worker sees nothing else"},
                  "cwd": {"type": "string", "description": "the project folder the task is about; ~ only when the task is about home"},
                  "model": {"type": "string", "enum": ["opus", "sonnet", "haiku"], "default": "opus"},
                  "effort": {"type": "string", "enum": ["low", "medium", "high", "xhigh", "max"],
                             "description": "reasoning effort; leave out for the user's default"},
                  "context": {"type": "string", "enum": ["256k", "400k", "1m"], "default": "400k"},
                  "subagent_model": {"type": "string", "enum": ["haiku", "sonnet", "opus"], "default": "haiku",
                                     "description": "model for the shikigami's own subagents; haiku unless a rare case needs more"},
                  "confirmed": {"type": "boolean", "description": f"set only after the user OKed more than {MAX_RUNNING} running workers"},
              },
              "required": ["name", "task"]})
    async def summon(args):
        if pool.running() >= MAX_RUNNING and not args.get("confirmed"):
            return text(f"{pool.running()} shikigami are already running. Ask the user before starting more, "
                        "then call again with confirmed=true.", error=True)
        try:
            w = await pool.summon(args["name"], args["task"], args.get("cwd", "~"), args.get("model", "opus"),
                                  args.get("effort"), args.get("context") or "400k", args.get("subagent_model") or "haiku")
        except Exception as e:
            return text(f"Could not summon: {e}", error=True)
        r = w.rec
        crowd = await asyncio.to_thread(writers, pool, w, own())
        return text(f"Summoned {w.name} ({w.id}) in {r['cwd']} on {r['model']}, effort {r['effort'] or 'default'}, "
                    f"context {r['context_cap']}, subagents on {r['subagents']}." + (f"\n\n{crowd}" if crowd else ""))

    @tool("workers", "List shikigami from the last day with their state and one-line summary.", {})
    async def workers(args):
        listing = pool.listing()["list"]
        if not listing:
            return text("No shikigami in the last day.")
        return text("\n".join(f"{w['name']} ({w['id']}): {w['state']}, {ago(w['updated'])}, in {w['cwd']}"
                              + (f": {w['summary']}" if w.get("summary") else "") for w in listing))

    @tool("peek", "Read a shikigami's recent messages and tool calls.", {
        "type": "object",
        "properties": {"worker": {"type": "string", "description": "id or name"},
                       "count": {"type": "integer", "default": 30}},
        "required": ["worker"]})
    async def peek(args):
        w = pool.find(args["worker"])
        if not w:
            return text("No such shikigami.", error=True)
        lines = [f"{w.name} ({w.id}) is {w.rec['state']}. Task: {w.rec['task'][:500]}"]
        for ev in w.log.before(None, int(args.get("count", 30))):
            kind = ev["type"]
            if kind in ("user", "assistant"):
                lines.append(f"[{kind}] {ev['text'][:1500]}")
            elif kind == "tool":
                lines.append(f"[tool] {ev['name']}: {ev['summary']}")
            elif kind == "tool_result" and ev["error"]:
                lines.append(f"[tool error] {ev['text'][:300]}")
        return text("\n".join(lines))

    @tool("message", "Send a shikigami a message: an answer to its question, new instructions, or 'continue'. "
          "It folds into a running turn or wakes a finished worker with its full context.", {
              "type": "object",
              "properties": {"worker": {"type": "string", "description": "id or name"}, "text": {"type": "string"}},
              "required": ["worker", "text"]})
    async def message(args):
        w = pool.find(args["worker"])
        if not w:
            return text("No such shikigami.", error=True)
        try:
            await w.send(args["text"])
        except RuntimeError as e:
            return text(str(e), error=True)
        return text(f"Sent to {w.name}.")

    @tool("stop", "Stop a running shikigami. Its session is kept; a later message resumes it.", {"worker": str})
    async def stop(args):
        w = pool.find(args["worker"])
        if not w:
            return text("No such shikigami.", error=True)
        if w.rec["state"] not in ACTIVE:
            return text(f"{w.name} is not running ({w.rec['state']}).")
        await w.stop()
        return text(f"Stopping {w.name}.")

    @tool("show_image", "Show the user an image inline in the chat: a screenshot, render or chart saved as PNG, JPEG, "
          "WebP or GIF (not SVG).", {
              "type": "object",
              "properties": {"path": {"type": "string", "description": "absolute path of the image file"},
                             "caption": {"type": "string", "description": "optional one-line caption"}},
              "required": ["path"]})
    async def show_image(args):
        path = Path(os.path.expanduser(str(args["path"])))
        try:
            if path.stat().st_size > MAX_BYTES:
                return text("That image is larger than 20 MB.", error=True)
            name, _ = files.save(path.read_bytes())
        except (OSError, ValueError) as e:
            return text(f"Could not show it: {e}", error=True)
        events.add({"type": "image", "file": name, "caption": str(args.get("caption") or "")[:300]})
        return text("Shown to the user.")

    @tool("send_file", "Hand the user a file to download on their phone or page: any type (PDF, archive, log, "
          "audio...), up to 50 MB. It shows in the chat as a download chip. For pictures to look at, use show_image.", {
              "type": "object",
              "properties": {"path": {"type": "string", "description": "absolute path of the file"},
                             "caption": {"type": "string", "description": "optional one-line caption"}},
              "required": ["path"]})
    async def send_file(args):
        if not uploads:
            return text("File sending is not set up.", error=True)
        try:
            info = uploads.add(Path(os.path.expanduser(str(args["path"]))))
        except (OSError, ValueError) as e:
            return text(f"Could not send it: {e}", error=True)
        # "file", not "id": the event log numbers every event with its own id.
        events.add({"type": "file", "file": info["id"], **{k: info[k] for k in ("name", "size", "kind")},
                    "caption": str(args.get("caption") or "")[:300]})
        return text(f"Sent {info['name']} to the user.")

    own = pool.own_sessions

    @tool("queue", "claude-queue right now: which queued command holds the GPU/NPU turn (pid, session, RSS, GPU "
          "memory, how long, STUCK when its command ended but it still holds the turn, GUI app), which device opens "
          "wait and for what (another command, a bench, free memory), and how long. Use it when a job seems stuck "
          "on its GPU or NPU.", {})
    async def queue(args):
        snap = await asyncio.to_thread(cq.snapshot, pool.queue_names())
        return text(cq.describe(snap))

    @tool("sessions", "List every running Claude Code session on the laptop: name, status (idle, busy, waiting with "
          "what for, shell, parked), kind, folder and last activity. Includes the user's own sessions.", {})
    async def sessions(args):
        rows = sess.live(own())
        if not rows:
            return text("No running sessions.")
        home = str(Path.home())
        return text("\n".join(
            f"{s['name']} [pid {s['pid']}]: {s['status']} since {sess.ago(s['since'])}, last activity {sess.ago(s['active'])}, "
            f"{s['kind']}, in {s['cwd'].replace(home, '~', 1)}" for s in rows))

    @tool("session_peek", "Read a running session: its last user messages and assistant texts, an open "
          "AskUserQuestion with its options (SendMessage does not reach a session sitting in one), and tool calls "
          "still running. Session by name, pid or session id.", {
              "type": "object",
              "properties": {"session": {"type": "string"}, "count": {"type": "integer", "default": 3}},
              "required": ["session"]})
    async def session_peek(args):
        s = sess.find(str(args["session"]), own())
        if not s:
            names = ", ".join(x["name"] for x in sess.live(own()))
            return text(f"No single running session matches. Running: {names}", error=True)
        path = sess.transcript(s["rec"])
        if not path:
            return text(f"{s['name']} has no transcript yet.")
        p = sess.peek(path, max(1, min(int(args.get("count", 3)), 20)))
        lines = [f"{s['name']} [pid {s['pid']}, {s['kind']}]: {s['status']} since {sess.ago(s['since'])}, "
                 f"last activity {sess.ago(s['active'])}, in {s['cwd']}"]
        for q in p["questions"]:
            lines.append(f"\nOPEN AskUserQuestion (asked {q['asked'][:19]}Z), waiting for an answer in the terminal:")
            for item in q["questions"]:
                lines.append(f"- {item.get('question')}" + (" (multi-select)" if item.get("multiSelect") else ""))
                for i, o in enumerate(item.get("options", []), 1):
                    lines.append(f"    {i}. {o.get('label')}: {o.get('description', '')}")
        for c in p["open_calls"]:
            lines.append(f"\nStill running: {c['tool']} since {c['since'][:19]}Z: {c['what']}")
        merged = sorted([(t, "user", x) for t, x in p["user"]] + [(t, "assistant", x) for t, x in p["assistant"]])
        if merged:
            lines.append("\nRecent messages, oldest first:")
            lines += [f"[{who} {t[11:19]}] {x}" for t, who, x in merged]
        return text("\n".join(lines))

    @tool("usage", "The account's plan usage: the 5-hour and weekly windows (share used, reset time) and today's "
          "tokens per seimei session.", {})
    async def usage(args):
        return text(pool.dispatcher.usage.report())

    return always_load(create_sdk_mcp_server("seimei", tools=[summon, workers, peek, message, stop, show_image, send_file,
                                                              sessions, session_peek, queue, usage, *extra]), DEFERRED)
