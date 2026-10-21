"""Shikigami: worker Claude sessions that seimei summons, watches and reports on."""

import asyncio
import json
import logging
import os
import re
import shlex
import shutil
import time
import uuid
from pathlib import Path

from claude_agent_sdk import (
    AssistantMessage,
    ClaudeAgentOptions,
    ClaudeSDKClient,
    HookMatcher,
    RateLimitEvent,
    ResultMessage,
    TaskNotificationMessage,
    TaskStartedMessage,
    TextBlock,
    ToolResultBlock,
    ToolUseBlock,
    UserMessage,
    create_sdk_mcp_server,
    tool,
)

from . import deploys, permissions, sessions
from .aliases import Aliases
from .archive import Archive
from .events import EventLog
from .util import NO_CONNECTORS, STREAM_BUFFER, result_text, summarize, write_json

log = logging.getLogger("seimei.workers")

PROMPT = """\
# You are a shikigami
You are `{name}`, a worker summoned by seimei, the user's dispatcher on their {about}.
Nobody watches your terminal: work on the task on your own until it is done.
Your final message is shown to the user on their phone as your report: start it with a one-line summary, keep it short.
Before reporting done, run the checks that prove it (tests, build, a command, a screenshot) and list them with their result in the report. If you could not verify, say so in the first line.
If you are blocked on a decision only the user can make, call mcp__shikigami__ask with the question and end your turn.
Subagents are fine for searches and lookups; they run on {subagents}. No big parallel fan-outs.
Start every task with a short step list (3-8 steps, TaskCreate) and keep it current as you go (TaskUpdate: in_progress when you start a step, completed when it is done; add or delete steps when the plan changes). It shows live as your task's steps on the user's board. Skip it only for a one-step job.
When you switch to a queued board item, start it (mcp__shikigami__board); when it is deployed or delivered, ship it.
"""
EFFORTS = ("low", "medium", "high", "xhigh", "max")
MODELS = ("opus", "sonnet", "haiku")
# What Claude Code's /effort command answers (no model turn): not part of the task.
EFFORT_SET = ("Set effort level", "Effort level")
# The context a shikigami may fill before it compacts: the user keeps most at 256-400k, 1m only where really needed.
CONTEXTS = {"256k": 256_000, "400k": 400_000, "1m": 1_000_000}
ACTIVE = {"running", "asking"}
IDLE_DISCONNECT = 600
# A turn with nothing to say (a stale task notification replayed on resume) is no report: wait this long for the
# real turn before calling the worker done.
TRIVIAL_GRACE = 30
TRIVIAL = {"", "No response requested."}
# Built-in tools a shikigami has no use for: scheduling, notebooks, worktree switching, remote triggers, design sync,
# and talking to other sessions (its report and ask go through seimei).
OFF_TOOLS = ["Workflow", "NotebookEdit", "CronCreate", "CronDelete", "CronList", "RemoteTrigger", "PushNotification",
             "DesignSync", "EnterWorktree", "ExitWorktree", "ScheduleWakeup", "SendMessage", "ListAgents"]
# The only skills a shikigami sees (the rest, like the claude.ai document bundle, only fill its context).
SKILLS = ["code-review", "simplify"]
# A worker cut off by a restart is resumed with this; one resumed this often within the window stays interrupted
# instead (a crash loop would otherwise keep poking it).
RESUME = "continue: seimei restarted, check your state first"
RESUME_CAP = 3
RESUME_WINDOW = 3600
# Seconds to gather a burst of events into one message for the dispatcher.
BATCH = 3
RECENT = 24 * 3600
# Tool calls a shikigami may make on a task without a step list before it is reminded, once.
NUDGE_AFTER = 15
NUDGE = ("seimei: you have made about 15 tool calls on this task without a step list. Write one now (3-8 steps, "
         "TaskCreate) and keep it current with TaskUpdate: it shows live on the user's board.")
STEP = re.compile(r"Task #(\w+)")


def slug(name: str) -> str:
    return re.sub(r"[^a-z0-9-]+", "-", name.lower()).strip("-")[:32] or "worker"


def title_of(task: str, name: str) -> str:
    """A board title for a summoned task: its first sentence without seimei's #NN or a leading aside like "(user OK):",
    cut at a word near 60 chars."""
    line = re.sub(r"^\s*(#\d+\b|\([^)]*\))[\s:,.;-]*", "", first_line(task, 400))
    line = re.sub(r"^\s*\([^)]*\)[\s:,.;-]*", "", line)
    line = re.split(r"(?<=[.!?])\s", line, maxsplit=1)[0].rstrip(".:")
    line = line[:1].upper() + line[1:]
    if len(line) > 60:
        line = line[:60].rsplit(" ", 1)[0].rstrip(",;:-") + "…"
    return line or name


def first_line(text: str, n: int = 160) -> str:
    line = next((l.strip() for l in text.splitlines() if l.strip()), "")
    return line if len(line) <= n else line[: n - 1] + "…"


class Worker:
    def __init__(self, pool: "Pool", rec: dict):
        self.pool = pool
        self.rec = rec
        self.log = EventLog(pool.dir / f"{rec['id']}.jsonl")
        self.client: ClaudeSDKClient | None = None
        self.reader: asyncio.Task | None = None
        self.lock = asyncio.Lock()
        self.stopping = False
        self.last_text = ""
        self.idle_timer: asyncio.TimerHandle | None = None
        self.grace: asyncio.TimerHandle | None = None
        self.turn_text = False
        # Subagents still working in the background: the turn may end before they report back.
        self.tasks: dict[str, str] = {}
        # /effort commands sent to the session whose own result (no model turn) is not a turn of the task.
        self.quiet = 0
        # TaskCreate calls waiting for their result, which names the step's id.
        self.steps: dict[str, str] = {}

    @property
    def id(self) -> str:
        return self.rec["id"]

    @property
    def name(self) -> str:
        return self.rec["name"]

    def info(self) -> dict:
        r = self.rec
        return {**{k: r.get(k) for k in ("id", "name", "state", "cwd", "model", "effort", "context_cap", "subagents", "created",
                                         "updated", "summary", "question", "doing", "card", "tunes", "effort_next")},
                "mode": self.mode,
                "started": bool(r.get("started")), "archived": self.pool.archive.worker(r)}

    def brief(self) -> dict:
        """Project and context use from the worker's transcript, like the user's sessions show."""
        path = sessions.transcript({"sessionId": self.rec["session_id"], "cwd": self.rec["cwd"]}) if self.rec.get("started") else None
        try:
            b = sessions.brief(path) if path else {}
        except OSError:
            b = {}
        cap = CONTEXTS.get(self.rec.get("context_cap") or "")
        used = b.get("tokens") or 0
        # The ring fills toward the point where this shikigami compacts.
        context = round(100 * used / cap) if cap and used else b.get("context")
        return {"project": b.get("project") or sessions.project_of(self.rec["cwd"] + "/x"), "context": context,
                "effort_now": b.get("effort")}

    def _set(self, state: str, **kw):
        self.rec.update(state=state, updated=time.time(), **kw)
        self.pool.changed()
        if self.pool.board:
            self.pool.board.follow(self.id, state)

    def _add(self, ev: dict):
        ev = self.log.add(ev)
        self.pool.events.flash({"type": "w", "worker": self.id, "ev": ev})

    def _doing(self, what: dict):
        """The latest step, for the Agents page; the next listing write carries it to disk."""
        self.rec["doing"] = what
        self.pool.events.flash(self.pool.listing())

    def _options(self) -> ClaudeAgentOptions:
        r = self.rec
        where = {"resume": r["session_id"]} if r.get("started") else {"session_id": r["session_id"]}
        return ClaudeAgentOptions(
            cli_path=shutil.which("claude") or "claude",
            cwd=r["cwd"],
            max_buffer_size=STREAM_BUFFER,
            model=r["model"],
            permission_mode=self.mode,
            # Whatever Claude Code would ask in a terminal becomes a card (in bypass only its safety checks ask: blocked).
            **({"can_use_tool": permissions.callback(self.pool.guard.approvals, r["name"], self, lambda: self.pool.mode,
                                                     self._mode_chosen, lambda: self.mode)} if self.pool.guard else {}),
            # Its own tools and seimei's plugins carry their own cards; external MCP servers ask like the rest.
            allowed_tools=["mcp__shikigami", *(f"mcp__{g}" for g in self.granted() if self.pool.plugins.own(g))],
            setting_sources=["user", "project", "local"],
            system_prompt={"type": "preset", "preset": "claude_code", "append": self.prompt()},
            # Its own ask, and the plugin groups summon granted it, built for it (their cards name it).
            mcp_servers={"shikigami": self._tools(), **(self.pool.plugins.servers(r["name"], r["id"], self.granted())
                                                       if self.pool.plugins else {})},
            # Workers talk to the user through their report and ask, never to other sessions directly.
            disallowed_tools=OFF_TOOLS,
            skills=SKILLS,
            env=self._env(),
            **({"effort": r["effort"]} if r.get("effort") else {}),
            extra_args={"name": f"shikigami {r['name']}"},
            hooks=self._hooks(),
            **where,
        )

    @property
    def mode(self) -> str:
        """Its permission mode: what summon or the user set, else the configured one."""
        return self.rec.get("mode") or self.pool.mode

    def _mode_chosen(self, mode: str):
        """The user picked a mode on one of its cards (Allow edits for this session, Go ahead on a plan)."""
        self.rec["mode"] = mode
        self.pool.changed()

    async def set_mode(self, mode: str):
        """The user changed its mode on the Agents page; a running session switches at once."""
        if mode not in permissions.MODES:
            raise ValueError(f"mode is one of {', '.join(permissions.MODES)}")
        old = self.mode
        if self.client:
            await self.client.set_permission_mode(mode)
        if mode != old:
            label = permissions.LABELS
            self.rec.setdefault("tunes", []).append({"t": time.time(), "by": "the user",
                                                     "what": f"permissions {label[old]} → {label[mode]}"})
            self._add({"type": "system", "text": f"Permissions {label[old]} → {label[mode]} (by the user)."})
        self._mode_chosen(mode)

    async def tune(self, effort: str | None = None, model: str | None = None, by: str = "the user") -> list[str]:
        """Change its effort and model without restarting the task. The model switches through the SDK (its next
        request); the effort through Claude Code's /effort at the next turn boundary (sent mid-turn it would reach the
        model as text). Its log and its record keep what changed."""
        if effort and effort not in EFFORTS:
            raise ValueError(f"effort is one of {', '.join(EFFORTS)}")
        if model and model not in MODELS:
            raise ValueError(f"model is one of {', '.join(MODELS)}")
        if self.rec["state"] == "released":
            raise ValueError(f"{self.name} is the user's own session now")
        r, changed = self.rec, []
        if model and model != r["model"]:
            if self.client:
                await self.client.set_model(model)
            changed.append(f"model {r['model']} → {model}")
            r["model"] = model
        if effort and effort != (r.get("effort_next") or r.get("effort")):
            changed.append(f"effort {r.get('effort_next') or r.get('effort') or 'default'} → {effort}")
            if self.client:
                r["effort_next"] = effort
            else:
                # Not running: the next start picks it up from the record.
                r["effort"] = effort
        if changed:
            r.setdefault("tunes", []).append({"t": time.time(), "by": by, "what": ", ".join(changed)})
            self._add({"type": "system", "text": f"{', '.join(changed).capitalize()} (by {by})."})
            self.pool.changed()
            if r.get("effort_next") and r["state"] != "running":
                await self._apply_effort()
        return changed

    async def _apply_effort(self):
        """A pending effort change, sent while no turn runs."""
        async with self.lock:
            effort = self.rec.pop("effort_next", None)
            if not effort:
                return
            if self.client:
                self.quiet += 1
                await self.client.query(f"/effort {effort}")
            self.rec["effort"] = effort
            self.pool.changed()

    def waiting_card(self, text: str | None):
        """A permission card of its is open (what it wants) or settled (None): the Agents page and its board task
        show that it waits on the user, and seimei hears it."""
        self.rec["card"] = text
        self.pool.changed()
        if self.pool.board:
            self.pool.board.follow(self.id, "asking" if text else self.rec["state"])
        if text:
            self.pool.tell(self.id, f"waits on a permission card: {text}", key=f"perm:{self.id}")

    def granted(self) -> list[str]:
        """The plugin groups it may use: what summon granted that is still enabled."""
        plugins = self.pool.plugins
        return [g for g in self.rec.get("tools") or [] if plugins and g in plugins.names]

    def prompt(self) -> str:
        r, plugins = self.rec, self.pool.plugins
        text = PROMPT.format(name=r["name"], subagents=r.get("subagents") or "haiku", about=self.pool.about)
        if plugins and (notes := plugins.worker_notes()):
            text += notes + "\n"
        if plugins and (groups := self.granted()):
            text += (f"\nYou have seimei's {', '.join(groups)} tools (mcp__{groups[0]}__...), with seimei's rules for them:\n\n"
                     + plugins.prompt(groups) + "\n")
        return text

    def _hooks(self) -> dict:
        hooks = self.pool.guard.hooks(self.rec["name"], self.rec["id"]) if self.pool.guard else {}
        hooks.setdefault("PostToolUse", []).append(HookMatcher(hooks=[self._nudge]))
        return hooks

    async def _nudge(self, inp, tool_use_id, ctx):
        """About NUDGE_AFTER tool calls into a task with no step list: remind it once (per task)."""
        board = self.pool.board
        t = board and board.current(self.id)
        if not t or inp.get("agent_id") or t.get("steps") or inp.get("tool_name") in ("TaskCreate", "TaskUpdate"):
            return {}
        n = self.rec.get("nudge") or {}
        if n.get("task") != t["id"]:
            n = {"task": t["id"], "calls": 0}
        n["calls"] += 1
        self.rec["nudge"] = n
        if n["calls"] != NUDGE_AFTER:
            return {}
        return {"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": NUDGE}}

    def _env(self) -> dict:
        r = self.rec
        # No auto-memory: a shikigami reports what it learned, and seimei decides what is kept.
        # The todo tools (TaskCreate, TaskUpdate) are off by default for current models: the board mirrors them.
        env = {**(self.pool.plugins.env(r["name"]) if self.pool.plugins else {}), **NO_CONNECTORS, "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
               "CLAUDE_CODE_SUBAGENT_MODEL": r.get("subagents") or "haiku", "CLAUDE_CODE_ENABLE_TODO_TOOLS": "1"}
        if cap := CONTEXTS.get(r.get("context_cap") or ""):
            env["CLAUDE_CODE_AUTO_COMPACT_WINDOW"] = str(cap)
        return env

    def _tools(self):
        @tool("ask", "Ask the user a question through seimei when only they can decide. End your turn afterwards.",
              {"question": str})
        async def ask(args):
            q = str(args.get("question", "")).strip()
            self._set("asking", question=q)
            self.pool.report(self, "asking", q)
            return {"content": [{"type": "text", "text": "Asked. The answer arrives as your next message; end your turn now."}]}

        @tool("board", "Your own items on the user's task board. start: you move on to one of your queued items (it runs "
              "and follows you; what you worked on before goes back to the queue unless you shipped it). ship: it is "
              "deployed or delivered and waits for seimei's check (seimei marks it done).",
              {"type": "object", "properties": {"action": {"type": "string", "enum": ["start", "ship"]},
                                                "id": {"type": "string", "description": "#79 or 79"}},
               "required": ["action", "id"]})
        async def board(args):
            b, r = self.pool.board, self.rec
            try:
                if not b:
                    raise ValueError("there is no task board")
                if args.get("action") == "start":
                    t = b.start(self.id, r["name"], r["state"], args["id"])
                elif args.get("action") == "ship":
                    t = b.ship(self.id, r["name"], args["id"])
                else:
                    raise ValueError("action is start or ship")
            except (KeyError, ValueError) as e:
                return {"content": [{"type": "text", "text": str(e).strip("'")}], "is_error": True}
            return {"content": [{"type": "text", "text": f"{t['id']} {t['title']}: {t['state']}."}]}

        return create_sdk_mcp_server("shikigami", tools=[ask, board])

    def on_laptop(self) -> bool:
        """The session is open on the server (after Open on server): its kitty window is still there, or a running
        Claude Code holds the session (a trust prompt delays that, hence the window check)."""
        if self.rec["state"] != "laptop":
            return False
        from .sessions import live
        if self.pool.plugins and self.pool.plugins.window_open(self.rec.get("laptop_window")):
            return True
        return any(s["rec"].get("sessionId") == self.rec["session_id"] or s["session_id"] == self.rec["session_id"]
                   for s in live({}))

    async def hand_over(self) -> dict:
        """Lend the session to the user: open it with `claude --resume` in a kitty window on the server. It stays a
        shikigami: once the window is closed, the next message resumes it here."""
        if self.rec["state"] == "released":
            raise RuntimeError("it is your session already")
        if self.on_laptop():
            raise RuntimeError("it is already open on the server")
        w = await self._to_laptop(f"shikigami {self.name}", [])
        self._set("laptop", summary="Open on the server.", laptop_window=w and w["id"])
        return w

    async def release(self) -> dict | None:
        """Give the session to the user for good: it opens like a loan, but as one of their sessions under its own
        name. No reports, no resume, no messages from seimei; adopting the session makes it this shikigami again."""
        if self.rec["state"] == "released":
            raise RuntimeError("it is your session already")
        if self.on_laptop():
            # Lent already: that window becomes theirs as it is.
            w = None
        else:
            w = await self._to_laptop(self.name, ["--name", self.name])
        path = sessions.transcript({"sessionId": self.rec["session_id"], "cwd": self.rec["cwd"]})
        self._set("released", summary="Handed to you as a session.", question=None,
                  laptop_window=w["id"] if w else self.rec.get("laptop_window"),
                  released={"at": time.time(), "path": str(path) if path else None,
                            "offset": path.stat().st_size if path else 0})
        self._add({"type": "system", "text": "Handed to you: it goes on as your session on the server."})
        board = self.pool.board
        if board and (t := board.current(self.id)):
            board.update(t["id"], owner="you")
        return w

    async def _to_laptop(self, title: str, extra: list[str]) -> dict | None:
        """Stop it if it works, let go of its process, and resume the session in kitty on the server."""
        if not self.rec.get("started"):
            raise RuntimeError("it has no session yet")
        if self.rec["state"] in ACTIVE:
            await self.stop()
            for _ in range(150):
                if self.rec["state"] not in ACTIVE:
                    break
                await asyncio.sleep(0.2)
            else:
                raise RuntimeError("it did not stop in 30 s")
        async with self.lock:
            if self.idle_timer:
                self.idle_timer.cancel()
                self.idle_timer = None
            if self.client:
                self.client = None
                self.reader.cancel()
                await asyncio.gather(self.reader, return_exceptions=True)
            return await self.pool.open_terminal(self, title, extra)

    async def send(self, text: str, shown: str | None = None, files: list[dict] | None = None):
        """Start the session if needed and hand it a message; folds into a running turn. The log keeps what the user
        wrote (shown) and their files as chips; the session gets text, with the files' paths."""
        if self.rec["state"] == "released":
            raise RuntimeError(f"{self.name} is the user's own session now; it becomes a shikigami again only when "
                               "adopted (the user taps Hand to seimei, or you adopt it with their OK)")
        if await asyncio.to_thread(self.on_laptop):
            raise RuntimeError(f"{self.name} is open on the server; message it there, or close that window first")
        async with self.lock:
            if self.idle_timer:
                self.idle_timer.cancel()
                self.idle_timer = None
            self._add({"type": "user", "text": text if shown is None else shown, **({"files": files} if files else {})})
            if not self.client:
                self.client = ClaudeSDKClient(self._options())
                await self.client.connect()
                self.rec["started"] = True
                self.reader = asyncio.create_task(self._read(self.client))
            self.stopping = False
            self._set("running", question=None)
            await self.client.query(text)

    async def stop(self):
        if not self.client and self.rec["state"] == "asking":
            # Asking after a restart: no process runs, so stopping only ends the wait.
            self._set("stopped", summary="Stopped.", question=None)
            self.pool.report(self, "stopped", "")
            return
        if self.client and self.rec["state"] in ACTIVE:
            self.stopping = True
            await self.client.interrupt()
            if (self.tasks or self.grace) and self.rec["state"] == "running":
                # No turn runs while it only waits for subagents (or after a trivial turn), so no result will say it stopped.
                self.tasks.clear()
                if self.grace:
                    self.grace.cancel()
                    self.grace = None
                self.stopping = False
                self._set("stopped", summary="Stopped.")
                self.pool.report(self, "stopped", "")

    async def _read(self, client: ClaudeSDKClient):
        try:
            async for msg in client.receive_messages():
                self._handle(msg)
            if self.rec["state"] in ACTIVE:
                # The stream ended without a result: the claude process is gone.
                self._set("failed", summary="The session's process exited.")
                self.pool.report(self, "failed", "The session's process exited mid-turn (crashed or killed).")
        except asyncio.CancelledError:
            raise
        except Exception as e:
            log.exception("worker %s failed", self.name)
            if self.rec["state"] in ACTIVE:
                self._set("failed", summary=f"Session error: {e}")
                self.pool.report(self, "failed", f"Session error: {e}")
        finally:
            if self.client is client:
                self.client = None
            try:
                await client.disconnect()
            except Exception:
                pass

    def _handle(self, msg):
        if isinstance(msg, TaskStartedMessage):
            if "bash" not in (msg.task_type or "") and "shell" not in (msg.task_type or ""):
                self.tasks[msg.task_id] = msg.description or "a subagent"
        elif isinstance(msg, TaskNotificationMessage):
            self.tasks.pop(msg.task_id, None)
        elif isinstance(msg, AssistantMessage):
            if msg.parent_tool_use_id:
                return
            if self.quiet and any(isinstance(b, TextBlock) and b.text.startswith(EFFORT_SET) for b in msg.content):
                return
            if any(isinstance(b, ToolUseBlock) or isinstance(b, TextBlock) and b.text.strip() not in TRIVIAL for b in msg.content):
                self._turn_goes_on()
            for b in msg.content:
                if isinstance(b, TextBlock) and b.text.strip():
                    self._add({"type": "assistant", "text": b.text})
                    if b.text.strip() in TRIVIAL:
                        continue
                    self.last_text = b.text
                    self.turn_text = True
                    self._doing({"text": first_line(b.text)})
                elif isinstance(b, ToolUseBlock):
                    self._step(b)
                    summary = summarize(b.input)
                    self._add({"type": "tool", "tool_id": b.id, "name": b.name, "summary": summary})
                    self._doing({"name": b.name, "summary": summary})
        elif isinstance(msg, UserMessage):
            if msg.parent_tool_use_id or isinstance(msg.content, str):
                return
            for b in msg.content:
                if isinstance(b, ToolResultBlock):
                    if (subject := self.steps.pop(b.tool_use_id, None)) is not None and not b.is_error and self.pool.board:
                        if m := STEP.search(result_text(b.content)):
                            self.pool.board.step(self.id, m[1], title=subject)
                    self._add({"type": "tool_result", "tool_id": b.tool_use_id, "error": bool(b.is_error),
                               "text": result_text(b.content)})
        elif isinstance(msg, ResultMessage):
            if self.quiet and isinstance(msg.result, str) and msg.result.startswith(EFFORT_SET):
                self.quiet -= 1
                return
            self._finish(msg)
            if self.rec.get("effort_next") and self.rec["state"] != "running":
                asyncio.ensure_future(self._apply_effort())
        elif isinstance(msg, RateLimitEvent):
            self.pool.dispatcher.rate_limit(msg)

    def _step(self, b: ToolUseBlock):
        """Its own todo list, mirrored onto its task: a TaskCreate gets its id from the result; a TaskUpdate applies now."""
        if b.name == "TaskCreate":
            self.steps[b.id] = str(b.input.get("subject") or "").strip()
        elif b.name == "TaskUpdate" and self.pool.board and b.input.get("taskId") is not None:
            self.pool.board.step(self.id, str(b.input["taskId"]).lstrip("#"), title=b.input.get("subject") or None,
                                 state=b.input.get("status"))

    def _turn_goes_on(self):
        """A new turn after the worker settled (its queued message, a subagent's report): it runs again."""
        if self.grace:
            self.grace.cancel()
            self.grace = None
        if self.rec["state"] in ("done", "failed"):
            if self.idle_timer:
                self.idle_timer.cancel()
                self.idle_timer = None
            self._set("running", question=None)

    def _finish(self, msg: ResultMessage):
        if self.pool.dispatcher and self.pool.dispatcher.usage:
            fresh = not self.rec.get("turns")
            self.rec["turns"] = self.rec.get("turns", 0) + 1
            self.pool.dispatcher.usage.turn(f"shikigami {self.name}", msg.session_id, msg.model_usage, fresh)
        said = msg.result.strip() if isinstance(msg.result, str) else ""
        trivial = said == "No response requested." or not said and not self.turn_text
        self.turn_text = False
        if not trivial and said:
            self.last_text = said
        text = (said or self.last_text).strip()
        if self.rec["state"] == "asking" or trivial and self.rec["state"] in ("done", "failed", "stopped"):
            pass
        elif self.stopping:
            self._set("stopped", summary="Stopped.")
            self.pool.report(self, "stopped", "")
        elif self.tasks and not msg.is_error:
            # Background subagents report back with a new turn; the report waits for that one.
            n = len(self.tasks)
            self._set("running", doing={"text": f"Waiting for {n} subagent{'s' if n > 1 else ''}"})
            return
        elif msg.is_error:
            self._set("failed", summary=first_line(text) or msg.subtype)
            self.pool.report(self, "failed", text or msg.subtype)
        elif trivial and self.rec["state"] == "running":
            # Nothing said: most likely a replayed notification ahead of the user's message, so no report yet.
            self.grace = asyncio.get_running_loop().call_later(TRIVIAL_GRACE, self._settle)
            return
        else:
            self._set("done", summary=first_line(text), result=text[:4000])
            self.pool.report(self, "done", text)
        self._idle()

    def _settle(self):
        """No turn followed a trivial one: done, with the last thing it said."""
        self.grace = None
        if self.rec["state"] == "running" and not self.tasks:
            text = self.last_text.strip()
            self._set("done", summary=first_line(text), result=text[:4000])
            self.pool.report(self, "done", text)
            self._idle()

    def _idle(self):
        self.stopping = False
        loop = asyncio.get_running_loop()
        self.idle_timer = loop.call_later(IDLE_DISCONNECT, lambda: asyncio.ensure_future(self._park()))

    async def _park(self):
        """Close an idle session's process; the next message resumes it."""
        async with self.lock:
            if self.client and self.rec["state"] not in ACTIVE:
                self.client = None
                self.reader.cancel()


class Pool:
    # The task board (tasks.Board), when there is one: summon and adopt file tasks there, and those follow the workers.
    board = None
    # The enabled plugins (plugins.Plugins): their env, the groups summon grants, and the window a session opens in.
    plugins = None
    # The machine, for the shikigami prompt ("their <about>"); config [host] about.
    about = "computer"
    # A released or lent session opens in this terminal ({cwd} and {title} filled in, quoted); config [sessions] terminal.
    terminal = "kitty -d {cwd} --title {title}"
    # The shikigami permission mode from config ([approvals] shikigami): their default and ceiling.
    mode = "default"

    def __init__(self, events: EventLog, state_dir: Path):
        self.events = events
        self.dir = state_dir / "workers"
        self.dir.mkdir(exist_ok=True)
        self.path = state_dir / "workers.json"
        self.dispatcher = None
        self.guard = None
        self._batch: dict[str, tuple[str, bool, int]] = {}
        self._flush_at: asyncio.TimerHandle | None = None
        recs = json.loads(self.path.read_text()) if self.path.exists() else []
        self.workers: dict[str, Worker] = {r["id"]: Worker(self, r) for r in recs}
        self.archive = Archive(state_dir / "archive.json")
        self.aliases = Aliases(state_dir / "aliases.json")
        self._hidden: set[str] = set()

    def startup(self) -> list[str]:
        """Workers that were running when seimei stopped lost their turn: each is resumed with a note to check its
        state first (one never started gets its task again). An asking one had ended its turn already: it keeps
        waiting, and the answer resumes its session. Returns the names resumed."""
        now = time.time()
        resumed = []
        for w in self.workers.values():
            if w.rec["state"] != "running":
                continue
            w.log.close_open_tools()
            recent = [t for t in w.rec.get("resumes", []) if now - t < RESUME_WINDOW]
            if len(recent) >= RESUME_CAP:
                w._set("interrupted", summary="Interrupted by a seimei restart.")
                self.report(w, "interrupted", "")
                continue
            w.rec["resumes"] = recent + [now]
            resumed.append(w)
        if resumed:
            asyncio.get_running_loop().create_task(self._resume_all(resumed))
        return [w.name for w in resumed]

    async def _resume_all(self, ws: list[Worker]):
        """Resume them together, then say once which ones went on (a failed one reports as interrupted)."""
        ok = await asyncio.gather(*(self._resume(w) for w in ws))
        names = ", ".join(w.name for w, good in zip(ws, ok) if good)
        if names:
            self.events.add({"type": "system", "text": f"Resumed after the restart: {names}."})
            self.notify(f"seimei restarted; resumed shikigami {names} (told to check their state first)")

    async def _resume(self, w: Worker) -> bool:
        try:
            if w.rec.get("started"):
                # A deploy ends with this restart: whoever started it hears how it went.
                await deploys.settled(self.path.parent)
                said = deploys.note(self.path.parent)
                await w.send(f"{RESUME}. {said}" if said else RESUME)
            else:
                await w.send(w.rec.get("task", ""), shown="(its task again: seimei restarted before it began)")
            return True
        except Exception as e:
            log.warning("resuming %s failed: %s", w.name, e)
            w._set("interrupted", summary="Interrupted by a seimei restart.")
            self.report(w, "interrupted", "")
            return False

    def tell(self, worker: str, line: str, urgent: bool = False, key: str | None = None):
        """One event line for the dispatcher about a shikigami. Lines that come close together go as one
        message; repeats collapse. Urgent ones go now (into a running turn, or as a turn of their own); the rest
        wait for its next turn (Dispatcher.note, at most HOLD)."""
        w = self.workers.get(worker)
        self.notify(f"shikigami {w.name} ({w.id}) {line}" if w else line, urgent=urgent, key=key)

    def notify(self, line: str, urgent: bool = False, key: str | None = None):
        """An event line for the dispatcher, batched with the others (see tell)."""
        k = key or line
        old = self._batch.pop(k, None)
        self._batch[k] = (line, urgent or bool(old and old[1]), (old[2] + 1) if old and not key else 1)
        if not self._flush_at:
            self._flush_at = asyncio.get_running_loop().call_later(BATCH, self._flush)

    def _flush(self):
        self._flush_at = None
        batch, self._batch = list(self._batch.items()), {}
        if not batch or not self.dispatcher:
            return
        lines = [line + (f" (×{n})" if n > 1 else "") for _, (line, _, n) in batch]
        # Only what needs the dispatcher starts a turn (or joins the running one); info-only lines wait for its next
        # turn even while it is busy, since a line that reaches a turn just as it ends starts a turn of its own.
        if any(u for _, (_, u, _) in batch):
            self.dispatcher.wake("\n".join(lines) if len(lines) == 1 else "Events:\n" + "\n".join(f"- {l}" for l in lines))
        else:
            for k, (line, _, n) in batch:
                self.dispatcher.note(line + (f" (×{n})" if n > 1 else ""), key=f"w:{k}")

    async def close(self):
        """On shutdown: end every worker process. Running ones are marked interrupted on the next start."""
        readers = [w.reader for w in self.workers.values() if w.client and w.reader]
        for r in readers:
            r.cancel()
        await asyncio.gather(*readers, return_exceptions=True)

    def changed(self):
        write_json(self.path, [w.rec for w in self.workers.values()])
        self.events.flash(self.listing())

    def set_archived(self, w: Worker, on: bool):
        self.archive.set(f"w:{w.id}", on)
        self.events.flash(self.listing())

    async def sweep(self, every: float = 300):
        """Auto-archiving needs no event: tell the pages when a shikigami's 12 hours run out."""
        while True:
            hidden = {w.id for w in self.workers.values() if self.archive.worker(w.rec)}
            if hidden != self._hidden:
                self._hidden = hidden
                self.events.flash(self.listing())
            await asyncio.sleep(every)

    def listing(self) -> dict:
        now = time.time()
        recent = [{**w.info(), **w.brief()} for w in self.workers.values()
                  if w.rec["state"] != "released" and (w.rec["state"] in ACTIVE or now - w.rec.get("updated", 0) < RECENT)]
        return {"type": "workers", "list": recent, "total": len(self.workers)}

    def own_sessions(self) -> dict[str, str]:
        """seimei's own sessions by session id, so listings can tell them from the user's."""
        # A released one is the user's: it shows with their sessions.
        labels = {w.rec["session_id"]: f"shikigami {w.name}" for w in self.workers.values() if w.rec["state"] != "released"}
        sid = self.dispatcher.session_id() if self.dispatcher else None
        if sid:
            labels[sid] = "seimei (you)"
        return labels

    def resume_argv(self, w) -> list[str]:
        """The command that resumes a shikigami's session in a terminal, as the plugins run it (claude-queue)."""
        argv = ["claude", "--resume", w.rec["session_id"]]
        return self.plugins.wrap(w.name, argv) if self.plugins else argv

    async def open_terminal(self, w, title: str, extra: list[str]) -> dict | None:
        cmd = (self.terminal.format(cwd=shlex.quote(w.rec["cwd"]), title=shlex.quote(title)) + " "
               + shlex.join(self.resume_argv(w) + extra))
        if self.plugins:
            return await self.plugins.spawn(cmd)
        from .plugins import spawn_detached
        return spawn_detached(cmd)

    def running(self) -> int:
        return sum(w.rec["state"] in ACTIVE for w in self.workers.values())

    def find(self, key: str) -> Worker | None:
        key = key.strip()
        if key in self.workers:
            return self.workers[key]
        named = [w for w in self.workers.values() if w.name == key]
        return max(named, key=lambda w: w.rec["created"]) if named else None

    def _file(self, w: Worker, title: str, project: str | None, link: str | None):
        """Its task on the board: the one seimei named, or a new one (project from its folder unless given)."""
        if self.board:
            cwd = w.rec["cwd"]
            project = project or sessions.project_of(cwd + "/x") or ("home" if cwd == str(Path.home()) else Path(cwd).name)
            t = self.board.attach(w.id, w.name, w.rec["task"], project, title, link)
            w.rec["board_task"] = t["id"]

    async def summon(self, name: str, task: str, cwd: str, model: str, effort: str | None = None, context: str = "400k",
                     subagents: str = "haiku", project: str | None = None, link: str | None = None,
                     tools: list[str] | None = None, mode: str | None = None) -> Worker:
        name = slug(name)
        if mode and not permissions.within(mode, self.mode):
            raise ValueError(f"mode {mode} lets through more than the shikigami mode set in config ({self.mode}); "
                             "only the user can raise it, on the Agents page")
        path = Path(os.path.expanduser(cwd or "~")).resolve()
        if not path.is_dir():
            raise ValueError(f"no such folder: {path}")
        if effort and effort not in EFFORTS:
            raise ValueError(f"effort is one of {', '.join(EFFORTS)}")
        if context not in CONTEXTS:
            raise ValueError(f"context is one of {', '.join(CONTEXTS)}")
        now = time.time()
        rec = {"id": uuid.uuid4().hex[:8], "name": name, "task": task, "cwd": str(path), "model": model or "opus",
               "effort": effort or None, "context_cap": context, "subagents": subagents or "haiku", "session_id": str(uuid.uuid4()), "state": "starting", "created": now, "updated": now, "summary": ""}
        if mode:
            rec["mode"] = mode
        if self.board and link and not self.board.find(link):
            raise ValueError(f"no task {link} on the board")
        if tools:
            _, unknown = self.plugins.grantable(tools) if self.plugins else ([], list(tools))
            if unknown:
                raise ValueError(f"no such tool group: {', '.join(unknown)} (enabled: {', '.join(self.plugins.names) if self.plugins else 'none'})")
            rec["tools"] = list(dict.fromkeys(tools))
        w = Worker(self, rec)
        self.workers[rec["id"]] = w
        self._file(w, title_of(task, name), project, link)
        self.events.add({"type": "worker", "worker": w.id, "name": name, "event": "summoned", "text": task,
                         "cwd": str(path), "model": rec["model"], "effort": rec["effort"], "context_cap": context,
                         "subagents": rec["subagents"], **({"tools": rec["tools"]} if rec.get("tools") else {})})
        await w.send(task)
        return w

    async def adopt(self, name: str, session_id: str, cwd: str, model: str, effort: str | None, context: str, origin: str,
                    first: str, project: str | None = None, link: str | None = None) -> Worker:
        """A shikigami that resumes one of the user's sessions (adopt.py ended its process). One released earlier
        comes back as itself: same id, name and log, with what was done in the session meanwhile."""
        if old := self.released(session_id, cwd):
            return await self._readopt(old, session_id, model, effort, context, origin, first, project, link)
        now = time.time()
        rec = {"id": uuid.uuid4().hex[:8], "name": slug(name), "task": f"Adopted from your session \"{origin}\".", "cwd": cwd,
               "model": model, "effort": effort, "context_cap": context, "subagents": "haiku", "session_id": session_id,
               "state": "starting", "created": now, "updated": now, "summary": "", "started": True, "adopted": origin}
        w = Worker(self, rec)
        self.workers[rec["id"]] = w
        try:
            self._file(w, origin, project, link)
        except KeyError:
            # The session is adopted already: a wrong task id only costs the link.
            self._file(w, origin, project, None)
        self.events.add({"type": "worker", "worker": w.id, "name": w.name, "event": "summoned", "text": rec["task"],
                         "cwd": cwd, "model": model, "effort": effort, "context_cap": context, "subagents": "haiku"})
        await w.send(first)
        return w

    def released(self, session_id: str, cwd: str) -> Worker | None:
        """The released shikigami this session is (Claude Code may have moved it on to a new transcript)."""
        now = sessions.transcript({"sessionId": session_id, "cwd": cwd})
        for w in self.workers.values():
            if w.rec["state"] == "released" and (w.rec["session_id"] == session_id or now and sessions.transcript(
                    {"sessionId": w.rec["session_id"], "cwd": w.rec["cwd"]}) == now):
                return w
        return None

    async def _readopt(self, w: Worker, session_id: str, model: str, effort: str | None, context: str, origin: str,
                       first: str, project: str | None, link: str | None) -> Worker:
        r = w.rec
        gone = r.get("released") or {}
        path = sessions.transcript({"sessionId": session_id, "cwd": r["cwd"]})
        if path:
            # What the user and the session did on the server, so the page reads on without a gap.
            start = gone.get("offset", 0) if gone.get("path") == str(path) else 0
            try:
                tl = await asyncio.to_thread(sessions.timeline, path, start)
            except OSError:
                tl = {"events": []}
            if tl["events"]:
                w._add({"type": "system", "text": f"In your session \"{origin}\" on the server:"})
                for ev in tl["events"]:
                    w._add({k: v for k, v in ev.items() if k not in ("id", "t")})
        r.update(session_id=session_id, model=model or r["model"], effort=effort or r.get("effort"),
                 context_cap=context or r.get("context_cap"), started=True, released=None, laptop_window=None,
                 adopted=origin, question=None)
        w.tasks.clear()
        w.steps.clear()
        cur = self.board.current(w.id) if self.board else None
        try:
            self._file(w, origin, project, link or (cur and cur["id"]))
        except KeyError:
            self._file(w, origin, project, cur and cur["id"])
        self.events.add({"type": "worker", "worker": w.id, "name": w.name, "event": "summoned",
                         "text": f"Back from your session \"{origin}\".", "cwd": r["cwd"], "model": r["model"],
                         "effort": r.get("effort"), "context_cap": r.get("context_cap"), "subagents": r.get("subagents")})
        w._set("starting", summary="")
        await w.send(first)
        return w

    def report(self, w: Worker, event: str, text: str):
        """A card in the conversation; the dispatcher hears about it now (a stop: at its next turn)."""
        self.events.add({"type": "worker", "worker": w.id, "name": w.name, "event": event, "text": text[:4000]})
        line = {"done": f"finished: {first_line(text, 300)}", "failed": f"failed: {first_line(text, 300)}",
                "asking": f"asks: {text[:1000]}", "stopped": "was stopped",
                "interrupted": "was interrupted by a seimei restart"}[event]
        # Everything but a stop the user asked for wakes the dispatcher, so it can follow up on its own.
        self.tell(w.id, line, urgent=event != "stopped", key=f"report:{w.id}")
