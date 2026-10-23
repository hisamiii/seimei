"""The live ACP shikigami on dsh (crates/seimei/tests/it/live_acp.rs): builds the test, runs it once and prints its
PASS/FAIL lines. dsh runs on its own login; it needs `dsh` on PATH. The test runs in a temp HOME whose only key is a
decoy (dsh keeps its own home through DSH_HOME, cargo and rustup theirs), so a read the guard failed to stop would
leak nothing."""
import os
import secrets
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
real = Path.home()
home = Path(tempfile.mkdtemp(prefix="seimei-acphome-"))
try:
    keys = home / ".ssh"
    keys.mkdir(mode=0o700)
    (keys / "id_ed25519").write_text(f"DECOY-{secrets.token_hex(8)}\n")
    env = {**os.environ, "HOME": str(home), "DSH_HOME": os.environ.get("DSH_HOME") or str(real / ".dsh"),
           "CARGO_HOME": os.environ.get("CARGO_HOME") or str(real / ".cargo"),
           "RUSTUP_HOME": os.environ.get("RUSTUP_HOME") or str(real / ".rustup")}
    r = subprocess.run(["cargo", "test", "-q", "-p", "seimei", "--test", "it", "--", "--ignored", "live_acp::",
                        "--nocapture"], cwd=ROOT, env=env)
finally:
    shutil.rmtree(home)
sys.exit(r.returncode)
