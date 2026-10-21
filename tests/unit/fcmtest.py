"""Fcm against a local fake of Google's token and send endpoints, with a throwaway RSA key."""
import asyncio
import base64
import json
import tempfile
from pathlib import Path

from aiohttp import web
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding, rsa

from seimei.push import Fcm

key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()).decode()
res, seen = [], {"token": 0}


def check(n, ok, x=""):
    res.append(ok)
    print("PASS" if ok else "FAIL", n, x, flush=True)


def unb64(s):
    return base64.urlsafe_b64decode(s + "=" * (-len(s) % 4))


async def token(req):
    form = await req.post()
    head, claims, sig = form["assertion"].split(".")
    key.public_key().verify(unb64(sig), f"{head}.{claims}".encode(), padding.PKCS1v15(), hashes.SHA256())
    c = json.loads(unb64(claims))
    assert c["scope"].endswith("firebase.messaging") and c["iss"] == "sa@test"
    seen["token"] += 1
    return web.json_response({"access_token": "AT", "expires_in": 3600})


async def send(req):
    assert req.headers["Authorization"] == "Bearer AT"
    body = await req.json()
    m = body["message"]
    if m["token"] == "dead":
        return web.json_response({"error": {"status": "NOT_FOUND", "details": [{"errorCode": "UNREGISTERED"}]}}, status=404)
    seen["msg"] = m
    return web.json_response({"name": "projects/p/messages/1"})


async def main():
    app = web.Application()
    app.router.add_post("/token", token)
    app.router.add_post("/v1/projects/p/messages:send", send)
    runner = web.AppRunner(app)
    await runner.setup()
    await web.TCPSite(runner, "127.0.0.1", 8499).start()
    d = Path(tempfile.mkdtemp())
    (d / "sa.json").write_text(json.dumps({"client_email": "sa@test", "private_key": pem, "project_id": "p",
                                           "token_uri": "http://127.0.0.1:8499/token"}))
    f = Fcm(d / "sa.json")
    f.base = "http://127.0.0.1:8499"
    check("send ok", await f.send("tok", {"kind": "reply", "tag": "reply"}, 60) == "ok")
    check("message shape", seen["msg"]["android"] == {"priority": "high", "ttl": "60s", "collapse_key": "reply"} and seen["msg"]["data"]["kind"] == "reply")
    check("dead token is gone", await f.send("dead", {"tag": "x"}, 60) == "gone")
    check("access token cached", seen["token"] == 1, seen)
    check("missing key is not ready", not Fcm(d / "nope.json").ready)
    await f.close()
    await runner.cleanup()
    print(f"{sum(res)}/{len(res)} passed")

asyncio.run(main())
