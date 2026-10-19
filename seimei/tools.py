"""The dispatcher's own tools, served in-process as the `seimei` MCP server."""

import time

from claude_agent_sdk import create_sdk_mcp_server, tool

from .workers import ACTIVE, Pool

MAX_RUNNING = 5


def text(t: str, error: bool = False) -> dict:
    return {"content": [{"type": "text", "text": t}], "is_error": error}


def ago(t: float) -> str:
    s = int(time.time() - t)
    return f"{s // 60} min ago" if s < 3600 else f"{s // 3600} h ago" if s < 86400 else f"{s // 86400} d ago"


def make_tools(pool: Pool):
    @tool("summon", "Start a shikigami: a worker Claude Code session that works on a task on its own and reports back. "
          "Pick a short lowercase name, the folder the task is about, and a model (opus for real work, sonnet for "
          "routine jobs, haiku for quick lookups). Returns at once; the report arrives later.", {
              "type": "object",
              "properties": {
                  "name": {"type": "string", "description": "short lowercase name, like npu-dma or readme-fix"},
                  "task": {"type": "string", "description": "the full instructions; the worker sees nothing else"},
                  "cwd": {"type": "string", "description": "folder to work in, default ~"},
                  "model": {"type": "string", "enum": ["opus", "sonnet", "haiku"], "default": "opus"},
                  "confirmed": {"type": "boolean", "description": f"set only after the user OKed more than {MAX_RUNNING} running workers"},
              },
              "required": ["name", "task"]})
    async def summon(args):
        if pool.running() >= MAX_RUNNING and not args.get("confirmed"):
            return text(f"{pool.running()} shikigami are already running. Ask the user before starting more, "
                        "then call again with confirmed=true.", error=True)
        try:
            w = await pool.summon(args["name"], args["task"], args.get("cwd", "~"), args.get("model", "opus"))
        except Exception as e:
            return text(f"Could not summon: {e}", error=True)
        return text(f"Summoned {w.name} ({w.id}) in {w.rec['cwd']} on {w.rec['model']}.")

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
        await w.send(args["text"])
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

    return create_sdk_mcp_server("seimei", tools=[summon, workers, peek, message, stop])
