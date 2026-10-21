"""Small helpers shared by the dispatcher, the workers and the login."""

import json
import os
from pathlib import Path

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


def mins(s: float) -> str:
    s = int(s)
    return f"{s}s" if s < 60 else f"{s // 60} min" if s < 3600 else f"{s // 3600} h {s % 3600 // 60} min"


def always_load(server: dict, deferred: set[str]) -> dict:
    """Mark a server's tools, except the deferred ones, with the MCP _meta flag Claude Code reads to keep a tool in
    the prompt instead of behind tool search. The SDK's tool() has no field for it, so the listing is wrapped."""
    srv = server["instance"]
    entry = srv.get_request_handler("tools/list")

    async def listed(ctx, params):
        res = await entry.handler(ctx, params)
        for t in res.tools:
            if t.name not in deferred:
                t.meta = {**(t.meta or {}), "anthropic/alwaysLoad": True}
        return res

    srv.add_request_handler("tools/list", entry.params_type, listed)
    return server


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


# One stream-json line from claude can carry a whole image a tool read (base64); the SDK's 1 MB default killed
# sessions that read a picture over ~750 KB.
STREAM_BUFFER = 64 << 20
# Sessions seimei starts leave out the user's claude.ai connectors (Gmail, Drive, Docs).
# seimei's own data folder (the state of the live service; --dev adds dev/): secrets placed by the user live here.
DATA = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share") / "seimei"


def claude_cli() -> str:
    """The claude executable sessions run: SEIMEI_CLAUDE (the tests' fake), else the one on PATH."""
    import shutil
    return os.environ.get("SEIMEI_CLAUDE") or shutil.which("claude") or "claude"


NO_CONNECTORS = {"ENABLE_CLAUDEAI_MCP_SERVERS": "false"}
