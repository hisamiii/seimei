"""Phase 4's dispatcher run (crates/seimei/tests/live_dispatcher.rs): seimei's dispatcher on haiku, wired like the
server, calls a read-only tool of the queue, music, browser and discord plugins on fakes. In this scrubbed environment
it builds the test and the seimei binary, runs it once and prints its PASS/FAIL lines."""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
r = subprocess.run(["cargo", "test", "-q", "-p", "seimei", "--test", "live_dispatcher", "--", "--ignored", "--nocapture"],
                   cwd=ROOT)
sys.exit(r.returncode)
