"""The queue plugin from a live haiku shikigami on the Rust pool (crates/seimei/tests/it/live_plugin.rs), in this scrubbed
environment: it builds the test and the seimei binary, runs it once and prints its PASS/FAIL lines."""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
r = subprocess.run(["cargo", "test", "-q", "-p", "seimei", "--test", "it", "--", "--ignored", "live_plugin::", "--nocapture"],
                   cwd=ROOT)
sys.exit(r.returncode)
