"""Every Claude Code session on the laptop, read from Claude Code's own session registry and transcripts."""

import asyncio
import json
import logging
import os
import re
import time
from pathlib import Path

from . import kitty
from .events import EventLog

log = logging.getLogger("seimei.sessions")

CLAUDE = Path.home() / ".claude"
TAIL = 4 << 20
TEXT = 1500


def _alive(rec: dict) -> bool:
    """The pid runs and is the same process (pids get reused)."""
    try:
        stat = Path(f"/proc/{rec['pid']}/stat").read_text()
    except (OSError, KeyError):
        return False
    start = stat.rsplit(")", 1)[1].split()[19]
    return str(rec.get("procStart", start)) == start


def ago(t: float) -> str:
    s = max(0, int(time.time() - t))
    return f"{s}s ago" if s < 60 else f"{s // 60} min ago" if s < 3600 else f"{s // 3600} h ago" if s < 86400 else f"{s // 86400} d ago"


def transcript(rec: dict) -> Path | None:
    """The session's current transcript; a session can continue into a new one."""
    sid, cwd = rec.get("sessionId"), rec.get("cwd", "")
    for _ in range(8):
        path = CLAUDE / "projects" / re.sub(r"[^A-Za-z0-9]", "-", cwd) / f"{sid}.jsonl"
        if not path.exists():
            path = next(iter((CLAUDE / "projects").glob(f"*/{sid}.jsonl")), None)
        if not path:
            return None
        nxt = None
        with open(path, "rb") as f:
            f.seek(max(f.seek(0, 2) - 4096, 0))
            for line in f.read().splitlines()[-3:]:
                try:
                    row = json.loads(line)
                except ValueError:
                    continue
                if row.get("type") == "continued-in":
                    nxt = row.get("continuedInSessionId")
        if not nxt:
            return path
        sid = nxt
    return None


def live(own: dict[str, str]) -> list[dict]:
    """Running sessions, newest activity first. own maps seimei's session ids to labels."""
    out = []
    for f in (CLAUDE / "sessions").glob("*.json"):
        try:
            rec = json.loads(f.read_text())
        except (OSError, ValueError):
            continue
        if not _alive(rec):
            continue
        path = transcript(rec)
        sid = path.stem if path else rec.get("sessionId", "")
        active = max(path.stat().st_mtime if path else 0, rec.get("updatedAt", 0) / 1000)
        status = rec.get("status", "?")
        if rec.get("parkedJobId"):
            status = f"parked (moved to background job {rec['parkedJobId']})"
        elif status == "waiting" and rec.get("waitingFor"):
            status = f"waiting: {rec['waitingFor']}"
        out.append({
            "name": rec.get("name") or f"pid {rec['pid']}",
            "pid": rec["pid"],
            "session_id": sid,
            "status": status,
            "since": rec.get("statusUpdatedAt", 0) / 1000,
            "active": active,
            "cwd": rec.get("cwd", ""),
            "kind": own.get(sid) or own.get(rec.get("sessionId", "")) or
                    {"bg": "background"}.get(rec.get("kind"), rec.get("kind", "?")) +
                    (" (SDK)" if str(rec.get("entrypoint", "")).startswith("sdk") else ""),
            "rec": rec,
        })
    out.sort(key=lambda s: -s["active"])
    return out


def find(key: str, own: dict[str, str]) -> dict | None:
    key = key.strip()
    sessions = live(own)
    for test in (lambda s: s["name"] == key, lambda s: str(s["pid"]) == key, lambda s: s["session_id"].startswith(key),
                 lambda s: key.lower() in s["name"].lower()):
        hits = [s for s in sessions if test(s)]
        if len(hits) == 1:
            return hits[0]
        if hits:
            return None
    return None


def _text(content) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(b.get("text", "") for b in content if isinstance(b, dict) and b.get("type") == "text")
    return ""


def _summary(inp: dict) -> str:
    qs = inp.get("questions")
    if isinstance(qs, list) and qs and isinstance(qs[0], dict) and qs[0].get("question"):
        return str(qs[0]["question"]).splitlines()[0][:200]
    for k in ("command", "file_path", "pattern", "url", "query", "description", "prompt"):
        if isinstance(inp.get(k), str) and inp[k].strip():
            return inp[k].strip().splitlines()[0][:200]
    return json.dumps(inp)[:200]


def peek(path: Path, count: int = 3, tail: int = TAIL) -> dict:
    """Last user messages and assistant texts, plus tool calls still open (a pending AskUserQuestion first)."""
    with open(path, "rb") as f:
        start = max(f.seek(0, 2) - tail, 0)
        f.seek(start)
        # A read that starts mid-file starts mid-line.
        lines = f.read().splitlines()[1 if start else 0:]
    users, texts, pending = [], [], {}
    for line in lines:
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if row.get("isSidechain") or row.get("isCompactSummary") or row.get("type") not in ("user", "assistant"):
            continue
        msg = row.get("message") or {}
        content = msg.get("content")
        if row["type"] == "user":
            results = [b for b in content if isinstance(b, dict) and b.get("type") == "tool_result"] if isinstance(content, list) else []
            for b in results:
                pending.pop(b.get("tool_use_id"), None)
            text = _text(content).strip()
            if text and not results and not row.get("isMeta") and not text.startswith(("<command-", "<local-command")):
                # A new prompt means calls left open before it were cut off, not waiting.
                pending.clear()
                users.append((row.get("timestamp", ""), text))
        else:
            for b in content if isinstance(content, list) else []:
                if b.get("type") == "text" and b.get("text", "").strip():
                    texts.append((row.get("timestamp", ""), b["text"].strip()))
                elif b.get("type") == "tool_use":
                    pending[b["id"]] = (row.get("timestamp", ""), b.get("name"), b.get("input") or {})
    cut = lambda s: s if len(s) <= TEXT else s[:TEXT] + " [...]"
    out = {"user": [(t, cut(x)) for t, x in users[-count:]], "assistant": [(t, cut(x)) for t, x in texts[-count:]],
           "questions": [], "open_calls": []}
    for t, name, inp in pending.values():
        if name == "AskUserQuestion":
            out["questions"].append({"asked": t, "questions": inp.get("questions", [])})
        else:
            out["open_calls"].append({"since": t, "tool": name, "what": _summary(inp)})
    return out


HOME = str(Path.home())
# Claude 5 models have a 1M-token window; the session status line agrees (4.2% read as "ctx: 4%").
CONTEXT = 1_000_000
PATHS = re.compile(r"(?:~|\$HOME|/home/[a-z_][\w-]*)/[\w.@+-]+(?:/[\w.@+-]+)*")


def project_of(path: str) -> str | None:
    """The project a path belongs to: its folder under ~/src, ~/.config, else its top folder under home."""
    path = re.sub(r"^(~|\$HOME)(?=/)", HOME, path)
    if not path.startswith(HOME + "/"):
        return None
    parts = path[len(HOME) + 1:].split("/")
    if len(parts) < 2:
        return None
    if parts[0] in ("src", ".config"):
        top = parts[1]
    elif parts[0].startswith("."):
        return None
    else:
        top = parts[0]
    return re.sub(r"-(scratch|wt)$", "", top)


def _votes(inp: dict) -> list[str]:
    paths = [inp[k] for k in ("file_path", "notebook_path", "path") if isinstance(inp.get(k), str)]
    if isinstance(inp.get("command"), str):
        paths += PATHS.findall(inp["command"])
    return [p for p in map(project_of, paths) if p]


def _rows(path: Path, start: int) -> tuple[list[dict], int]:
    """Whole transcript rows from byte start on (a start mid-line skips to the next line) and where they end."""
    with open(path, "rb") as f:
        f.seek(start)
        data = f.read()
    end = data.rfind(b"\n") + 1
    lines = data[:end].splitlines()
    if start and lines and not data.startswith(b"{"):
        lines = lines[1:]
    rows = []
    for line in lines:
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if isinstance(row, dict) and row.get("type") in ("user", "assistant") and not row.get("isSidechain") \
                and not row.get("isCompactSummary"):
            rows.append(row)
    return rows, start + end


def _t(row: dict) -> float:
    try:
        return time.mktime(time.strptime(row.get("timestamp", "")[:19], "%Y-%m-%dT%H:%M:%S")) - time.timezone \
            if row.get("timestamp") else 0
    except ValueError:
        return 0


def _question(inp: dict) -> dict:
    qs = [q for q in inp.get("questions", []) if isinstance(q, dict)]
    return {"questions": qs, "answerable": len(qs) == 1 and not qs[0].get("multiSelect")}


_briefs: dict[Path, tuple[float, dict]] = {}


def brief(path: Path) -> dict:
    """What the list shows for a session: project, the open call or question, last line, context; cached by mtime."""
    mtime = path.stat().st_mtime
    hit = _briefs.get(path)
    if hit and hit[0] == mtime:
        return hit[1]
    size = path.stat().st_size
    rows, _ = _rows(path, max(size - (512 << 10), 0))
    votes, pending, last, used, model = [], {}, "", 0, ""
    for row in rows:
        content = (row.get("message") or {}).get("content")
        blocks = content if isinstance(content, list) else []
        if row["type"] == "user":
            results = [b for b in blocks if isinstance(b, dict) and b.get("type") == "tool_result"]
            for b in results:
                pending.pop(b.get("tool_use_id"), None)
            text = _text(content).strip()
            if text and not results and not row.get("isMeta") and not text.startswith(("<command-", "<local-command")):
                pending.clear()
            continue
        msg = row.get("message") or {}
        if u := msg.get("usage"):
            used = sum(u.get(k, 0) for k in ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "output_tokens"))
            model = msg.get("model", model)
        for b in blocks:
            if b.get("type") == "text" and b.get("text", "").strip():
                last = b["text"].strip()
            elif b.get("type") == "tool_use":
                inp = b.get("input") or {}
                votes += _votes(inp)
                pending[b["id"]] = (_t(row), b.get("name"), inp)
    out = {"project": max(set(votes[-60:]), key=votes[-60:].count) if votes else None,
           "line": next((x.strip() for x in last.splitlines() if x.strip()), "")[:200].lstrip("#* "),
           "context": round(100 * used / CONTEXT) if used else None, "model": model, "ask": None, "doing": None}
    for tid, (t, name, inp) in pending.items():
        if name == "AskUserQuestion":
            out["ask"] = {"tool_id": tid, "since": t, **_question(inp)}
        elif not out["doing"]:
            out["doing"] = {"tool_id": tid, "since": t, "tool": name, "what": _summary(inp)}
    _briefs[path] = (mtime, out)
    if len(_briefs) > 200:
        _briefs.pop(next(iter(_briefs)))
    return out


def asking(path: Path) -> bool:
    """Whether the transcript ends in an open AskUserQuestion; cached until the file changes."""
    try:
        return bool(brief(path)["ask"])
    except OSError:
        return False


RESULT = 4000
OUTPUT = re.compile(r"^/tmp/claude-\d+/[^/]+/[0-9a-f-]+/tasks/[\w-]+\.output$")
ANSI = re.compile(r"\x1b(\[[0-9;:?]*[A-Za-z]|\][^\x07\x1b]*(\x07|\x1b\\)|[()][A-Z0-9])")


def running_output(pid: int, lines: int = 4) -> list[str]:
    """The last lines of the command a session runs now: Claude Code sends its shell's output to a tasks file."""
    best = None
    try:
        kids = [int(c) for t in Path(f"/proc/{pid}/task").iterdir() for c in (t / "children").read_text().split()]
    except OSError:
        return []
    for c in kids:
        try:
            out = Path(os.readlink(f"/proc/{c}/fd/1"))
        except OSError:
            continue
        if OUTPUT.match(str(out)) and out.is_file() and (not best or out.stat().st_mtime > best.stat().st_mtime):
            best = out
    if not best:
        return []
    with open(best, "rb") as f:
        f.seek(max(f.seek(0, 2) - 8192, 0))
        text = f.read().decode(errors="replace")
    rows = [ANSI.sub("", r).rstrip()[-300:] for r in re.split(r"\r?\n", text)]
    # A progress bar redraws its line with carriage returns: keep what the line shows last.
    rows = [r.rsplit("\r", 1)[-1] for r in rows]
    return [r for r in rows if r.strip()][-lines:]


def timeline(path: Path, start: int | None = None, events: int = 400) -> dict:
    """The session as the chat log draws it: user and assistant texts, tool calls and their results.

    From byte start on (the next poll passes back "next"), or the transcript's last events on the first read."""
    size = path.stat().st_size
    tail = start is None
    rows, nxt = _rows(path, max(size - TAIL, 0) if tail else min(start, size))
    out = []
    for row in rows:
        t, uid = _t(row), row.get("uuid", "")
        content = (row.get("message") or {}).get("content")
        blocks = content if isinstance(content, list) else []
        if row["type"] == "user":
            results = [b for b in blocks if isinstance(b, dict) and b.get("type") == "tool_result"]
            for b in results:
                text = _text(b.get("content")) if not isinstance(b.get("content"), str) else b["content"]
                out.append({"type": "tool_result", "tool_id": b.get("tool_use_id"), "error": bool(b.get("is_error")),
                            "text": text if len(text) <= RESULT else text[:RESULT] + "\n[...]", "t": t})
            text = _text(content).strip()
            if text and not results and not row.get("isMeta") and not text.startswith(("<command-", "<local-command")):
                out.append({"id": uid, "type": "user", "text": text[:8000], "t": t})
            continue
        for i, b in enumerate(blocks):
            if b.get("type") == "text" and b.get("text", "").strip():
                out.append({"id": f"{uid}:{i}", "type": "assistant", "text": b["text"].strip()[:20000], "t": t})
            elif b.get("type") == "tool_use":
                out.append({"id": f"{uid}:{i}", "type": "tool", "name": b.get("name"), "tool_id": b["id"],
                            "summary": _summary(b.get("input") or {}), "t": t})
    if tail and len(out) > events:
        out = out[-events:]
    return {"events": out, "next": nxt}


def listing(own: dict[str, str]) -> list[dict]:
    """The user's running sessions for the page: seimei's and parked terminals left out."""
    out = []
    for s in live(own):
        rec = s["rec"]
        # seimei's sessions show in their own places; another seimei (dev) by its name.
        if s["kind"] in own.values() or rec.get("parkedJobId") or re.match(r"^(seimei(-dev)?|shikigami .+)$", s["name"]):
            continue
        path = transcript(rec)
        try:
            b = brief(path) if path else {}
        except OSError:
            b = {}
        ask = b.get("ask")
        out.append({"pid": s["pid"], "name": s["name"], "session_id": s["session_id"],
                    "status": rec.get("status", "?"), "waiting_for": rec.get("waitingFor"),
                    "since": s["since"], "active": s["active"], "cwd": s["cwd"], "kind": s["kind"],
                    "question": bool(ask), "ask": ask and ask["questions"][0].get("question", "") if ask and ask["questions"] else None,
                    "project": b.get("project") or (project_of(s["cwd"] + "/x") if s["cwd"] != HOME else None),
                    "line": b.get("line", ""), "context": b.get("context"), "doing": b.get("doing"),
                    "reach": kitty.reachable(s["pid"])})
    return out


class Watcher:
    """Pushes the user's sessions to the page when anything but the clock changes."""

    def __init__(self, events: EventLog, own, finished=None):
        self.events = events
        self.own = own
        self.finished = finished
        self.list: list[dict] = []

    def snapshot(self) -> dict:
        return {"type": "sessions", "list": self.list}

    async def run(self, every: float = 4):
        sig = None
        while True:
            try:
                rows = await asyncio.to_thread(listing, self.own())
                new = [(r["pid"], r["name"], r["status"], r["waiting_for"], r["since"], r["question"], r["project"],
                        r["line"], r["context"], (r["doing"] or {}).get("tool_id"), r["reach"],
                        r["status"] == "busy" or round(r["active"])) for r in rows]
                if new != sig:
                    before = {r["pid"]: r["status"] for r in self.list}
                    for r in rows:
                        if self.finished and before.get(r["pid"]) == "busy" and r["status"] != "busy":
                            self.finished(r)
                    sig, self.list = new, rows
                    self.events.flash(self.snapshot())
            except Exception:
                log.exception("session scan failed")
            await asyncio.sleep(every)
