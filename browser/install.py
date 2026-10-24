"""Installs the browser connector (deploy.sh runs it): the native-messaging host manifest in each hosts folder given
(Firefox reads ~/.mozilla/native-messaging-hosts, or ~/.config/mozilla/native-messaging-hosts where it uses XDG
paths), and the extension for the user to install once (about:addons, Install Add-on From File).

The extension is browser/seimei.xpi when sign.py signed the version in manifest.json (Firefox builds that enforce
signing load only that one), else it is packed here unsigned. Its id: the signed file's, else [plugins.browser]
extension_id in config.toml, else the one the installed host already allows (so an extension installed before keeps
working), else seimei@localhost.
Usage: install.py <browser dir> <xpi to write> <config.toml> <native-messaging hosts dir>..."""
import json
import shutil
import sys
import tomllib
import zipfile
from pathlib import Path

DEFAULT = "seimei@localhost"
CONFIG = Path.home() / ".config/seimei/config.toml"
HOSTS = [Path.home() / ".mozilla/native-messaging-hosts", Path.home() / ".config/mozilla/native-messaging-hosts"]


def extension_id(config: Path, host: Path) -> str:
    try:
        chosen = tomllib.loads(config.read_text()).get("plugins", {}).get("browser", {}).get("extension_id")
    except (OSError, tomllib.TOMLDecodeError):
        chosen = None
    if chosen:
        return chosen
    try:
        return json.loads(host.read_text())["allowed_extensions"][0]
    except (OSError, ValueError, KeyError, IndexError, TypeError):
        return DEFAULT


def signed(src: Path) -> tuple[str, str] | None:
    """(id, version) of the extension sign.py signed, if there is one."""
    try:
        with zipfile.ZipFile(src / "seimei.xpi") as z:
            m = json.loads(z.read("manifest.json"))
        return m["browser_specific_settings"]["gecko"]["id"], m["version"]
    except (OSError, KeyError, ValueError, zipfile.BadZipFile):
        return None


def install(src: Path, xpi: Path, config: Path, hosts: list[Path]) -> str:
    version = json.loads((src / "manifest.json").read_text())["version"]
    sig = signed(src)
    if sig and sig[1] == version:
        eid = sig[0]
        tmp = xpi.with_suffix(".tmp")
        shutil.copyfile(src / "seimei.xpi", tmp)
        tmp.replace(xpi)
    else:
        if sig:
            print(f"install: browser/seimei.xpi is {sig[1]}, manifest.json {version}: run browser/sign.py; "
                  "packing it unsigned", file=sys.stderr)
        known = [h / "seimei.json" for h in hosts if (h / "seimei.json").exists()]
        eid = extension_id(config, known[0] if known else hosts[0] / "seimei.json")
        pack(src, eid, xpi)
    host = json.dumps({"name": "seimei", "description": "seimei connector", "path": str(src / "host.py"),
                       "type": "stdio", "allowed_extensions": [eid]}) + "\n"
    for h in hosts:
        h.mkdir(parents=True, exist_ok=True)
        # Written beside and renamed over, so a hand-made symlink there becomes a file of its own.
        tmp = h / "seimei.json.tmp"
        tmp.write_text(host)
        tmp.replace(h / "seimei.json")
    return eid


def pack(src: Path, eid: str, xpi: Path) -> None:
    """The extension as an xpi, with its id set to eid."""
    manifest = json.loads((src / "manifest.json").read_text())
    manifest["browser_specific_settings"]["gecko"]["id"] = eid
    tmp = xpi.with_suffix(".tmp")
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("manifest.json", json.dumps(manifest, indent=2) + "\n")
        for f in ("background.js", "read.js", "icon.svg"):
            z.write(src / f, f)
    tmp.replace(xpi)


if __name__ == "__main__":
    a = sys.argv[1:]
    print(install(Path(a[0]), Path(a[1]), Path(a[2]), [Path(h) for h in a[3:]] or HOSTS))
