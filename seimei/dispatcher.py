"""The permanent dispatcher session: one Claude Code conversation that survives restarts."""

import asyncio
import json
import logging
import os
import shutil
import uuid
from pathlib import Path

from claude_agent_sdk import (
    AssistantMessage,
    ClaudeAgentOptions,
    ClaudeSDKClient,
    RateLimitEvent,
    ResultMessage,
    StreamEvent,
    SystemMessage,
    TextBlock,
    ToolResultBlock,
    ToolUseBlock,
    UserMessage,
    get_session_info,
)

from .events import EventLog

log = logging.getLogger("seimei")

PROMPT = """\
# You are seimei
You are seimei, the user's permanent assistant on their laptop (Arch, niri).
They talk to you through a web page, usually from their phone.
- This is one conversation that never resets; older turns get compacted.
  Keep durable notes (ongoing tasks, decisions, open questions) in {notes} and re-read them after a compaction or restart.
- Replies are read on a phone: lead with the answer, keep it short, avoid wide tables.
- Messages starting with [seimei] are events from the server, not typed by the user.
"""

QUEUE_LIB = Path.home() / ".local/lib/claude-queue"
RESULT_CHARS = 4000


def summarize(inp: dict) -> str:
    """One line describing a tool call, for the collapsed activity row."""
    for key in ("command", "file_path", "path", "pattern", "url", "query", "description", "prompt"):
        if isinstance(inp.get(key), str):
            return inp[key].strip().splitlines()[0][:200] if inp[key].strip() else ""
    return json.dumps(inp)[:200]


def result_text(content) -> str:
    if isinstance(content, str):
        text = content
    elif isinstance(content, list):
        text = "\n".join(c.get("text", "") if c.get("type") == "text" else f"[{c.get('type')}]" for c in content)
    else:
        text = ""
    return text if len(text) <= RESULT_CHARS else text[:RESULT_CHARS] + "\n[...]"


def queue_env() -> dict:
    """Run the dispatcher's commands through claude-queue, like a `claude-queue seimei` session."""
    if not (QUEUE_LIB / "queue-prefix").exists():
        return {}
    qdir = Path(os.environ.get("XDG_RUNTIME_DIR") or f"/tmp/claude-queue-{os.getuid()}") / "claude-queue"
    qdir.mkdir(parents=True, exist_ok=True)
    return {
        "CLAUDE_QUEUE": "seimei",
        "CLAUDE_QUEUE_DIR": str(qdir),
        "CLAUDE_QUEUE_LIB": str(QUEUE_LIB / "devgate.so"),
        "CLAUDE_CODE_SHELL_PREFIX": str(QUEUE_LIB / "queue-prefix"),
    }


class Dispatcher:
    def __init__(self, cfg: dict, events: EventLog, state_dir: Path):
        self.cfg = cfg
        self.events = events
        self.sid_file = state_dir / "session_id"
        self.notes = state_dir / "notes.md"
        self.client: ClaudeSDKClient | None = None
        self.connected = False
        self.busy = False
        self.stopping = False
        self.pending: list[str] = []
        self.write_lock = asyncio.Lock()
        self.rate_limits: dict[str, dict] = {}

    def state(self) -> dict:
        return {"type": "state", "connected": self.connected, "busy": self.busy}

    def _set(self, **kw):
        changed = any(getattr(self, k) != v for k, v in kw.items())
        for k, v in kw.items():
            setattr(self, k, v)
        if changed:
            self.events.flash(self.state())

    def _options(self) -> ClaudeAgentOptions:
        cwd = os.path.expanduser(self.cfg.get("cwd", "~"))
        sid = self.sid_file.read_text().strip() if self.sid_file.exists() else ""
        if sid and get_session_info(sid, directory=cwd):
            where = {"resume": sid}
        else:
            # The transcript only exists after the first message, so a fresh id is kept until then.
            sid = sid or str(uuid.uuid4())
            self.sid_file.write_text(sid + "\n")
            where = {"session_id": sid}
        return ClaudeAgentOptions(
            cli_path=shutil.which("claude") or "claude",
            cwd=cwd,
            model=self.cfg.get("model", "opus"),
            permission_mode="bypassPermissions",
            include_partial_messages=True,
            setting_sources=["user", "project", "local"],
            system_prompt={"type": "preset", "preset": "claude_code", "append": PROMPT.format(notes=self.notes)},
            env=queue_env(),
            **where,
        )

    async def run(self):
        delay, failures = 1, 0
        while True:
            try:
                self.client = ClaudeSDKClient(self._options())
                await self.client.connect()
                self._set(connected=True)
                if failures:
                    self.events.add({"type": "system", "text": "Session is back."})
                delay, failures = 1, 0
                while self.pending:
                    await self._write(self.pending.pop(0))
                async for msg in self.client.receive_messages():
                    self._handle(msg)
                raise RuntimeError("Claude Code exited")
            except asyncio.CancelledError:
                raise
            except Exception as e:
                log.exception("dispatcher session failed")
                if not failures:
                    self.events.add({"type": "system", "level": "error", "text": f"Session stopped ({e}), reconnecting."})
                failures += 1
            finally:
                self._set(connected=False, busy=False)
                client, self.client = self.client, None
                if client:
                    try:
                        await client.disconnect()
                    except Exception:
                        pass
            await asyncio.sleep(delay)
            delay = min(delay * 2, 60)

    async def _write(self, text: str):
        async with self.write_lock:
            await self.client.query(text)

    async def send(self, text: str):
        self.events.add({"type": "user", "text": text})
        self._set(busy=True)
        # A message sent mid-turn is folded into the running turn at its next step.
        if self.connected:
            await self._write(text)
        else:
            self.pending.append(text)

    async def interrupt(self):
        if self.connected and self.busy:
            self.stopping = True
            await self.client.interrupt()

    def _handle(self, msg):
        if isinstance(msg, StreamEvent):
            if msg.parent_tool_use_id:
                return
            ev = msg.event
            kind = ev.get("type")
            if kind == "message_start":
                self._set(busy=True)
            elif kind == "content_block_delta" and ev["delta"].get("type") == "text_delta":
                self.events.flash({"type": "delta", "text": ev["delta"]["text"]})
            elif kind == "content_block_start" and ev["content_block"].get("type") == "thinking":
                self.events.flash({"type": "thinking"})
        elif isinstance(msg, AssistantMessage):
            if msg.parent_tool_use_id:
                return
            for b in msg.content:
                if isinstance(b, TextBlock) and b.text.strip():
                    self.events.add({"type": "assistant", "text": b.text})
                elif isinstance(b, ToolUseBlock):
                    self.events.add({"type": "tool", "tool_id": b.id, "name": b.name, "summary": summarize(b.input)})
        elif isinstance(msg, UserMessage):
            if msg.parent_tool_use_id or isinstance(msg.content, str):
                return
            for b in msg.content:
                if isinstance(b, ToolResultBlock):
                    self.events.add({"type": "tool_result", "tool_id": b.tool_use_id, "error": bool(b.is_error),
                                     "text": result_text(b.content)})
        elif isinstance(msg, SystemMessage):
            if msg.subtype == "compact_boundary":
                self.events.add({"type": "system", "text": "Older context was compacted."})
        elif isinstance(msg, ResultMessage):
            if msg.is_error:
                if self.stopping:
                    self.events.add({"type": "system", "text": "Stopped."})
                else:
                    self.events.add({"type": "system", "level": "error", "text": f"Turn failed: {msg.result or msg.subtype}"})
            self.stopping = False
            self._set(busy=False)
        elif isinstance(msg, RateLimitEvent):
            info = msg.rate_limit_info
            window = info.rate_limit_type or "unknown"
            prev = self.rate_limits.get(window, {}).get("status", "allowed")
            self.rate_limits[window] = {"status": info.status, "resets_at": info.resets_at, "utilization": info.utilization}
            if info.status != prev:
                self.events.add({"type": "rate_limit", "window": window, **self.rate_limits[window]})
