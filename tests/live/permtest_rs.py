"""The Rust twin of permtest.py (crates/core/tests/it/live_perm.rs), in this scrubbed environment: it builds the test, runs
it once and prints its PASS/FAIL lines."""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
r = subprocess.run(["cargo", "test", "-q", "-p", "seimei-core", "--test", "it", "--", "--ignored", "live_perm::", "--nocapture"],
                   cwd=ROOT)
sys.exit(r.returncode)
