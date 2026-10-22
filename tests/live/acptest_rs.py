"""The live ACP shikigami on dsh (crates/seimei/tests/live_acp.rs): builds the test, runs it once and prints its
PASS/FAIL lines. dsh runs on its own login; it needs `dsh` on PATH."""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
r = subprocess.run(["cargo", "test", "-q", "-p", "seimei", "--test", "live_acp", "--", "--ignored", "--nocapture"],
                   cwd=ROOT)
sys.exit(r.returncode)
