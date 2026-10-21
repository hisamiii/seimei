"""Watch rules the dispatcher registers: when something happens, ping the user with its text.

A rule names a trigger and a target, the ping (title, body, level, key, reply) and an expiry. It fires once unless
repeat is set; the dispatcher hears that it fired, and that it expired without firing. {name} and {detail} in the
title or body become who it was about and what happened (a shikigami's summary, a session's question)."""

import asyncio
import json
import logging
import time
import uuid
from pathlib import Path

from .util import mins

log = logging.getLogger("seimei.watches")

TRIGGERS = {
    "shikigami_done": "a shikigami (target: its name, or * for any) finishes its task",
    "shikigami_failed": "a shikigami fails",
    "shikigami_asks": "a shikigami asks a question",
    "shikigami_ends": "a shikigami ends in any way: done, failed, stopped, interrupted",
    "session_asks": "one of the user's sessions (target: its name, or *) asks a question or waits on a prompt",
    "session_idle": "a session finishes its turn (busy -> idle)",
}
DEFAULT_HOURS = 24
MAX_HOURS = 7 * 24
TICK = 15


class Watches:
    def __init__(self, path: Path, push, tell, plugins=None):
        self.path = path
        self.push = push
        self.tell = tell  # (line) -> the dispatcher
        self.plugins = plugins  # their triggers (the queue's) come on top of TRIGGERS
        try:
            self.rules: list[dict] = json.loads(path.read_text())
        except (OSError, ValueError):
            self.rules = []
        self.workers: dict[str, str] = {}
        self.sessions: dict[str, tuple[str, str]] = {}

    def triggers(self) -> dict[str, str]:
        return {**TRIGGERS, **(self.plugins.triggers if self.plugins else {})}

    def _save(self):
        tmp = self.path.with_suffix(".tmp")
        tmp.write_text(json.dumps(self.rules, indent=1))
        tmp.rename(self.path)

    def add(self, trigger: str, target: str, title: str, body: str = "", level: str = "normal", key: str | None = None,
            reply: bool = False, hours: float = DEFAULT_HOURS, repeat: bool = False) -> dict:
        if trigger not in self.triggers():
            raise ValueError(f"unknown trigger {trigger!r}; one of {', '.join(self.triggers())}")
        if self.plugins:
            self.plugins.check_trigger(trigger, (target or "*").strip())
        rule = {"id": "w" + uuid.uuid4().hex[:5], "trigger": trigger, "target": (target or "*").strip(),
                "title": title[:120], "body": body[:600], "level": level, "key": key, "reply": reply,
                "repeat": repeat, "created": time.time(),
                "expires": time.time() + 3600 * min(max(float(hours or DEFAULT_HOURS), 0.05), MAX_HOURS)}
        self.rules.append(rule)
        self._save()
        return rule

    def remove(self, rid: str) -> bool:
        n = len(self.rules)
        self.rules = [r for r in self.rules if r["id"] != rid]
        self._save()
        return len(self.rules) < n

    def listing(self) -> str:
        if not self.rules:
            return "No watch rules."
        now = time.time()
        return "\n".join(f"{r['id']}: {r['trigger']} {r['target']} -> {r['level']} ping \"{r['title']}\""
                         + (f" (key {r['key']})" if r.get("key") else "") + (", reply" if r.get("reply") else "")
                         + (", repeats" if r.get("repeat") else "") + f", expires in {mins(r['expires'] - now)}"
                         for r in self.rules)

    @staticmethod
    def _hits(rule: dict, name: str) -> bool:
        t = rule["target"]
        return t in ("", "*") or t.lower() == name.lower()

    def fire(self, trigger: str, name: str, detail: str = "", rules: list[dict] | None = None):
        """Ping for each rule of this trigger that names this target (or the rules given)."""
        for r in rules if rules is not None else [r for r in self.rules if r["trigger"] == trigger and self._hits(r, name)]:
            fill = lambda s: s.replace("{name}", name).replace("{detail}", detail[:300])  # noqa: E731
            n = self.push.ping(fill(r["title"]), fill(r["body"]), r["level"], r.get("key"), r.get("reply", False))
            if not r.get("repeat"):
                self.rules = [x for x in self.rules if x["id"] != r["id"]]
            self._save()
            self.tell(f"watch {r['id']} fired ({trigger} {name}): pinged the user \"{fill(r['title'])}\" "
                      f"to {n} phone(s)" + ("" if r.get("repeat") else "; the rule is done"))

    def handle(self, ev: dict):
        """Worker and session listings from the event stream: fire on state changes, never on first sight."""
        kind = ev.get("type")
        if kind == "workers":
            for w in ev.get("list", []):
                name, state, was = w.get("name", ""), w.get("state", ""), self.workers.get(w.get("id", ""))
                self.workers[w.get("id", "")] = state
                if was is None or was == state:
                    continue
                detail = w.get("question") if state == "asking" else w.get("summary") or ""
                if state == "asking":
                    self.fire("shikigami_asks", name, detail or "")
                if state in ("done", "failed", "stopped", "interrupted"):
                    if state in ("done", "failed"):
                        self.fire(f"shikigami_{state}", name, detail or "")
                    self.fire("shikigami_ends", name, f"{state}: {detail}" if detail else state)
        elif kind == "sessions":
            for s in ev.get("list", []):
                sid = str(s.get("session_id") or s.get("pid"))
                q = s.get("question") if isinstance(s.get("question"), str) else ""
                status = s.get("status", "")
                asks = q or (s.get("waiting_text") or "Waiting on the server" if status.startswith("waiting") else "")
                was = self.sessions.get(sid)
                self.sessions[sid] = (status, asks)
                if was is None:
                    continue
                if asks and not was[1]:
                    self.fire("session_asks", s.get("name", ""), asks.splitlines()[0])
                if was[0] == "busy" and status == "idle":
                    self.fire("session_idle", s.get("name", ""), "")

    async def tick(self):
        now = time.time()
        for r in [r for r in self.rules if r["expires"] <= now]:
            self.rules.remove(r)
            self._save()
            self.tell(f"watch {r['id']} ({r['trigger']} {r['target']}) expired without firing")
        used = {r["trigger"] for r in self.rules} - set(TRIGGERS)
        if used and self.plugins:
            await self.plugins.watch(self, used)

    async def run(self):
        while True:
            try:
                await self.tick()
            except Exception:
                log.exception("watch tick failed")
            await asyncio.sleep(TICK)


def make_watch_tools(watches: Watches) -> list:
    from claude_agent_sdk import tool

    def text(t: str, error: bool = False) -> dict:
        return {"content": [{"type": "text", "text": t}], "is_error": error}

    @tool("watch", "Watch rules: ping the user when something happens, without you waiting for it. action add "
          "(trigger, target, title, body, level, key, reply, hours, repeat), list, or remove (id). Triggers: "
          + "; ".join(f"{k}: {v}" for k, v in watches.triggers().items()) + ". {name} and {detail} in title/body are filled "
          f"in. A rule fires once unless repeat, expires after hours (default {DEFAULT_HOURS}, max {MAX_HOURS}), and "
          "you hear when it fires or expires.", {
              "type": "object",
              "properties": {
                  "action": {"type": "string", "enum": ["add", "list", "remove"]},
                  "trigger": {"type": "string", "enum": list(watches.triggers())},
                  "target": {"type": "string", "description": "shikigami or session name, * for any"
                             + ("; minutes for queue_wait" if "queue_wait" in watches.triggers() else "")},
                  "title": {"type": "string"}, "body": {"type": "string"},
                  "level": {"type": "string", "enum": ["silent", "normal", "urgent"], "default": "normal"},
                  "key": {"type": "string", "description": "collapse key"},
                  "reply": {"type": "boolean", "description": "the tap opens the chat at your latest message"},
                  "hours": {"type": "number", "default": DEFAULT_HOURS},
                  "repeat": {"type": "boolean", "default": False},
                  "id": {"type": "string", "description": "for remove"},
              },
              "required": ["action"]})
    async def watch(args):
        action = args.get("action")
        if action == "list":
            return text(watches.listing())
        if action == "remove":
            return text(f"Removed {args.get('id')}." if watches.remove(str(args.get("id", ""))) else "No such rule.",
                        error=False)
        if action == "add":
            try:
                r = watches.add(str(args.get("trigger", "")), str(args.get("target") or "*"), str(args.get("title") or ""),
                                str(args.get("body") or ""), str(args.get("level") or "normal"), args.get("key") or None,
                                bool(args.get("reply")), float(args.get("hours") or DEFAULT_HOURS), bool(args.get("repeat")))
            except (ValueError, TypeError) as e:
                return text(f"Could not add: {e}", error=True)
            if not r["title"]:
                watches.remove(r["id"])
                return text("A rule needs a title for its ping.", error=True)
            return text(f"Added {r['id']}: {r['trigger']} {r['target']}, expires in {mins(r['expires'] - time.time())}.")
        return text("action is add, list or remove.", error=True)

    return [watch]
