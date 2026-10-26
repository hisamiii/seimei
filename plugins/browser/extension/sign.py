"""Signs the browser connector on addons.mozilla.org, unlisted (AMO signs it but does not list it), and writes the
signed extension to seimei.xpi beside it, which install.py then installs. Firefox builds that enforce signing load only
a signed extension.

The AMO API key (https://addons.mozilla.org/developers/addon/api/key/) goes in ~/.config/amo/amo.env as
AMO_JWT_ISSUER=... and AMO_JWT_SECRET=... (mode 600); it is never printed. Each signing needs a version AMO has not
seen yet: bump "version" in manifest.json first.
Usage: python3 plugins/browser/extension/sign.py [extension id]   (default: the id install.py picks)"""
import base64
import hashlib
import hmac
import io
import json
import os
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid
import zipfile
from pathlib import Path

import install

API = "https://addons.mozilla.org/api/v5/addons"
ENV = Path.home() / ".config/amo/amo.env"
HERE = Path(__file__).resolve().parent
OUT = HERE / "seimei.xpi"


def key() -> tuple[str, str]:
    env = {}
    for line in ENV.read_text().splitlines():
        k, sep, v = line.strip().partition("=")
        if sep and not k.startswith("#"):
            env[k.strip()] = v.strip().strip("'\"")
    return env["AMO_JWT_ISSUER"], env["AMO_JWT_SECRET"]


def token(issuer: str, secret: str) -> str:
    """A JWT for one request (HS256, valid 60 s)."""
    b64 = lambda d: base64.urlsafe_b64encode(json.dumps(d, separators=(",", ":")).encode()).rstrip(b"=")
    now = int(time.time())
    msg = b64({"alg": "HS256", "typ": "JWT"}) + b"." + b64({"iss": issuer, "jti": str(uuid.uuid4()), "iat": now,
                                                            "exp": now + 60})
    sig = base64.urlsafe_b64encode(hmac.new(secret.encode(), msg, hashlib.sha256).digest()).rstrip(b"=")
    return (msg + b"." + sig).decode()


def call(creds, method: str, url: str, body: bytes | None = None, ctype: str = "application/json"):
    """(status, body bytes); errors come back as their status, so a 404 can mean "not there yet"."""
    req = urllib.request.Request(url, data=body, method=method, headers={
        "Authorization": "JWT " + token(*creds), "User-Agent": "seimei-sign/1", "Accept": "application/json"})
    if body is not None:
        req.add_header("Content-Type", ctype)
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def ok(status: int, body: bytes, what: str) -> dict:
    if status >= 300:
        sys.exit(f"sign: {what} failed ({status}): {body.decode(errors='replace')[:1000]}")
    return json.loads(body)


def upload(creds, xpi: bytes) -> str:
    """Uploads for the unlisted channel and waits for the validation; the upload's uuid."""
    b = uuid.uuid4().hex
    parts = [f'--{b}\r\nContent-Disposition: form-data; name="channel"\r\n\r\nunlisted\r\n'.encode(),
             f'--{b}\r\nContent-Disposition: form-data; name="upload"; filename="seimei.xpi"\r\n'
             f'Content-Type: application/x-xpinstall\r\n\r\n'.encode() + xpi + b"\r\n", f"--{b}--\r\n".encode()]
    up = ok(*call(creds, "POST", f"{API}/upload/", b"".join(parts), f"multipart/form-data; boundary={b}"), "upload")
    for _ in range(120):
        if up.get("processed"):
            break
        time.sleep(5)
        up = ok(*call(creds, "GET", f"{API}/upload/{up['uuid']}/"), "upload status")
    else:
        sys.exit("sign: AMO did not validate the upload in 10 minutes")
    msgs = (up.get("validation") or {}).get("messages", [])
    for m in msgs:
        print(f"sign: {m.get('type')}: {m.get('message')}", file=sys.stderr)
    if not up.get("valid"):
        sys.exit("sign: AMO rejected the upload (see above)")
    return up["uuid"]


def submit(creds, eid: str, up: str, version: str) -> None:
    """A new version from the upload; the add-on itself is created on its first signing."""
    status, _ = call(creds, "GET", f"{API}/addon/{eid}/")
    if status == 404:
        body = json.dumps({"version": {"upload": up}}).encode()
        ok(*call(creds, "PUT", f"{API}/addon/{eid}/", body), "add-on create")
    else:
        ok(*call(creds, "POST", f"{API}/addon/{eid}/versions/", json.dumps({"upload": up}).encode()),
           f"version {version}")


def signed(creds, eid: str, version: str) -> bytes:
    """Waits for AMO to sign the version and downloads the signed file."""
    for _ in range(180):
        v = ok(*call(creds, "GET", f"{API}/addon/{eid}/versions/{version}/"), "version status")
        f = v.get("file") or {}
        if f.get("status") == "public" and f.get("url"):
            status, data = call(creds, "GET", f["url"])
            if status >= 300:
                sys.exit(f"sign: download failed ({status})")
            return data
        if f.get("status") == "disabled":
            sys.exit(f"sign: AMO did not sign {version} (status disabled)")
        time.sleep(5)
    sys.exit("sign: not signed after 15 minutes; run again later to fetch it")


def main() -> None:
    eid = sys.argv[1] if len(sys.argv) > 1 else install.extension_id(install.CONFIG, install.HOSTS[0] / "seimei.json")
    version = json.loads((HERE / "manifest.json").read_text())["version"]
    creds = key()
    status, _ = call(creds, "GET", f"{API}/addon/{eid}/versions/{version}/")
    if status == 404:
        with tempfile.TemporaryDirectory() as tmp:
            xpi = Path(tmp) / "seimei.xpi"
            install.pack(HERE, eid, xpi)
            up = upload(creds, xpi.read_bytes())
        submit(creds, eid, up, version)
        print(f"sign: {eid} {version} uploaded, waiting for the signature")
    else:
        print(f"sign: {eid} {version} is on AMO already, fetching it")
    data = signed(creds, eid, version)
    names = zipfile.ZipFile(io.BytesIO(data)).namelist()
    if not any(n.startswith("META-INF/") for n in names):
        sys.exit("sign: the download has no signature")
    tmp = OUT.with_suffix(".tmp")
    tmp.write_bytes(data)
    os.replace(tmp, OUT)
    print(f"sign: wrote {OUT} ({eid} {version})")


if __name__ == "__main__":
    main()
