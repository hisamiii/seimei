"""Plugins: optional groups of tools on top of seimei's core, each enabled in config.toml and self-contained.

    [plugins]
    enabled = ["desktop", "discord", "browser", "music", "queue", "mytool"]

    [plugins.queue]            # a built-in plugin's own keys
    lib = "~/.local/lib/claude-queue"

    [plugins.mytool]           # anything else: an external MCP server, as Claude Code's mcpServers takes it
    command = "mytool-mcp"
    args = ["--stdio"]
    prompt = "## My tool\\nWhat it is for and its rules."

Each plugin is one MCP server named after it (its tools are mcp__<name>__<tool>), built fresh for every session that
gets it so its approval cards name the right session; the backends (a desktop, a socket to a connector) are shared.
seimei always has every enabled plugin; a shikigami gets the ones summon grants it (tools: [...]), with the same
guard, cards and rules (the plugin's prompt section comes along). With no plugins seimei is the core: the dispatcher,
shikigami, tasks, notes, cards, the guard, notifications, sessions and adopt."""

import importlib
import logging
import os
import shlex
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

from claude_agent_sdk import create_sdk_mcp_server

from .. import approvals
from ..util import always_load

log = logging.getLogger("seimei.plugins")

BUILTIN = ("desktop", "discord", "browser", "music", "queue")
# What an external entry may pass on to Claude Code (its mcpServers format).
MCP_KEYS = ("type", "command", "args", "env", "url", "headers")


def spawn_detached(command: str) -> dict:
    """Start a command on its own, outside seimei (it opens wherever seimei's environment points it)."""
    env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE")}
    proc = subprocess.Popen(shlex.split(command), env=env, start_new_session=True, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return {"id": None, "pid": proc.pid}


@dataclass
class Host:
    """What seimei hands a plugin."""
    events: object
    pool: object
    files: object
    uploads: object
    approvals: object
    state: Path
    dev: bool = False
    note: Callable[[str], None] = lambda text: None
    config: dict = field(default_factory=dict)


class Plugin:
    name = ""
    # Tools that stay behind tool search; the rest load up front.
    deferred: frozenset[str] = frozenset()
    # The dispatcher's prompt section for this group; a shikigami granted the group gets it too.
    prompt = ""
    # Added to every shikigami's prompt while the plugin is on, granted or not (the queue wraps every session).
    worker_note = ""
    # Watch rule triggers it adds: name -> what fires it.
    triggers: dict[str, str] = {}

    def __init__(self, host: Host, cfg: dict):
        self.host, self.cfg = host, cfg

    def tools(self, who: str, worker: str | None) -> list:
        """The tools, built for one session: who and worker go on the approval cards they raise."""
        return []

    def server(self, who: str = "seimei", worker: str | None = None):
        return always_load(create_sdk_mcp_server(self.name, tools=self.tools(who, worker)), set(self.deferred))

    def check(self, tool: str, inp: dict) -> dict | None:
        """A tool call (any session's) that needs the user's OK: {action, target, detail}, else None."""
        return None

    def env(self, name: str) -> dict:
        """Environment for a session named name (seimei's own and each shikigami's)."""
        return {}

    def wrap(self, name: str, argv: list[str]) -> list[str]:
        """How a session's own command runs when it is opened in a terminal (a release or a loan)."""
        return argv

    async def spawn(self, command: str) -> dict | None:
        """Open a terminal command on the desktop: the window ({"id": ...}) or None. NotImplemented: not this one."""
        return NotImplemented

    def window_open(self, wid) -> bool | None:
        """Whether a window spawn() opened is still there; None: cannot tell."""
        return None

    def halt(self) -> str:
        """The stop key: stop what it drives; returns a line for the dispatcher (or "")."""
        return ""

    def check_trigger(self, trigger: str, target: str):
        """Raise ValueError when a watch rule's target makes no sense for one of this plugin's triggers."""

    async def watch(self, watches):
        """Every watch tick, while some rule uses one of its triggers: fire them (watches.fire)."""

    def routes(self, app):
        """Extra HTTP routes (behind the login like every route)."""

    def tasks(self) -> list:
        """Coroutines to run while seimei runs."""
        return []

    async def close(self):
        pass


class External(Plugin):
    """An MCP server from the config: Claude Code starts it per session; seimei adds its prompt and the guard."""

    def __init__(self, host: Host, name: str, cfg: dict):
        super().__init__(host, cfg)
        self.name = name
        self.mcp = {k: cfg[k] for k in MCP_KEYS if k in cfg}
        if "url" in self.mcp:
            self.mcp.setdefault("type", "http")
        self.prompt = str(cfg.get("prompt") or "")

    def server(self, who: str = "seimei", worker: str | None = None):
        return dict(self.mcp)


class Plugins:
    def __init__(self, host: Host, cfg: dict):
        self.host = host
        self.list: list[Plugin] = []
        for name in cfg.get("enabled") or []:
            own = cfg.get(name) if isinstance(cfg.get(name), dict) else {}
            try:
                if name in BUILTIN:
                    self.list.append(importlib.import_module(f"{__name__}.{name}").Plugin(host, own))
                elif own.get("command") or own.get("url"):
                    self.list.append(External(host, name, own))
                else:
                    log.warning("plugin %s: not built in, and its [plugins.%s] has no command or url; skipped", name, name)
            except Exception:
                log.exception("plugin %s failed to load; seimei goes on without it", name)
        # Every session's tool calls pass the enabled plugins' card checks (the guard asks approvals.check).
        approvals.CHECKS[:] = [p.check for p in self.list]
        log.info("plugins: %s", ", ".join(self.names) or "none")

    @property
    def names(self) -> list[str]:
        return [p.name for p in self.list]

    def get(self, name: str) -> Plugin | None:
        return next((p for p in self.list if p.name == name), None)

    def grantable(self, groups) -> tuple[list[str], list[str]]:
        """The asked-for groups split into (enabled, unknown)."""
        groups = [str(g) for g in groups or []]
        return [g for g in groups if g in self.names], [g for g in groups if g not in self.names]

    def servers(self, who: str = "seimei", worker: str | None = None, groups=None) -> dict:
        """MCP servers for a session: all plugins (seimei), or the granted groups (a shikigami)."""
        want = self.names if groups is None else [g for g in groups if g in self.names]
        return {p.name: p.server(who, worker) for p in self.list if p.name in want}

    def prompt(self, groups=None) -> str:
        want = self.names if groups is None else list(groups)
        return "\n\n".join(p.prompt.strip() for p in self.list if p.name in want and p.prompt.strip())

    def worker_notes(self) -> str:
        return "\n".join(p.worker_note.strip() for p in self.list if p.worker_note.strip())

    def env(self, name: str) -> dict:
        out = {}
        for p in self.list:
            out.update(p.env(name))
        return out

    def wrap(self, name: str, argv: list[str]) -> list[str]:
        for p in self.list:
            argv = p.wrap(name, argv)
        return argv

    async def spawn(self, command: str) -> dict | None:
        """The first plugin that opens windows does; without one, a plain detached process (the terminal opens wherever
        seimei's environment points it)."""
        for p in self.list:
            w = await p.spawn(command)
            if w is not NotImplemented:
                return w
        return spawn_detached(command)

    def window_open(self, wid) -> bool | None:
        for p in self.list:
            if (r := p.window_open(wid)) is not None:
                return r
        return None

    def halt(self) -> str:
        return " ".join(x for p in self.list if (x := p.halt()))

    @property
    def triggers(self) -> dict[str, str]:
        out = {}
        for p in self.list:
            out.update(p.triggers)
        return out

    def check_trigger(self, trigger: str, target: str):
        for p in self.list:
            if trigger in p.triggers:
                p.check_trigger(trigger, target)

    async def watch(self, watches, used: set[str]):
        for p in self.list:
            if used & set(p.triggers):
                try:
                    await p.watch(watches)
                except Exception:
                    log.exception("plugin %s: watch failed", p.name)

    def routes(self, app):
        for p in self.list:
            p.routes(app)

    def tasks(self) -> list:
        return [t for p in self.list for t in p.tasks()]

    async def close(self):
        for p in self.list:
            try:
                await p.close()
            except Exception:
                log.exception("plugin %s: close failed", p.name)
