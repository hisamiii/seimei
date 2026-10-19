"""Small helpers shared by the dispatcher, the workers and the login."""

import json
import os
import re
from pathlib import Path

QUEUE_LIB = Path.home() / ".local/lib/claude-queue"
RESULT_CHARS = 4000


def summarize(inp: dict) -> str:
    """One line describing a tool call, for the collapsed activity row."""
    for key in ("command", "file_path", "path", "pattern", "url", "query", "description", "prompt", "task", "worker"):
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


def queue_env(name: str) -> dict:
    """Run a session's commands through claude-queue, like `claude-queue <name>`, so benches freeze it."""
    if not (QUEUE_LIB / "queue-prefix").exists():
        return {}
    qdir = Path(os.environ.get("XDG_RUNTIME_DIR") or f"/tmp/claude-queue-{os.getuid()}") / "claude-queue"
    qdir.mkdir(parents=True, exist_ok=True)
    return {
        # Same rule as the claude-queue script: a dash would nest slices.
        "CLAUDE_QUEUE": re.sub(r"[^A-Za-z0-9_]", "_", name),
        "CLAUDE_QUEUE_DIR": str(qdir),
        "CLAUDE_QUEUE_LIB": str(QUEUE_LIB / "devgate.so"),
        "CLAUDE_CODE_SHELL_PREFIX": str(QUEUE_LIB / "queue-prefix"),
    }


def read_json(path: Path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def write_json(path: Path, data, private: bool = False):
    """Atomic write; private files are created 600."""
    tmp = path.with_suffix(".tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600 if private else 0o644)
    with os.fdopen(fd, "w") as f:
        json.dump(data, f, indent=1)
    tmp.replace(path)
