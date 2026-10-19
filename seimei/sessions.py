"""Every Claude Code session on the laptop, read from Claude Code's own session registry and transcripts."""

import json
import os
import re
import time
from pathlib import Path

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
    for k in ("command", "file_path", "pattern", "url", "query", "description", "prompt"):
        if isinstance(inp.get(k), str) and inp[k].strip():
            return inp[k].strip().splitlines()[0][:200]
    return json.dumps(inp)[:200]


def peek(path: Path, count: int = 3) -> dict:
    """Last user messages and assistant texts, plus tool calls still open (a pending AskUserQuestion first)."""
    with open(path, "rb") as f:
        f.seek(max(f.seek(0, 2) - TAIL, 0))
        lines = f.read().splitlines()[1:] or []
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
