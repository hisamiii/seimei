"""#63 server side: Web Push encryption against RFC 8291's worked example, VAPID, the size bound, a send to a fake push
server that decrypts it like a phone, dispatch per registration kind, and the background connection."""
import asyncio
import json
import struct
import tempfile
import time
from pathlib import Path

import aiohttp
from aiohttp import web
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import encode_dss_signature
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

import seimei.push as P
from seimei.events import EventLog

res = []


def check(n, ok, x=""):
    res.append(bool(ok))
    print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300], flush=True)


# RFC 8291, Appendix A (tests/cases/webpush.json, shared with the Rust server's tests).
_V = json.loads((Path(__file__).resolve().parents[1] / "cases/webpush.json").read_text())["rfc8291"]
AS_PRIVATE = _V["as_private"]
AS_PUBLIC = _V["as_public"]
UA_PUBLIC = _V["ua_public"]
UA_PRIVATE = _V["ua_private"]
SALT = _V["salt"]
AUTH = _V["auth"]
PLAIN = _V["plain"]
HEADER = _V["header"]
CIPHER = _V["cipher"]


def private(b: str) -> ec.EllipticCurvePrivateKey:
    return ec.derive_private_key(int.from_bytes(P.unb64(b), "big"), ec.SECP256R1())


def decrypt(body: bytes, ua_private: ec.EllipticCurvePrivateKey, auth_secret: bytes) -> bytes:
    """What the phone's push library does, written from the RFC independently of seimei's encrypt()."""
    salt, rs, idlen = body[:16], *struct.unpack(">IB", body[16:21])
    as_public, sealed = body[21:21 + idlen], body[21 + idlen:]
    ua_public = P._point(ua_private.public_key())
    ecdh = ua_private.exchange(ec.ECDH(), ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), as_public))
    ikm = P._hkdf(auth_secret, ecdh, b"WebPush: info\0" + ua_public + as_public, 32)
    cek = P._hkdf(salt, ikm, b"Content-Encoding: aes128gcm\0", 16)
    nonce = P._hkdf(salt, ikm, b"Content-Encoding: nonce\0", 12)
    plain = AESGCM(cek).decrypt(nonce, sealed, None)
    assert plain.endswith(b"\x02") and rs == 4096
    return plain[:-1]


class FakeAuth:
    def __init__(self, devs):
        self.devs = devs

    def push_devices(self):
        return [d for d in self.devs if d.get("push")]

    def device(self, dev):
        return next((d for d in self.devs if d["id"] == dev), None)

    def set_push(self, dev, rec):
        for d in self.devs:
            if d["id"] == dev:
                d["push"] = rec


class Fake:
    ready = True

    def __init__(self, out="ok"):
        self.got, self.out = [], out

    async def send(self, to, data, ttl):
        self.got.append((to, data))
        return self.out


async def main():
    # 1. RFC 8291 Appendix A, byte for byte.
    body = P.encrypt(P.unb64(PLAIN), P.unb64(UA_PUBLIC), P.unb64(AUTH), private(AS_PRIVATE), P.unb64(SALT))
    check("RFC 8291 example: header", body[:86] == P.unb64(HEADER), P.b64(body[:86]))
    check("RFC 8291 example: ciphertext", body[86:] == P.unb64(CIPHER), P.b64(body[86:]))
    check("the example's public key matches its private one", P._point(private(AS_PRIVATE).public_key()) == P.unb64(AS_PUBLIC))
    check("decrypts with the phone's key", decrypt(body, private(UA_PRIVATE), P.unb64(AUTH)) == P.unb64(PLAIN))

    # 2. The largest message seimei sends stays under Web Push's 4096 bytes.
    phone = ec.generate_private_key(ec.SECP256R1())
    secret = b"0123456789abcdef"
    big = {"kind": "approval", "tag": "k:" + "x" * 60, "title": "é" * 120, "body": "ü" * 900, "alert": "1", "tab": "chat",
           "event": "123456789"}
    size = len(P.encrypt(P.fit(big), P._point(phone.public_key()), secret))
    check("largest message fits in 4096 bytes", size <= 4096, size)
    worst = {**big, "title": "😀" * 120, "body": "😀" * 900}
    body = P.fit(worst)
    size = len(P.encrypt(body, P._point(phone.public_key()), secret))
    check("an all-emoji one is shortened to fit, title kept", size <= 4096 and json.loads(body)["title"] == worst["title"]
          and json.loads(body)["body"].endswith("…"), size)
    check("a normal one is left whole", json.loads(P.fit(big)) == big)

    # 3. A send to a fake push server: it decrypts like the phone, checks VAPID, and keeps the headers.
    d = Path(tempfile.mkdtemp())
    wp = P.WebPush(d / "vapid.pem", "https://seimei.example.org")
    got = []

    async def endpoint(request):
        got.append((request.path, dict(request.headers), await request.read()))
        return web.Response(status=201 if request.path == "/up/ok" else 410)

    app = web.Application()
    app.router.add_post("/up/{name}", endpoint)
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, "127.0.0.1", 0)
    await site.start()
    port = site._server.sockets[0].getsockname()[1]
    sub = {"up": f"http://localhost:{port}/up/ok", "p256dh": P.b64(P._point(phone.public_key())), "auth": P.b64(secret)}
    data = {"kind": "approval", "tag": "approval", "title": "seimei wants to edit the niri config", "body": "x", "alert": "1"}
    check("send answers ok", await wp.send(sub, data, 600) == "ok")
    path, headers, raw = got[-1]
    check("the phone reads the same message", json.loads(decrypt(raw, phone, secret)) == data)
    check("aes128gcm, TTL, high urgency", headers.get("Content-Encoding") == "aes128gcm" and headers.get("TTL") == "600"
          and headers.get("Urgency") == "high", headers)
    check("topic is 32 url-safe chars at most", 0 < len(headers["Topic"]) <= 32 and all(c.isalnum() or c in "-_" for c in headers["Topic"]))
    t, k = (x.split("=", 1)[1] for x in headers["Authorization"][len("vapid "):].split(", "))
    check("VAPID key is seimei's", k == wp.public)
    head, claims, sig = t.split(".")
    c = json.loads(P.unb64(claims))
    check("VAPID claims: audience, expiry, contact", c["aud"] == f"http://localhost:{port}" and time.time() < c["exp"] <= time.time() + 24 * 3600
          and c["sub"] == "https://seimei.example.org", c)
    pub = ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), P.unb64(k))
    raw_sig = P.unb64(sig)
    try:
        pub.verify(encode_dss_signature(int.from_bytes(raw_sig[:32], "big"), int.from_bytes(raw_sig[32:], "big")),
                   f"{head}.{claims}".encode(), ec.ECDSA(hashes.SHA256()))
        ok = True
    except Exception as e:
        ok = e
    check("VAPID signature verifies", ok is True, ok)
    check("the VAPID key is kept (0600) and reused", (d / "vapid.pem").stat().st_mode & 0o777 == 0o600
          and P.WebPush(d / "vapid.pem", "x").public == wp.public)
    check("410 reads as gone", await wp.send({**sub, "up": f"http://localhost:{port}/up/old"}, data, 60) == "gone")
    check("endpoints: https or loopback http only", P.valid_endpoint("https://ntfy.example/upX")
          and P.valid_endpoint("http://localhost:2586/up") and not P.valid_endpoint("http://10.0.0.5/up")
          and not P.valid_endpoint("file:///etc/passwd") and not P.valid_endpoint("https://"))

    # 4. Dispatch: each phone the way it registered; a gone endpoint is forgotten.
    devs = [{"id": "F", "push": "tokF"}, {"id": "U", "push": sub}, {"id": "W", "push": {"ws": P.secret_hash("s3cret")}},
            {"id": "G", "push": {**sub, "up": f"http://localhost:{port}/up/old"}}]
    auth = FakeAuth(devs)
    fcm, stream = Fake(), P.Stream()
    push = P.Push(EventLog(d / "e.jsonl"), auth, fcm, d, wp, stream)
    got.clear()
    await push._send({"kind": "ping", "tag": "ping", "title": "hi", "body": "there", "alert": 1})
    check("fcm phone through Firebase", [t for t, _ in fcm.got] == ["tokF"], fcm.got)
    check("UnifiedPush phone through its endpoint", [p for p, _, _ in got] == ["/up/ok", "/up/old"], [p for p, _, _ in got])
    check("gone endpoint dropped", auth.device("G")["push"] is None and auth.device("U")["push"] == sub)
    check("ws phone's message waits for its connection", "ping" in stream.waiting.get("W", {}))
    check("ways", [push.way(x) for x in ("t", sub, {"ws": "h"})] == ["fcm", "up", "ws"])
    push.sender = None
    check("no Firebase key: fcm phones are skipped, others still sent", not push.can_send("tokF") and push.can_send(sub))

    # 5. The background connection, on a real websocket.
    app2 = web.Application()
    app2.router.add_get("/api/push/ws", lambda r: stream.serve(r, auth))
    r2 = web.AppRunner(app2)
    await r2.setup()
    s2 = web.TCPSite(r2, "127.0.0.1", 0)
    await s2.start()
    url = f"http://127.0.0.1:{s2._server.sockets[0].getsockname()[1]}/api/push/ws"
    async with aiohttp.ClientSession() as http:
        for name, h in (("no header", {}), ("wrong secret", {"Authorization": "Push W nope"}),
                        ("someone else's device", {"Authorization": "Push U s3cret"})):
            try:
                async with http.ws_connect(url, headers=h):
                    check(f"refused: {name}", False)
            except aiohttp.WSServerHandshakeError as e:
                check(f"refused: {name}", e.status == 401, e.status)
        async with http.ws_connect(url, headers={"Authorization": "Push W s3cret"}) as ws:
            first = await asyncio.wait_for(ws.receive_json(), 3)
            check("what waited arrives on connect", first.get("tag") == "ping" and first.get("title") == "hi", first)
            check("the connection does not count as the app being open", not push._is_open("W"))
            await push._send({"kind": "approval", "tag": "approval", "title": "card", "body": "b", "alert": 1})
            live = await asyncio.wait_for(ws.receive_json(), 3)
            check("live message arrives at once", live.get("tag") == "approval" and live.get("alert") == "1", live)
            await stream.drop("W")
            msg = await asyncio.wait_for(ws.receive(), 3)
            check("drop closes it", msg.type in (aiohttp.WSMsgType.CLOSE, aiohttp.WSMsgType.CLOSED, aiohttp.WSMsgType.CLOSING), msg.type)
        # Expired messages are not replayed.
        stream.waiting["W"] = {"old": (time.time() - 1, {"tag": "old"}), "new": (time.time() + 60, {"tag": "new"})}
        async with http.ws_connect(url, headers={"Authorization": "Push W s3cret"}) as ws:
            m = await asyncio.wait_for(ws.receive_json(), 3)
            try:
                extra = await asyncio.wait_for(ws.receive_json(), 0.5)
            except asyncio.TimeoutError:
                extra = None
            check("only unexpired messages are replayed", m["tag"] == "new" and extra is None, (m, extra))
    # A test notification goes to one phone only, with the app open too.
    push.sender = fcm
    fcm.got.clear()
    push.opened("F")
    push.test("F")
    await asyncio.sleep(0.1)
    check("test reaches its phone even when open, and no other", [(t, x.get("test")) for t, x in fcm.got] == [("tokF", "1")], fcm.got)
    await wp.close()
    await runner.cleanup()
    await r2.cleanup()
    print(f"{sum(res)}/{len(res)} passed")


asyncio.run(main())
