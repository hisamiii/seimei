"""Installs the Waterfox connector (deploy.sh runs it): the native-messaging host manifest, and the extension packed
for the user to install once (about:addons, Install Add-on From File).

The extension's id: [plugins.browser] extension_id in config.toml, else the one the installed host already allows (so
an extension installed before keeps working), else seimei@localhost.
Usage: install.py <browser dir> <native-messaging hosts dir> <xpi to write> <config.toml>"""
import json
import sys
import tomllib
import zipfile
from pathlib import Path

DEFAULT = "seimei@localhost"
CONFIG = Path.home() / ".config/seimei/config.toml"
HOSTS = [Path.home() / ".mozilla/native-messaging-hosts"]


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


def install(src: Path, hosts: Path, xpi: Path, config: Path) -> str:
    host = hosts / "seimei.json"
    eid = extension_id(config, host)
    hosts.mkdir(parents=True, exist_ok=True)
    host.write_text(json.dumps({"name": "seimei", "description": "seimei connector", "path": str(src / "host.py"),
                                "type": "stdio", "allowed_extensions": [eid]}) + "\n")
    pack(src, eid, xpi)
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
    print(install(*map(Path, sys.argv[1:5])))
