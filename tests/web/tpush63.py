"""#63 against the QA server (--login): /api/hello is public and says nothing else; a scripted phone pairs, logs in
with its key, reads the VAPID key, registers UnifiedPush (bad endpoints refused), switches to the background connection,
gets a test notification on the websocket, and loses it when unpaired. No real phone involved."""
import asyncio
import base64
import json

import aiohttp
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec

U = "http://localhost:8431"
O = {"Origin": U, "Content-Type": "application/json"}
res = []


def check(n, ok, x=""):
    res.append(bool(ok))
    print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300], flush=True)


def b64(b):
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


async def main():
    async with aiohttp.ClientSession() as web, aiohttp.ClientSession() as phone:
        r = await phone.get(U + "/api/hello")
        body = await r.json()
        check("hello is public", r.status == 200 and body == {"seimei": 1}, (r.status, body))
        r = await phone.get(U + "/api/devices/push")
        check("push info needs a login", r.status == 401, r.status)

        await web.post(U + "/api/auth/password", data=json.dumps({"password": "qa-password-123"}), headers=O)
        code = (await (await web.post(U + "/api/devices/pair", data="{}", headers=O)).json())["code"]
        key = ec.generate_private_key(ec.SECP256R1())
        pub = b64(key.public_key().public_bytes(serialization.Encoding.DER, serialization.PublicFormat.SubjectPublicKeyInfo))
        j = {"Content-Type": "application/json"}
        dev = (await (await phone.post(U + "/api/devices/claim", json={"code": code, "name": "qa-phone-63", "public_key": pub}, headers=j)).json())["device"]
        nonce = (await (await phone.post(U + "/api/devices/challenge", json={"device": dev}, headers=j)).json())["nonce"]
        sig = b64(key.sign(b"seimei device login\n" + nonce.encode(), ec.ECDSA(hashes.SHA256())))
        token = (await (await phone.post(U + "/api/devices/login", json={"device": dev, "nonce": nonce, "signature": sig}, headers=j)).json())["token"]
        A = {"Authorization": f"Bearer {token}", "Content-Type": "application/json"}

        info = await (await phone.get(U + "/api/devices/push", headers=A)).json()
        check("VAPID key handed to the phone (65-byte point)", info.get("vapid") and len(base64.urlsafe_b64decode(info["vapid"] + "==")) == 65
              and info.get("way") is None, info)
        r = await web.get(U + "/api/devices/push")
        check("a browser session cannot register", r.status == 403, r.status)

        ua = ec.generate_private_key(ec.SECP256R1())
        up = {"endpoint": "https://ntfy.example.org/upQA63?up=1",
              "p256dh": b64(ua.public_key().public_bytes(serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint)),
              "auth": b64(b"0123456789abcdef")}
        for bad in ({**up, "endpoint": "http://192.168.1.5/up"}, {**up, "endpoint": "ftp://x/y"}, {**up, "p256dh": ""}):
            r = await phone.post(U + "/api/devices/push", json={"up": bad}, headers=A)
            check(f"refused endpoint {bad['endpoint'][:24]} / key {bool(bad['p256dh'])}", r.status == 400, r.status)
        r = await phone.post(U + "/api/devices/push", json={"up": up}, headers=A)
        check("UnifiedPush registered and sendable", r.status == 200 and (await r.json()).get("sending") is True)
        info = await (await phone.get(U + "/api/devices/push", headers=A)).json()
        check("way reads up", info.get("way") == "up" and info.get("sending"), info)
        devs = (await (await web.get(U + "/api/devices")).json())["devices"]
        check("the web list shows notifications on, no secrets", any(d["id"] == dev and d["push"] is True for d in devs)
              and "ntfy" not in json.dumps(devs), devs)

        r = await phone.post(U + "/api/devices/push", json={"ws": True}, headers=A)
        out = await r.json()
        secret = out.get("ws")
        check("background connection registered, secret returned once", r.status == 200 and secret and len(secret) >= 40 and out.get("sending"), out)
        r = await phone.get(U + "/api/push/ws", headers={"Authorization": f"Push {dev} wrong"})
        check("websocket refuses a wrong secret", r.status == 401, r.status)
        r = await phone.get(U + "/api/push/ws", headers={"Authorization": f"Bearer {token}"})
        check("a login token does not open the stream", r.status == 401, r.status)
        r = await phone.get(U + "/api/history", headers={"Authorization": f"Push {dev} {secret}"})
        check("the stream secret opens nothing else", r.status == 401, r.status)
        async with phone.ws_connect(U + "/api/push/ws", headers={"Authorization": f"Push {dev} {secret}"}) as ws:
            r = await phone.post(U + "/api/devices/push/test", json={}, headers=A)
            check("test send accepted", r.status == 200, r.status)
            m = await asyncio.wait_for(ws.receive_json(), 5)
            check("test notification arrives on the websocket", m.get("tag") == "test" and m.get("test") == "1" and m.get("body"), m)
            r = await web.post(U + f"/api/devices/{dev}/revoke", data="{}", headers=O)
            check("unpaired", r.status == 200, r.status)
            msg = await asyncio.wait_for(ws.receive(), 5)
            check("unpairing closes the stream", msg.type in (aiohttp.WSMsgType.CLOSE, aiohttp.WSMsgType.CLOSED), msg.type)
        r = await phone.get(U + "/api/push/ws", headers={"Authorization": f"Push {dev} {secret}"})
        check("and the secret no longer works", r.status == 401, r.status)
    print(f"{sum(res)}/{len(res)} passed")


asyncio.run(main())
