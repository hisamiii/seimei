"""Ends what QA servers left behind: the fake user sessions (sleep 7200 stand-ins writing to this server.log) and any
fake claude whose server is gone (they exit when their stdin closes; this is the backstop)."""
import os, signal
from pathlib import Path

QA = Path(__file__).resolve().parent
log = str(QA / "server.log")
fake = str(QA.parent / "fakes/claude.py").encode()
n = m = 0
for d in Path("/proc").iterdir():
    if not d.name.isdigit():
        continue
    try:
        cmd = (d / "cmdline").read_bytes()
        if cmd == b"sleep\x007200\x00" and os.readlink(d / "fd" / "1") == log:
            os.kill(int(d.name), signal.SIGTERM)
            n += 1
        elif fake in cmd.split(b"\0") and f"CLAUDE_CONFIG_DIR={QA}/".encode() in (d / "environ").read_bytes():
            os.kill(int(d.name), signal.SIGTERM)
            m += 1
    except OSError:
        pass
print(f"reaped {n} fake sessions, {m} fake claude")
