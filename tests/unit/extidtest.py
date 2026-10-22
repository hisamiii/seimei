"""The Waterfox connector's install (browser/install.py) on temp dirs: which extension id the host allows and the
packed extension carries: the configured one, else the one already installed, else the neutral default."""
import json
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "browser"))
import install as I

res = []


def check(n, ok, x=""):
    res.append(bool(ok))
    print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300], flush=True)


def run(d: Path) -> tuple[str, dict, str]:
    xpi = d / "seimei.xpi"
    eid = I.install(ROOT / "browser", d / "hosts", xpi, d / "config.toml")
    host = json.loads((d / "hosts/seimei.json").read_text())
    with zipfile.ZipFile(xpi) as z:
        packed = json.loads(z.read("manifest.json"))["browser_specific_settings"]["gecko"]["id"]
        names = set(z.namelist())
    check("the extension has its files", names == {"manifest.json", "background.js", "read.js", "icon.svg"}, names)
    return eid, host, packed


d = Path(tempfile.mkdtemp(prefix="extid-"))
eid, host, packed = run(d)
check("nothing set or installed: the neutral id", eid == "seimei@localhost" and host["allowed_extensions"] == [eid] and packed == eid,
      (eid, host, packed))
check("the host runs the checkout's host.py", host["path"] == str(ROOT / "browser/host.py"), host)

(d / "hosts/seimei.json").write_text(json.dumps({**host, "allowed_extensions": ["old@example.org"]}))
eid, host, packed = run(d)
check("installed before: its id stays", eid == "old@example.org" and host["allowed_extensions"] == [eid] and packed == eid,
      (eid, host, packed))

(d / "config.toml").write_text('[plugins.browser]\nextension_id = "mine@example.net"\n')
eid, host, packed = run(d)
check("configured: that id wins", eid == "mine@example.net" and host["allowed_extensions"] == [eid] and packed == eid,
      (eid, host, packed))

(d / "config.toml").write_text("not toml [")
check("a broken config falls back to the installed id", I.extension_id(d / "config.toml", d / "hosts/seimei.json") == "mine@example.net")

print(f"{sum(res)}/{len(res)}")
sys.exit(0 if all(res) else 1)
