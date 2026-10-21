"""The suite runs every script in a scrubbed environment: no desktop (no display, compositor or kitty socket variables,
so nothing can open a window even if a stub rots), no job variables of the session that runs it, auto-memory off, and a
Claude config dir of its own, so test sessions never show up in the user's session list or seimei's events.

Groups: unit (offline, the default), web (the QA server on :8431 plus headless Chrome, `-m web`), live (headless
claude sessions on haiku, `-m live`; log the test config dir in once: `CLAUDE_CONFIG_DIR=<dir> claude /login`)."""
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
TESTS = ROOT / "tests"
CLAUDE_DIR = Path(os.environ.get("SEIMEI_TEST_CLAUDE_DIR") or Path.home() / ".local/share/seimei-test/claude")
DESKTOP = ("DISPLAY", "WAYLAND_DISPLAY", "NIRI_SOCKET", "SWAYSOCK", "HYPRLAND_INSTANCE_SIGNATURE", "KITTY_LISTEN_ON",
           "KITTY_WINDOW_ID", "XDG_CURRENT_DESKTOP")


def clean_env(**extra) -> dict:
    env = {k: v for k, v in os.environ.items()
           if k not in DESKTOP and not (k.startswith("CLAUDE") or k.startswith("SEIMEI_") or k == "LD_PRELOAD")}
    env.update(PYTHONPATH=str(ROOT), CLAUDE_CODE_DISABLE_AUTO_MEMORY="1", CLAUDE_CONFIG_DIR=str(CLAUDE_DIR))
    env.update(extra)
    return env


def run_script(path: Path, timeout=300, **env):
    """Runs one of the check scripts: it prints PASS/FAIL lines and must exit 0 with no FAIL."""
    r = subprocess.run([sys.executable, str(path)], cwd=ROOT, env=clean_env(**env), capture_output=True, text=True,
                       timeout=timeout)
    out = r.stdout + r.stderr
    fails = [line for line in out.splitlines() if line.startswith("FAIL")]
    assert r.returncode == 0 and not fails, f"{path.name}: rc={r.returncode}\n" + "\n".join(fails or out.splitlines()[-30:])


def scripts(group: str) -> list:
    return sorted(p for p in (TESTS / group).glob("*.py") if p.name != "h.py")


def pytest_configure(config):
    config.addinivalue_line("markers", "web: needs the QA server and headless Chrome")
    config.addinivalue_line("markers", "live: starts headless claude sessions (haiku)")


@pytest.fixture
def qa_server():
    """A fresh QA server per test: state carries over otherwise."""
    subprocess.run([str(TESTS / "qa/start.sh"), "--login"], env=clean_env(), check=True, capture_output=True)
    yield "http://localhost:8431"
    subprocess.run([str(TESTS / "qa/stop.sh")], capture_output=True)
