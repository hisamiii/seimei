"""The task board (#55, #58): what seimei has queued, who has each item and where it stands, grouped by project.
seimei keeps it current with its tasks tool; summon and adopt file a task for each new shikigami, and a linked task's
state follows its shikigami (running, asks, review once it reports). The user ticks their own items off on the phone.
Pages read it whole from /api/tasks and follow it live: every change goes out on the event stream as
{"type": "tasks", ...} (not stored).

A task: {"id": "#55" (seimei's numbered work) or "t3" (anything else), "title", "project", "owner" (a shikigami's
name, "seimei" or "you"), "state" (STATES), "done_at", "worker" (a shikigami id, optional), "linked" (when it was
linked), "wstate" (the shikigami state it last followed), "steps" (the shikigami's own todo list, mirrored:
[{"id", "title", "state": pending | in_progress | completed}]), "pos" (its place in its owner's queue), "since" (when
its state last changed), "created", "updated"}. A project: {"name", "color" (optional; pages tint it like the Agents
tab otherwise), "paused"}."""

import re
import time
from pathlib import Path

from .events import EventLog
from .util import read_json, write_json

STATES = ("open", "running", "asks", "review", "paused", "done")
YOU = "you"
# How many done tasks the board keeps per project; older ones drop off.
DONE_KEEP = 30
# A linked task's state for each shikigami state: it works, waits on a question, or finished and waits for seimei's
# check (failed ones too: seimei looks at those first); a stop or a restart parks it; given to the user, it is theirs
# to carry on (release makes them its owner).
FOLLOW = {"starting": "running", "running": "running", "laptop": "running", "asking": "asks", "done": "review",
          "failed": "review", "stopped": "paused", "interrupted": "paused", "released": "open"}
LIVE = {"starting", "running", "asking", "laptop"}
# A review item older than this shows up in the drift line.
REVIEW_STALE = 2 * 3600
STEP_STATES = ("pending", "in_progress", "completed")
# How live work sorts within a project: what waits on someone first, then what runs, what waits for a check, what is
# queued, what is parked.
ORDER = {"asks": 0, "running": 1, "review": 2, "open": 3, "paused": 4, "done": 5}


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
        # Boards from before #58: each owner's queue in the order the tasks were added.
        last: dict[str, int] = {}
        for t in self.data["tasks"]:
            if "pos" not in t:
                t["pos"] = last[t["owner"]] = last.get(t["owner"], 0) + 1
            last[t["owner"]] = max(last.get(t["owner"], 0), t["pos"])
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

    def _last(self, owner: str) -> int:
        return max((t.get("pos", 0) for t in self.data["tasks"] if t["owner"] == owner), default=0)

    def add(self, title: str, project: str, owner: str, state: str = "open", worker: str | None = None,
            numbered: bool = True, tid: str | None = None) -> dict:
        if state not in STATES:
            raise ValueError(f"state is one of {', '.join(STATES)}")
        if tid:
            # seimei's own number for it (a summon whose task starts with #NN): the count goes on after it.
            if self.find(tid):
                raise ValueError(f"{tid} is on the board already")
            self.data["next"] = max(self.data["next"], int(tid[1:]) + 1)
        elif numbered:
            tid = f"#{self.data['next']}"
            self.data["next"] += 1
        else:
            tid = f"t{self.data['other']}"
            self.data["other"] += 1
        now = time.time()
        self.project(project)
        t = {"id": tid, "title": title, "project": project, "owner": owner, "state": state,
             "done_at": now if state == "done" else None, "worker": worker, "linked": now if worker else None,
             "steps": [], "pos": self._last(owner) + 1, "since": now, "created": now, "updated": now}
        self.data["tasks"].append(t)
        self._trim(project)
        self._changed()
        return t

    def _state(self, t: dict, state: str):
        if state not in STATES:
            raise ValueError(f"state is one of {', '.join(STATES)}")
        if state == t["state"]:
            return
        t["done_at"] = time.time() if state == "done" else None
        t["state"] = state
        t["since"] = time.time()
        if state != "review":
            t.pop("shipped", None)

    def update(self, tid: str, **fields) -> dict:
        t = self.find(tid)
        if not t:
            raise KeyError(f"no task {tid}")
        if fields.get("owner") is not None and fields["owner"] != t["owner"]:
            # To the end of its new owner's queue.
            t["pos"] = self._last(fields["owner"]) + 1
        for k in ("title", "project", "owner"):
            if fields.get(k) is not None:
                t[k] = fields[k]
        if fields.get("worker") is not None and (fields["worker"] or None) != t.get("worker"):
            t["worker"] = fields["worker"] or None
            t["linked"] = time.time() if t["worker"] else None
            t["wstate"] = None
        if fields.get("state") is not None:
            self._state(t, fields["state"])
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

    def order(self, ids: list[str]) -> list[dict]:
        """Puts these tasks first in their owners' queues, in this order; the rest keep theirs after them."""
        picked = []
        for i in ids:
            t = self.find(i)
            if not t:
                raise KeyError(f"no task {i}")
            if t not in picked:
                picked.append(t)
        for owner in {t["owner"] for t in picked}:
            mine = [t for t in picked if t["owner"] == owner]
            rest = sorted((t for t in self.data["tasks"] if t["owner"] == owner and t not in mine), key=lambda t: t.get("pos", 0))
            for n, t in enumerate(mine + rest, 1):
                t["pos"] = n
        self._changed()
        return picked

    # ---------- following the shikigami ----------

    def current(self, worker: str) -> dict | None:
        """The task a shikigami works on: of those linked to it and not done, the one it is followed on (seimei may queue
        its next task on it early: that one takes over once this one is done or shipped), else the one linked last."""
        mine = [t for t in self.data["tasks"] if t.get("worker") == worker and t["state"] != "done"]
        return max(mine, key=lambda t: (not t.get("shipped"), t.get("wstate") is not None, t.get("linked") or t["created"]),
                   default=None)

    def attach(self, worker: str, owner: str, task: str, project: str, title: str, link: str | None = None) -> dict:
        """A new shikigami's task: the one [link] names, or one of its own (seimei's #NN when [task] starts with one).
        Returns the task."""
        if link:
            t = self.find(link)
            if not t:
                raise KeyError(f"no task {link}")
            if t["owner"] != owner:
                t["pos"] = self._last(owner) + 1
            t.update(worker=worker, linked=time.time(), wstate=None, owner=owner, steps=[])
            if t["state"] == "done":
                self._state(t, "open")
            t["updated"] = time.time()
            self._changed()
            return t
        m = re.match(r"\s*#(\d+)\b", task)
        if m and (t := self.find(f"#{m[1]}")) and t["state"] != "done":
            return self.attach(worker, owner, task, project, title, link=t["id"])
        return self.add(title, project, owner, "running", worker, tid=f"#{m[1]}" if m and not self.find(f"#{m[1]}") else None)

    def follow(self, worker: str, wstate: str):
        """A shikigami changed state: its task follows (once per change, so seimei's own edits stand until the next)."""
        t = self.current(worker)
        if not t or t.get("wstate") == wstate:
            return
        t["wstate"] = wstate
        # A shipped item waits for seimei's check: whatever its shikigami does after, it stays in review.
        if (state := FOLLOW.get(wstate)) and state != t["state"] and not t.get("shipped"):
            self._state(t, state)
            t["updated"] = time.time()
        self._changed()

    def _own(self, name: str, tid: str) -> dict:
        t = self.find(tid)
        if not t:
            raise KeyError(f"no task {tid}")
        if t["owner"] != name:
            raise ValueError(f"{t['id']} is {t['owner']}'s, not yours: ask seimei to give it to you")
        if t["state"] == "done":
            raise ValueError(f"{t['id']} is done already")
        return t

    def start(self, worker: str, name: str, wstate: str, tid: str) -> dict:
        """A shikigami moves on to one of its own queued items (#79): that one runs and follows it; what it worked on
        before goes back to the queue, unless it was shipped."""
        t = self._own(name, tid)
        now = time.time()
        for o in self.data["tasks"]:
            if o is not t and o.get("worker") == worker and o["state"] not in ("done", "review"):
                o.update(worker=None, linked=None, wstate=None, updated=now)
                self._state(o, "open")
        t.update(worker=worker, linked=now, wstate=wstate, updated=now)
        self._state(t, FOLLOW.get(wstate) or "running")
        self._changed()
        return t

    def ship(self, worker: str, name: str, tid: str) -> dict:
        """A shikigami deployed or delivered one of its items (#79): it waits in review for seimei's check."""
        t = self._own(name, tid)
        if not t.get("worker"):
            t.update(worker=worker, linked=time.time())
        self._state(t, "review")
        t.update(shipped=True, updated=time.time())
        self._changed()
        return t

    def step(self, worker: str, sid: str, title: str | None = None, state: str | None = None):
        """The shikigami's todo list changed: a step was added (TaskCreate), moved on, renamed or dropped (TaskUpdate)."""
        t = self.current(worker)
        if not t:
            return
        steps = t.setdefault("steps", [])
        s = next((s for s in steps if s["id"] == sid), None)
        if state == "deleted":
            if s:
                steps.remove(s)
        elif s is None:
            if title is None:
                return
            steps.append({"id": sid, "title": title, "state": state if state in STEP_STATES else "pending"})
        else:
            if title:
                s["title"] = title
            if state in STEP_STATES:
                s["state"] = state
        self._changed()

    def drift(self, workers: dict[str, tuple[str, str]], now: float | None = None) -> str | None:
        """Where the board and the shikigami disagree, as one line for seimei; None when they agree. [workers]: id ->
        (name, state)."""
        now = now or time.time()
        out = []
        for wid, (name, state) in workers.items():
            # One open on the server is the user's to drive: it needs no task.
            if state in LIVE - {"laptop"} and not self.current(wid):
                out.append(f"shikigami {name} ({wid}) {'asks' if state == 'asking' else 'works'} with no open task")
        for t in self.data["tasks"]:
            w = t.get("worker")
            if t["state"] in ("running", "asks") and w:
                if w not in workers:
                    out.append(f"{t['id']} is {t['state']} but its shikigami {w} is gone")
                elif workers[w][1] not in LIVE:
                    out.append(f"{t['id']} is {t['state']} but shikigami {workers[w][0]} is {workers[w][1]}")
            if t["state"] == "review" and now - (t.get("since") or t["updated"]) > REVIEW_STALE:
                out.append(f"{t['id']} has waited for review {(now - (t.get('since') or t['updated'])) / 3600:.0f} h")
        if not out:
            return None
        more = f" (+{len(out) - 6} more)" if len(out) > 6 else ""
        return "[seimei] Board drift: " + "; ".join(out[:6]) + more + "."

    def _trim(self, project: str):
        done = sorted((t for t in self.data["tasks"] if t["project"] == project and t["state"] == "done"),
                      key=lambda t: t["done_at"] or 0)
        drop = {id(t) for t in done[:-DONE_KEEP]}
        if drop:
            self.data["tasks"] = [t for t in self.data["tasks"] if id(t) not in drop]
        # A project with no tasks left goes too.
        names = {t["project"] for t in self.data["tasks"]}
        self.data["projects"] = [p for p in self.data["projects"] if p["name"] in names]

    def nexts(self) -> set[str]:
        """Each owner's next item: their first open task in queue order."""
        out = {}
        for t in sorted(self.data["tasks"], key=lambda t: t.get("pos", 0)):
            if t["state"] == "open":
                out.setdefault(t["owner"], t["id"])
        return set(out.values())

    def text(self, done_too: bool = True) -> str:
        """The board for the dispatcher: per project, live work in queue order, then the last few done."""
        nexts = self.nexts()
        out = []
        for p in self.data["projects"]:
            ts = [t for t in self.data["tasks"] if t["project"] == p["name"]]
            live = [t for t in ts if t["state"] != "done"]
            if not live and not done_too:
                continue
            ended = sorted((t for t in ts if t["state"] == "done"), key=lambda t: -(t["done_at"] or 0))
            out.append(f"{p['name']}" + (" (paused)" if p["paused"] else "") + (f" [color {p['color']}]" if p["color"] else ""))
            for t in sorted(live, key=lambda t: (ORDER[t["state"]], t.get("pos", 0))):
                steps = t.get("steps") or []
                out.append(f"  {t['id']} {t['state']}{' (next)' if t['id'] in nexts else ''}: {t['title']} ({t['owner']} #{t.get('pos')}"
                           + (f", shikigami {t['worker']}" if t.get("worker") else "")
                           + (f", {sum(s['state'] == 'completed' for s in steps)}/{len(steps)} steps" if steps else "") + ")")
            if ended and done_too:
                out.append("  done: " + "; ".join(f"{t['id']} {t['title']}" for t in ended[:5]) + (f" (+{len(ended) - 5})" if len(ended) > 5 else ""))
        return "\n".join(out) or "The board is empty."


def make_task_tools(board: Board) -> list:
    from claude_agent_sdk import tool

    def text(t: str, error: bool = False) -> dict:
        return {"content": [{"type": "text", "text": t}], "is_error": error}

    @tool("tasks", "The user's task board (the Tasks tab next to Chat | Trajectory), yours to keep current. summon and "
          "adopt file a task for each new shikigami by themselves (or link the one you name), and a linked task follows "
          "its shikigami: running while it works, asks while it waits on a question, review once it reports (finished or "
          "failed), paused when stopped or interrupted. A shikigami moves along its own queue itself: it starts an item "
          "(running; its previous one goes back to open unless shipped) and ships it (review, and it stays there). "
          "You mark it done after checking the report; the shikigami's own "
          "todo list shows live as the task's steps. The user's own to-dos are owner 'you' (they tick those off "
          "themselves). Each owner's items form a queue: the first open one shows as next. "
          "action: list | add | update | done | remove | order | project. "
          "add: title, project, owner (a shikigami's name, 'seimei' or 'you'), optional state (open, running, asks, "
          "review, paused, done; default open), worker (a shikigami id: tapping the task opens its page; its state "
          "then follows it) and numbered (default true: the next #NN; false for the user's to-dos and small items); "
          "it goes to the end of its owner's queue. update: id plus any of title, project, owner, state, worker. "
          "done: id. remove: id (drops an item that will not be done; an empty project goes too). order: ids, put "
          "first in their owners' queues in that order. project: name plus paused (true shuts it on the board) and/or "
          "color (#rrggbb, empty for the default tint).", {
              "type": "object",
              "properties": {
                  "action": {"type": "string", "enum": ["list", "add", "update", "done", "remove", "order", "project"]},
                  "id": {"type": "string", "description": "#55, 55 or t3"},
                  "ids": {"type": "array", "items": {"type": "string"}, "description": "order: the new front of the queue"},
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
            if act == "order":
                ids = a.get("ids") or ([a["id"]] if a.get("id") else [])
                if not ids:
                    return text("order needs ids.", True)
                owners = dict.fromkeys(t["owner"] for t in board.order([str(i) for i in ids]))
                queue = lambda o: sorted((t for t in board.data["tasks"] if t["owner"] == o and t["state"] != "done"), key=lambda t: t["pos"])
                return text("Queues now: " + "; ".join(f"{o}: " + ", ".join(t["id"] for t in queue(o)) for o in owners) + ".")
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
