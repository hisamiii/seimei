"""The dispatcher's own tools, served in-process as the `seimei` MCP server."""

import asyncio
import os
import re
import subprocess
import time
from pathlib import Path

from claude_agent_sdk import create_sdk_mcp_server, tool

from . import metrics
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
DEFERRED = {"niri_query", "discord_search", "discord_edit", "discord_delete", "discord_react", "discord_mark_read", "browser_read",
            "browser_switch", "browser_download", "browser_downloads", "scroll", "notifications", "send_file",
            "restore_desktop", "unlock_screen", "queue_control", "queue_policy", "queue_history",
            "adopt"}


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


def address(s: dict) -> str:
    """Where SendMessage reaches a session, whatever its name."""
    return "uds:" + (s["rec"].get("messagingSocketPath") or f"/run/user/{os.getuid()}/cc-socks/{s['pid']}.sock")


MODELS = ("opus", "sonnet", "haiku")
EFFORTS = ("low", "medium", "high", "xhigh", "max")
CONTEXTS = ("256k", "400k", "1m")
# What summon must be told each time (rules.md: the sizing rule lives in the schema, not in memory).
SUMMON_PICKS = ("cwd", "model", "effort", "context")
EFFORT_GUIDE = ("reasoning effort, picked per task: low (or the haiku model) for lookups; medium for mockups, CSS and "
                "config; high for research, kernels and risky or delicate work; xhigh or max only for escalations or "
                "tasks seimei judges very important, never for long iterative work")
CONTEXT_GUIDE = "where it compacts: 256k or 400k for most tasks; 1m only when the task really needs it"


def summon_problem(args: dict) -> str:
    """Why a summon call cannot go ahead (a pick missing or not one of the choices), or ""."""
    missing = [k for k in SUMMON_PICKS if not str(args.get(k) or "").strip()]
    if missing:
        return (f"summon needs {', '.join(missing)}: pick {'it' if len(missing) == 1 else 'each'} for this task, there are no "
                f"defaults. cwd: the project folder (~ only for home tasks). model: {'/'.join(MODELS)}. effort: "
                f"{EFFORT_GUIDE}. context: {CONTEXT_GUIDE}.")
    for k, choices in (("model", MODELS), ("effort", EFFORTS), ("context", CONTEXTS)):
        if args[k] not in choices:
            return f"summon: {k} must be one of {', '.join(choices)}, not {args[k]!r}."
    return ""


def make_tools(pool: Pool, files: Files, events: EventLog, extra: list = (), uploads=None):
    @tool("summon", "Start a shikigami: a worker Claude Code session that works on a task on its own and reports back. "
          "Every call picks name, task, cwd, model, effort and context for this task: there are no defaults. "
          "Returns at once; the report arrives later.", {
              "type": "object",
              "properties": {
                  "name": {"type": "string", "description": "short lowercase name, like npu-dma or readme-fix"},
                  "task": {"type": "string", "description": "the full instructions; the worker sees nothing else"},
                  "cwd": {"type": "string", "description": "the project folder the task is about; ~ only when the task is about home"},
                  "model": {"type": "string", "enum": list(MODELS),
                            "description": "opus for real work, sonnet for routine jobs, haiku for quick lookups"},
                  "effort": {"type": "string", "enum": list(EFFORTS), "description": EFFORT_GUIDE},
                  "context": {"type": "string", "enum": list(CONTEXTS), "description": CONTEXT_GUIDE},
                  "subagent_model": {"type": "string", "enum": ["haiku", "sonnet", "opus"], "default": "haiku",
                                     "description": "model for the shikigami's own subagents; haiku unless a rare case needs more"},
                  "confirmed": {"type": "boolean", "description": f"set only after the user OKed more than {MAX_RUNNING} running workers"},
              },
              "required": ["name", "task", *SUMMON_PICKS]})
    async def summon(args):
        if bad := summon_problem(args):
            return text(bad, error=True)
        if pool.running() >= MAX_RUNNING and not args.get("confirmed"):
            return text(f"{pool.running()} shikigami are already running. Ask the user before starting more, "
                        "then call again with confirmed=true.", error=True)
        try:
            w = await pool.summon(args["name"], args["task"], args["cwd"], args["model"], args["effort"], args["context"],
                                  args.get("subagent_model") or "haiku")
        except Exception as e:
            return text(f"Could not summon: {e}", error=True)
        r = w.rec
        crowd = await asyncio.to_thread(writers, pool, w, own())
        return text(f"Summoned {w.name} ({w.id}) in {r['cwd']} on {r['model']}, effort {r['effort'] or 'default'}, "
                    f"context {r['context_cap']}, subagents on {r['subagents']}." + (f"\n\n{crowd}" if crowd else ""))

    worker_events = metrics.Events(events.path)

    @tool("workers", "List shikigami from the last day with their state and one-line summary. Ones the user archived "
          "(or that ended over 12 hours ago) are left out; archived: true lists only those, from any time. metrics: "
          "true gives every shikigami's (or the named ones') task metrics instead: model / effort / context, folder, "
          "tokens and $ at API prices, wall time, end state, asks, follow-ups after its first done, and reopened.", {
              "type": "object", "properties": {"archived": {"type": "boolean"}, "metrics": {"type": "boolean"},
                                               "worker": {"type": "string", "description": "with metrics: one id or name"}}})
    async def workers(args):
        if args.get("metrics"):
            ids = None
            if args.get("worker"):
                w = pool.find(str(args["worker"]))
                if not w:
                    return text("No such shikigami.", error=True)
                ids = [w.id]
            cost = pool.dispatcher.usage and pool.dispatcher.usage.cost
            if not cost:
                return text("No token counts here (cost reading is off).", error=True)
            rows = await asyncio.to_thread(metrics.collect, pool, cost, worker_events, ids)
            return text(metrics.text(rows))
        if args.get("archived"):
            old = sorted((w for w in pool.workers.values() if pool.archive.worker(w.rec)), key=lambda w: -w.rec["updated"])
            listing = [w.info() for w in old[:50]]
        else:
            listing = [w for w in pool.listing()["list"] if not w["archived"]]
        if not listing:
            return text("No archived shikigami." if args.get("archived") else "No shikigami in the last day.")
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

    @tool("queue", "claude-queue right now: which queued command holds the GPU/NPU turn (pid, session, memory, how "
          "long, about how long it has left, STUCK when its command ended but it still holds the turn), which device "
          "opens wait, in which order, for what (another command, a bench, free memory) and roughly when they start, "
          "agents frozen by a bench, idle memory hogs, and whether grants are closed after a GPU fault. Use it when a "
          "job seems stuck on its GPU or NPU.", {})
    async def queue(args):
        snap = await asyncio.to_thread(cq.snapshot, pool.queue_names())
        return text(cq.describe(snap))

    @tool("sessions", "List every running Claude Code session on the laptop: your alias for it, name, status (idle, "
          "busy, waiting with what for, shell, parked), kind, folder and last activity. Includes the user's own "
          "sessions. Aliased ones show their SendMessage address, which holds through renames.", {})
    async def sessions(args):
        rows = sess.live(own())
        if not rows:
            return text("No running sessions.")
        home = str(Path.home())

        def head(s):
            a = pool.aliases.of(s)
            return (f"{a} = \"{s['name']}\" [pid {s['pid']}, to {address(s)}]" if a else f"{s['name']} [pid {s['pid']}]")
        return text("\n".join(
            f"{head(s)}: {s['status']} since {sess.ago(s['since'])}, last activity {sess.ago(s['active'])}, "
            f"{s['kind']}, in {s['cwd'].replace(home, '~', 1)}" for s in rows))

    @tool("alias", "Your own name for a session (like dev or asr) that holds when it is renamed: sessions, "
          "session_peek and adopt take it, and sessions shows its SendMessage address. session: its current name, "
          "pid or alias; leave alias empty to remove it. Without arguments: the list.", {
              "type": "object", "properties": {"alias": {"type": "string"}, "session": {"type": "string"}}})
    async def alias(args):
        name, key = str(args.get("alias") or "").strip(), str(args.get("session") or "").strip()
        rows = sess.live(own())
        if not key and not name:
            if not pool.aliases.map:
                return text("No aliases yet.")
            return text("\n".join(f"{a} = " + (f"\"{s['name']}\" [pid {s['pid']}, to {address(s)}]" if (s := pool.aliases.resolve(a, rows))
                                               else "(not running)") for a in list(pool.aliases.map)))
        if not key:
            return text(f"Removed {name}." if pool.aliases.remove(name) else f"No alias {name}.")
        s = sess.find(key, own(), pool.aliases)
        if not s:
            return text(f"No single running session matches {key!r}.", error=True)
        if not name:
            a = pool.aliases.of(s)
            return text(f"Removed {a}." if a and pool.aliases.remove(a) else f"{s['name']} has no alias.")
        if not re.fullmatch(r"[\w.-]{1,32}", name):
            return text("An alias is up to 32 letters, digits, dots, dashes or underscores.", error=True)
        pool.aliases.set(name, s)
        return text(f"{name} = \"{s['name']}\" [pid {s['pid']}], SendMessage to {address(s)}.")

    @tool("session_peek", "Read a running session: its last user messages and assistant texts, an open "
          "AskUserQuestion with its options (SendMessage does not reach a session sitting in one), and tool calls "
          "still running. Session by name, pid or session id.", {
              "type": "object",
              "properties": {"session": {"type": "string"}, "count": {"type": "integer", "default": 3}},
              "required": ["session"]})
    async def session_peek(args):
        s = sess.find(str(args["session"]), own(), pool.aliases)
        if not s:
            names = ", ".join(x["name"] for x in sess.live(own()))
            return text(f"No single running session matches. Running: {names}", error=True)
        path = sess.transcript(s["rec"])
        if not path:
            return text(f"{s['name']} has no transcript yet.")
        p = sess.peek(path, max(1, min(int(args.get("count", 3)), 20)))
        lines = [f"{s['name']} [pid {s['pid']}, {s['kind']}]: {s['status']} since {sess.ago(s['since'])}, "
                 f"last activity {sess.ago(s['active'])}, in {s['cwd']}; SendMessage to {address(s)}"]
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

    @tool("usage", "The account's plan usage: the 5-hour and weekly windows (share used, reset time), today's "
          "tokens per seimei session, and at API prices: what this and earlier 5-hour windows cost and the price of "
          "1% of the window in each, and per session (yours, shikigami, the user's) this window and today: $, calls, "
          "output / cache read / cache write / input tokens and average context per call.", {})
    async def usage(args):
        names = {s["session_id"]: s["name"] for s in await asyncio.to_thread(sess.live, {})}
        names.update(own())
        return text(await asyncio.to_thread(pool.dispatcher.usage.report, names))

    return always_load(create_sdk_mcp_server("seimei", tools=[summon, workers, peek, message, stop, show_image, send_file,
                                                              sessions, session_peek, alias, queue, usage, *extra]), DEFERRED)
