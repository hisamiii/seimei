"""The desktop layout, remembered so a new desktop session (a WM restart or a reboot) can be brought back: apps on their
workspaces, kitty windows in their folders, and Claude sessions resumed."""

import asyncio
import logging
import os
import shlex
import time
from pathlib import Path

from . import desktop
from ...sessions import transcript
from ...util import read_json, write_json

log = logging.getLogger("seimei.layout")

SESSIONS = Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude") / "sessions"
EVERY = 30
MAX_AGE = 3 * 86400
SKIP_APPS = {"kitty-quick-access"}
INTERPRETERS = {"python", "python3", "node", "electron", "sh", "bash"}


def _children() -> dict[int, list[int]]:
    kids: dict[int, list[int]] = {}
    for p in Path("/proc").iterdir():
        if p.name.isdigit():
            try:
                ppid = int((p / "stat").read_text().rsplit(")", 1)[1].split()[1])
            except (OSError, ValueError, IndexError):
                continue
            kids.setdefault(ppid, []).append(int(p.name))
    return kids


def _argv(pid: int) -> list[str]:
    try:
        return [a.decode(errors="replace") for a in Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0") if a]
    except OSError:
        return []


def _cwd(pid: int) -> str | None:
    try:
        return os.readlink(f"/proc/{pid}/cwd")
    except OSError:
        return None


def _kitty(pid: int, kids: dict) -> dict:
    """A kitty window's shell folder and the Claude session running in it, if any."""
    out = {}
    stack = list(kids.get(pid, []))
    while stack:
        c = stack.pop(0)
        argv = _argv(c)
        prog = os.path.basename(argv[0]) if argv else ""
        if prog in ("fish", "zsh", "bash", "sh") and "cwd" not in out:
            out["cwd"] = _cwd(c)
        rec = read_json(SESSIONS / f"{c}.json")
        if rec and rec.get("sessionId") and "claude" not in out:
            # A session can continue into a new transcript; resume the newest.
            path = transcript(rec)
            out["claude"] = {"session_id": path.stem if path else rec["sessionId"], "cwd": rec.get("cwd") or _cwd(c),
                             "name": rec.get("name"), "args": [a for a in argv[1:] if a not in ("-r", "--resume", "-c", "--continue")]}
            continue
        stack.extend(kids.get(c, []))
    return out


def snapshot() -> dict:
    b = desktop.backend()
    wins, spaces = b.windows(), b.workspaces()
    ws = {s["id"]: s for s in spaces}
    kids = _children()
    out = []
    for w in wins:
        if w["id"] in desktop.OURS or w["app_id"] in SKIP_APPS or not w.get("pid"):
            continue
        space = ws.get(w["workspace_id"]) or {}
        if space.get("name") == desktop.WORKSPACE:
            continue
        rec = {"app_id": w["app_id"], "title": w["title"], "workspace": space.get("idx"), "output": space.get("output"),
               "pos": w.get("pos"), "floating": w["is_floating"],
               "argv": _argv(w["pid"]), "cwd": _cwd(w["pid"])}
        if w["app_id"] == "kitty":
            rec |= _kitty(w["pid"], kids)
        out.append(rec)
    out.sort(key=lambda r: (r["output"] or "", r["workspace"] or 99, r["pos"] or [99, 99]))
    return {"session": b.session(), "time": time.time(), "windows": out}


def _session(snap: dict | None) -> str | None:
    """Which WM session a snapshot is from (layouts saved before the backends called it "niri")."""
    return (snap or {}).get("session", (snap or {}).get("niri"))


def command(rec: dict, shell: str = "fish") -> str | None:
    """How to start a remembered window again."""
    if rec["app_id"] == "kitty":
        c = rec.get("claude")
        cwd = (c or {}).get("cwd") or rec.get("cwd") or str(Path.home())
        if c:
            # Through the user's shell, so their `claude` function (claude-queue here) wraps it as before.
            claude = shlex.join(["claude", *c["args"], "--resume", c["session_id"]])
            return f"kitty -d {shlex.quote(cwd)} {shell} -c {shlex.quote(claude + f'; exec {shell}')}"
        return f"kitty -d {shlex.quote(cwd)}"
    argv = rec.get("argv") or []
    if not argv:
        return None
    # Just the program: apps restore their own state, and arguments like a file played at launch are stale.
    keep = 2 if os.path.basename(argv[0]) in INTERPRETERS and len(argv) > 1 else 1
    return shlex.join(argv[:keep])


class Layout:
    def __init__(self, desk: desktop.Desktop, approvals, events, state_dir: Path, shell: str | None = None):
        self.desk = desk
        # The shell a resumed Claude session runs in: config [plugins.desktop] shell, else $SHELL.
        self.shell = shell or os.path.basename(os.environ.get("SHELL") or "sh")
        self.approvals = approvals
        self.events = events
        self.path = state_dir / "layout.json"
        self.previous = state_dir / "layout-previous.json"
        self.offered: str | None = None

    async def run(self):
        """Snapshot every EVERY seconds; when the WM is a new session, offer to bring the old layout back."""
        while True:
            await asyncio.sleep(EVERY)
            try:
                await self.tick()
            except desktop.NoDesktop:
                pass
            except Exception:
                log.exception("layout snapshot failed")

    async def tick(self):
        if not self.desk.up:
            return
        snap = await asyncio.to_thread(snapshot)
        saved = read_json(self.path)
        if saved and _session(saved) != snap["session"]:
            # A new WM session: keep the old layout aside until it is restored or declined.
            os.replace(self.path, self.previous)
            saved = None
        if self.offered != snap["session"] and self.previous.exists():
            self.offered = snap["session"]
            asyncio.ensure_future(self.offer())
        if snap["windows"] or not saved:
            write_json(self.path, snap)

    def plan(self) -> list[dict]:
        old = read_json(self.previous) or {}
        if not old or time.time() - old.get("time", 0) > MAX_AGE:
            return []
        have = {}
        for w in desktop.backend().windows():
            have[w["app_id"]] = have.get(w["app_id"], 0) + 1
        out = []
        for rec in old["windows"]:
            # Windows that came back on their own (autostart, an app's own session restore) count first.
            if have.get(rec["app_id"], 0) > 0 and not rec.get("claude"):
                have[rec["app_id"]] -= 1
                continue
            if cmd := command(rec, self.shell):
                out.append({**rec, "command": cmd})
        return out

    def describe(self, plan: list[dict]) -> str:
        lines = []
        for r in plan:
            what = f"resume Claude session \"{r['claude'].get('name') or r['claude']['session_id'][:8]}\"" if r.get("claude") \
                else r["app_id"]
            lines.append(f"workspace {r['workspace']}: {what}")
        return "\n".join(lines)

    async def offer(self):
        """One card for the whole layout: it resumes the user's Claude sessions, which always needs their OK."""
        await asyncio.sleep(15)
        try:
            plan = await asyncio.to_thread(self.plan)
        except desktop.NoDesktop:
            return
        if not plan:
            return
        outcome = await self.approvals.hold({"action": "desktop", "target": f"bring back your desktop ({len(plan)} windows)",
                                             "detail": self.describe(plan)}, "seimei", None)
        if outcome == "approved":
            await self.restore(plan)
        else:
            self.events.add({"type": "system", "text": "The old desktop layout was not restored; ask seimei to restore it later."})

    async def restore(self, plan: list[dict]) -> list[str]:
        done = []
        for r in plan:
            try:
                w = await self.desk.spawn(r["command"], own_workspace=False)
                if w:
                    # Spawned apps are the user's again, not seimei's.
                    desktop.OURS.pop(w["id"], None)
                    if r.get("workspace") and not r.get("floating"):
                        await asyncio.to_thread(desktop.backend().move_to_workspace, w["id"], str(r["workspace"]))
                done.append(f"{r['app_id']} on workspace {r['workspace']}" + (" (resumed)" if r.get("claude") else ""))
            except Exception as e:
                done.append(f"{r['app_id']} failed: {e}")
        self.previous.unlink(missing_ok=True)
        self.events.add({"type": "system", "text": f"Brought back {len(done)} windows."})
        return done
