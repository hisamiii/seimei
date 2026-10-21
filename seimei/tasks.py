"""The task board (#55): what seimei has queued, who has each item and where it stands, grouped by project. seimei
keeps it current with its tasks tool; the user ticks their own items off on the phone. Pages read it whole from
/api/tasks and follow it live: every change goes out on the event stream as {"type": "tasks", ...} (not stored).

A task: {"id": "#55" (seimei's numbered work) or "t3" (anything else), "title", "project", "owner" (a shikigami's
name, "seimei" or "you"), "state": open | running | paused | done, "done_at", "worker" (a shikigami id, optional),
"created", "updated"}. A project: {"name", "color" (optional; pages tint it like the Agents tab otherwise), "paused"}."""

import time
from pathlib import Path

from .events import EventLog
from .util import read_json, write_json

STATES = ("open", "running", "paused", "done")
YOU = "you"
# How many done tasks the board keeps per project; older ones drop off.
DONE_KEEP = 30


def seed() -> dict:
    """The board as it stood when it was built (2026-10-21), so it starts with today's real work."""
    now = time.time()
    rows = [
        ("#47", "Metrics view", "seimei", "dev", "done", "3905ef87"),
        ("#48", "re: anchor marker", "seimei", "dev", "done", "3905ef87"),
        ("#52", "Remove the Screen tab", "seimei", "dev", "done", "3905ef87"),
        ("#53", "Conversation cache on the phone", "seimei", "dev", "done", "3905ef87"),
        ("#54", "Fold info-only events", "seimei", "dev", "done", "3905ef87"),
        ("#55", "Task board", "seimei", "dev", "running", "3905ef87"),
        ("t1", "Back up the signing key", "seimei", YOU, "open", None),
        ("t2", "M4 step 0 tests", "seimei", YOU, "open", None),
        ("t3", "Font fix", "discord tiled", "discord-tiled", "done", "dc956b55"),
        ("t4", "Material icons", "discord tiled", "discord-tiled", "open", "dc956b55"),
        ("t5", "ASR internal-mic A/B", "desktop", YOU, "open", None),
        ("t6", "Clean up yah", "yah cleanup", "cleanup", "paused", "313245d5"),
        ("t7", "Minecraft harness", "minecraft harness", "mc", "paused", "e468acc5"),
    ]
    tasks = [{"id": i, "title": t, "project": p, "owner": o, "state": s, "done_at": now - 60 * (len(rows) - n) if s == "done" else None,
              "worker": w, "created": now, "updated": now} for n, (i, t, p, o, s, w) in enumerate(rows)]
    projects = [{"name": n, "color": None, "paused": n in ("yah cleanup", "minecraft harness")}
                for n in ("seimei", "discord tiled", "desktop", "yah cleanup", "minecraft harness")]
    return {"next": 56, "other": 8, "tasks": tasks, "projects": projects}


class Board:
    def __init__(self, path: Path, events: EventLog | None = None):
        self.path = path
        self.events = events
        self.data = read_json(path) or seed()
        if not path.exists():
            write_json(path, self.data)

    def snapshot(self) -> dict:
        return {"type": "tasks", "tasks": self.data["tasks"], "projects": self.data["projects"]}

    def _changed(self):
        write_json(self.path, self.data)
        if self.events:
            self.events.flash(self.snapshot())

    def find(self, tid: str) -> dict | None:
        tid = str(tid).strip()
        if tid.isdigit():
            tid = f"#{tid}"
        return next((t for t in self.data["tasks"] if t["id"] == tid), None)

    def project(self, name: str) -> dict:
        p = next((p for p in self.data["projects"] if p["name"] == name), None)
        if not p:
            p = {"name": name, "color": None, "paused": False}
            self.data["projects"].append(p)
        return p

    def add(self, title: str, project: str, owner: str, state: str = "open", worker: str | None = None,
            numbered: bool = True) -> dict:
        if state not in STATES:
            raise ValueError(f"state is one of {', '.join(STATES)}")
        if numbered:
            tid = f"#{self.data['next']}"
            self.data["next"] += 1
        else:
            tid = f"t{self.data['other']}"
            self.data["other"] += 1
        now = time.time()
        self.project(project)
        t = {"id": tid, "title": title, "project": project, "owner": owner, "state": state,
             "done_at": now if state == "done" else None, "worker": worker, "created": now, "updated": now}
        self.data["tasks"].append(t)
        self._trim(project)
        self._changed()
        return t

    def update(self, tid: str, **fields) -> dict:
        t = self.find(tid)
        if not t:
            raise KeyError(f"no task {tid}")
        for k in ("title", "project", "owner", "worker"):
            if fields.get(k) is not None:
                t[k] = fields[k] or None if k == "worker" else fields[k]
        state = fields.get("state")
        if state is not None:
            if state not in STATES:
                raise ValueError(f"state is one of {', '.join(STATES)}")
            if state == "done" and t["state"] != "done":
                t["done_at"] = time.time()
            elif state != "done":
                t["done_at"] = None
            t["state"] = state
        if fields.get("project"):
            self.project(fields["project"])
        t["updated"] = time.time()
        self._trim(t["project"])
        self._changed()
        return t

    def set_project(self, name: str, paused: bool | None = None, color: str | None = None) -> dict:
        p = self.project(name)
        if paused is not None:
            p["paused"] = bool(paused)
        if color is not None:
            p["color"] = color or None
        self._changed()
        return p

    def remove(self, tid: str) -> dict:
        """Drops a task for good (one that will not be done); a project left empty goes with it."""
        t = self.find(tid)
        if not t:
            raise KeyError(f"no task {tid}")
        self.data["tasks"].remove(t)
        self._trim(t["project"])
        self._changed()
        return t

    def tick(self, tid: str, done: bool) -> dict:
        """The user ticks one of their own items off on the phone, or back on."""
        t = self.find(tid)
        if not t:
            raise KeyError(f"no task {tid}")
        if t["owner"] != YOU:
            raise PermissionError("only your own items can be ticked here")
        return self.update(tid, state="done" if done else "open")

    def _trim(self, project: str):
        done = sorted((t for t in self.data["tasks"] if t["project"] == project and t["state"] == "done"),
                      key=lambda t: t["done_at"] or 0)
        drop = {id(t) for t in done[:-DONE_KEEP]}
        if drop:
            self.data["tasks"] = [t for t in self.data["tasks"] if id(t) not in drop]
        # A project with no tasks left goes too.
        names = {t["project"] for t in self.data["tasks"]}
        self.data["projects"] = [p for p in self.data["projects"] if p["name"] in names]

    def text(self) -> str:
        """The board for the dispatcher: open work first per project, then the last few done."""
        out = []
        for p in self.data["projects"]:
            ts = [t for t in self.data["tasks"] if t["project"] == p["name"]]
            live = [t for t in ts if t["state"] != "done"]
            done = sorted((t for t in ts if t["state"] == "done"), key=lambda t: -(t["done_at"] or 0))
            out.append(f"{p['name']}" + (" (paused)" if p["paused"] else "") + (f" [color {p['color']}]" if p["color"] else ""))
            for t in sorted(live, key=lambda t: STATES.index(t["state"]) if t["state"] != "running" else -1):
                out.append(f"  {t['id']} {t['state']}: {t['title']} ({t['owner']}" + (f", shikigami {t['worker']}" if t["worker"] else "") + ")")
            if done:
                out.append("  done: " + "; ".join(f"{t['id']} {t['title']}" for t in done[:5]) + (f" (+{len(done) - 5})" if len(done) > 5 else ""))
        return "\n".join(out) or "The board is empty."


def make_task_tools(board: Board) -> list:
    from claude_agent_sdk import tool

    def text(t: str, error: bool = False) -> dict:
        return {"content": [{"type": "text", "text": t}], "is_error": error}

    @tool("tasks", "The user's task board (the Tasks tab next to Chat | Trajectory), yours to keep current: add an item "
          "when you queue work, set it running when a shikigami starts on it, done when it lands; the user's own "
          "to-dos are owner 'you' (they can tick those off themselves). action: list | add | update | done | remove | project. "
          "add: title, project, owner (a shikigami's name, 'seimei' or 'you'), optional state (open, running, paused, "
          "done; default open), worker (a shikigami id: tapping the task opens its page) and numbered (default true: "
          "the next #NN; false for the user's to-dos and small items). update: id plus any of title, project, owner, "
          "state, worker. done: id. remove: id (drops an item that will not be done; an empty project goes too). project: name plus paused (true shuts it on the board) and/or color (#rrggbb, empty "
          "for the default tint).", {
              "type": "object",
              "properties": {
                  "action": {"type": "string", "enum": ["list", "add", "update", "done", "remove", "project"]},
                  "id": {"type": "string", "description": "#55, 55 or t3"},
                  "title": {"type": "string"}, "project": {"type": "string"}, "owner": {"type": "string"},
                  "state": {"type": "string", "enum": list(STATES)}, "worker": {"type": "string"},
                  "numbered": {"type": "boolean"}, "name": {"type": "string"}, "paused": {"type": "boolean"},
                  "color": {"type": "string"},
              },
              "required": ["action"]})
    async def tasks(a):
        act = a.get("action")
        try:
            if act == "add":
                if not (a.get("title") and a.get("project") and a.get("owner")):
                    return text("add needs title, project and owner.", True)
                t = board.add(a["title"], a["project"], a["owner"], a.get("state") or "open", a.get("worker"),
                              a.get("numbered", True))
                return text(f"Added {t['id']} {t['title']} ({t['project']}, {t['owner']}, {t['state']}).")
            if act in ("update", "done"):
                if not a.get("id"):
                    return text(f"{act} needs an id.", True)
                fields = {k: a.get(k) for k in ("title", "project", "owner", "worker", "state")}
                if act == "done":
                    fields["state"] = "done"
                t = board.update(a["id"], **fields)
                return text(f"{t['id']} {t['title']}: {t['state']} ({t['project']}, {t['owner']}).")
            if act == "remove":
                if not a.get("id"):
                    return text("remove needs an id.", True)
                t = board.remove(a["id"])
                gone = "" if any(p["name"] == t["project"] for p in board.data["projects"]) else f"; {t['project']} is empty and left the board"
                return text(f"Removed {t['id']} {t['title']}{gone}.")
            if act == "project":
                name = a.get("name") or a.get("project")
                if not name:
                    return text("project needs a name.", True)
                if not any(t["project"] == name for t in board.data["tasks"]):
                    return text(f"No tasks in project {name}.", True)
                p = board.set_project(name, a.get("paused"), a.get("color"))
                return text(f"{p['name']}: " + ("paused" if p["paused"] else "active") + (f", color {p['color']}" if p["color"] else ""))
            return text(board.text())
        except (KeyError, ValueError) as e:
            return text(str(e).strip("'"), True)

    return [tasks]
