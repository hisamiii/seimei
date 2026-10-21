"""Ends the fake user sessions (sleep 7200 stand-ins) that QA servers started: sleeps writing to this server.log."""
import os, signal
from pathlib import Path

log = str(Path(__file__).resolve().parent / "server.log")
n = 0
for d in Path("/proc").iterdir():
    if not d.name.isdigit():
        continue
    try:
        if (d / "cmdline").read_bytes() != b"sleep\x007200\x00" or os.readlink(d / "fd" / "1") != log:
            continue
        os.kill(int(d.name), signal.SIGTERM)
        n += 1
    except OSError:
        pass
print(f"reaped {n} fake sessions")
