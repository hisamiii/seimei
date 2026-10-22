"""Login against the QA server (--login), black-box, so it holds for either implementation: the password and its
throttle, cookie sessions and logout, the auth.json tripwire (a forged session is refused and dropped, a pairing code
made behind the server is only reported), fresh logins for pairing, a phone's key login, unpairing, and a passkey
added and used through the page's own passkey.js on a virtual authenticator."""
import asyncio
import base64
import hashlib
import json
import os
import sys
import time
from pathlib import Path

import aiohttp
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec

sys.path.insert(0, os.path.dirname(__file__))
from h import async_playwright, browser

U = "http://localhost:8431"
O = {"Origin": U, "Content-Type": "application/json"}
J = {"Content-Type": "application/json"}
AUTH = Path(__file__).resolve().parents[1] / "qa/data/seimei/dev/auth.json"
res = []


def check(n, ok, x=""):
    res.append(bool(ok))
    print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300], flush=True)


def b64(b):
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def behind(change):
    """Another process (the CLI, or an agent) edits auth.json."""
    data = json.loads(AUTH.read_text())
    change(data)
    AUTH.write_text(json.dumps(data))
    st = AUTH.stat()
    os.utime(AUTH, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000))


async def alarms(web):
    evs = (await (await web.get(U + "/api/history?limit=50")).json())["events"]
    return [e["text"] for e in evs if e.get("type") == "system" and e.get("level") == "error"]


async def password(s, pw):
    return await s.post(U + "/api/auth/password", data=json.dumps({"password": pw}), headers=O)


async def passkeys():
    async with async_playwright() as p:
        b, ctx, pg = await browser(p, 1000, 800, touch=False, mobile=False)
        cdp = await ctx.new_cdp_session(pg)
        await cdp.send("WebAuthn.enable")
        await cdp.send("WebAuthn.addVirtualAuthenticator", {"options": {
            "protocol": "ctap2", "transport": "internal", "hasResidentKey": True, "hasUserVerification": True,
            "isUserVerified": True, "automaticPresenceSimulation": True}})
        await ctx.request.post(U + "/api/auth/password", data='{"password": "qa-password-123"}', headers=O)
        await pg.goto(U + "/")
        r = await pg.evaluate("async () => (await import('/static/any/passkey.js')).register('QA key')")
        check("a passkey is added from a fresh login", r.get("ok"), r)
        me = await (await ctx.request.get(U + "/api/auth/me")).json()
        check("... and counted", me.get("passkeys") == 1, me)
        r = await pg.evaluate("async () => (await import('/static/any/passkey.js')).register('again')")
        check("the same device cannot add a second one", not r.get("ok") and r.get("exists"), r)
        await ctx.request.post(U + "/api/auth/logout", data="{}", headers=O)
        r = await ctx.request.get(U + "/api/history")
        check("logged out", r.status == 401, r.status)
        r = await pg.evaluate("async () => (await import('/static/any/passkey.js')).login()")
        check("the passkey logs in", r.get("ok"), r)
        r = await ctx.request.get(U + "/api/history?limit=50")
        check("... with a working session", r.status == 200, r.status)
        texts = [e["text"] for e in (await r.json())["events"] if e.get("type") == "system"]
        check("adding it raised the alarm", any(t.startswith("A passkey was added. QA key") for t in texts), texts[-3:])
        keys = json.loads(AUTH.read_text())["passkeys"]
        check("its use is recorded", len(keys) == 1 and keys[0].get("used") and keys[0]["sign_count"] >= 0, keys)
        await b.close()


async def app_password():
    """The app's first run (#63): its server check, then pairing with the password the way the app sends it (OkHttp's
    JSON type, the server's Origin, the cookie by hand), and the page session ended once the phone has its key."""
    jar = aiohttp.DummyCookieJar()
    async with aiohttp.ClientSession(cookie_jar=jar) as app:
        r = await app.get(U + "/api/hello")
        check("hello says seimei", r.status == 200 and "seimei" in await r.json(), r.status)
        page = {"Origin": U, "Content-Type": "application/json; charset=utf-8"}
        r = await app.post(U + "/api/auth/password", data='{"password":"qa-password-123"}',
                           headers={**page, "Origin": "https://elsewhere.example"})
        check("a login from another origin is refused", r.status == 403, r.status)
        r = await app.post(U + "/api/auth/password", data='{"password":"qa-password-123"}', headers=page)
        cookie = r.headers.get("Set-Cookie", "").split(";")[0]
        check("the app's password login gets a cookie", r.status == 200 and "=" in cookie, (r.status, cookie))
        r = await app.post(U + "/api/devices/pair", data="{}", headers={**page, "Cookie": cookie})
        code = (await r.json()).get("code") if r.status == 200 else None
        check("... which makes a code", code, r.status)
        pk = ec.generate_private_key(ec.SECP256R1())
        pub = b64(pk.public_key().public_bytes(serialization.Encoding.DER, serialization.PublicFormat.SubjectPublicKeyInfo))
        r = await app.post(U + "/api/devices/claim", data=json.dumps({"code": code, "name": "qa-app", "public_key": pub}),
                           headers={"Content-Type": "application/json; charset=utf-8"})
        dev = (await r.json()).get("device") if r.status == 200 else None
        check("... that pairs the phone", dev, r.status)
        r = await app.post(U + "/api/auth/logout", data="{}", headers={**page, "Cookie": cookie})
        check("the app ends the page session", r.status == 200, r.status)
        r = await app.get(U + "/api/history", headers={"Cookie": cookie})
        check("... so its cookie no longer works", r.status == 401, r.status)
        nonce = (await (await app.post(U + "/api/devices/challenge", json={"device": dev}, headers=J)).json())["nonce"]
        sig = b64(pk.sign(b"seimei device login\n" + nonce.encode(), ec.ECDSA(hashes.SHA256())))
        r = await app.post(U + "/api/devices/login", json={"device": dev, "nonce": nonce, "signature": sig}, headers=J)
        tok = (await r.json()).get("token") if r.status == 200 else None
        check("the phone logs in with its own key", tok, r.status)
        r = await app.post(U + f"/api/devices/{dev}/revoke", data="{}", headers={"Authorization": f"Bearer {tok}", **J})
        check("... and unpairs itself", r.status == 200, r.status)


async def main():
    async with aiohttp.ClientSession() as web, aiohttp.ClientSession() as phone, aiohttp.ClientSession() as anon:
        r = await anon.get(U + "/")
        check("the page asks for a login first", r.status == 200 and "login" in (await r.text()).lower())
        r = await anon.get(U + "/api/history")
        check("the API needs a login", r.status == 401, r.status)
        r = await password(anon, "wrong password")
        check("a wrong password is refused", r.status == 401 and (await r.json()) == {"ok": False, "wait": 0})
        r = await password(web, "qa-password-123")
        check("the right one logs in with a cookie", r.status == 200 and "seimei" in r.cookies)
        r = await web.get(U + "/api/history")
        check("... that works", r.status == 200, r.status)
        r = await web.post(U + "/api/auth/password", data=b"x" * (70 << 10), headers=O)
        check("a big public body is refused", r.status == 413, r.status)

        token = "forged-qa-token"
        key = hashlib.sha256(token.encode()).hexdigest()
        behind(lambda d: d["sessions"].update({key: {"created": time.time(), "exp": time.time() + 999, "how": "x", "agent": ""}}))
        r = await anon.get(U + "/api/history", headers={"Authorization": f"Bearer {token}"})
        check("a session written behind the server is refused", r.status == 401, r.status)
        check("... and dropped", key not in json.loads(AUTH.read_text())["sessions"])
        check("... with an alarm", any(t.startswith("Login data was forged") for t in await alarms(web)), await alarms(web))
        behind(lambda d: d["pairing"].append({"hash": "0" * 64, "exp": time.time() + 60}))
        await web.get(U + "/api/auth/me")
        check("a pairing code made behind it is reported", any(t.startswith("A pairing code was made") for t in await alarms(web)))
        check("... but kept", any(c["hash"] == "0" * 64 for c in json.loads(AUTH.read_text())["pairing"]))

        r = await phone.post(U + "/api/devices/claim", json={"code": "AAAA-AAAA", "name": "x", "public_key": "x"}, headers=J)
        check("a wrong code pairs nothing", r.status == 401, r.status)
        code = (await (await web.post(U + "/api/devices/pair", data="{}", headers=O)).json())["code"]
        pk = ec.generate_private_key(ec.SECP256R1())
        pub = b64(pk.public_key().public_bytes(serialization.Encoding.DER, serialization.PublicFormat.SubjectPublicKeyInfo))
        dev = (await (await phone.post(U + "/api/devices/claim", json={"code": code.lower(), "name": " qa-auth ", "public_key": pub}, headers=J)).json())["device"]
        nonce = (await (await phone.post(U + "/api/devices/challenge", json={"device": dev}, headers=J)).json())["nonce"]
        bad = b64(pk.sign(b"something else" + nonce.encode(), ec.ECDSA(hashes.SHA256())))
        r = await phone.post(U + "/api/devices/login", json={"device": dev, "nonce": nonce, "signature": bad}, headers=J)
        check("a wrong signature does not log in", r.status == 401, r.status)
        nonce = (await (await phone.post(U + "/api/devices/challenge", json={"device": dev}, headers=J)).json())["nonce"]
        sig = b64(pk.sign(b"seimei device login\n" + nonce.encode(), ec.ECDSA(hashes.SHA256())))
        r = await phone.post(U + "/api/devices/login", json={"device": dev, "nonce": nonce, "signature": sig}, headers=J)
        tok = (await r.json()).get("token")
        check("the phone logs in with its key", r.status == 200 and tok)
        r = await phone.post(U + "/api/devices/login", json={"device": dev, "nonce": nonce, "signature": sig}, headers=J)
        check("a nonce works once", r.status == 401, r.status)
        A = {"Authorization": f"Bearer {tok}", "Content-Type": "application/json"}
        mine = [d for d in (await (await phone.get(U + "/api/devices", headers=A)).json())["devices"] if d["this"]]
        check("it sees itself, named as typed", len(mine) == 1 and mine[0]["name"] == "qa-auth", mine)
        r = await phone.post(U + "/api/devices/pair", data="{}", headers=A)
        check("a phone cannot pair further phones", r.status == 403, r.status)
        check("pairing raised the alarm", any(t.startswith("A phone was paired. qa-auth") for t in await alarms(web)))
        r = await web.post(U + f"/api/devices/{dev}/revoke", data="{}", headers=O)
        r = await phone.get(U + "/api/history", headers=A)
        check("unpairing ends its session", r.status == 401, r.status)

        r = await web.post(U + "/api/auth/logout", data="{}", headers=O)
        check("logout answers", r.status == 200, r.status)
        r = await web.get(U + "/api/history")
        check("... and the cookie no longer works", r.status == 401, r.status)

        await app_password()
        await passkeys()

        # Last: it locks the password out for the rest of this server's life.
        waits = [(await (await password(anon, f"wrong {i}")).json())["wait"] for i in range(5)]
        check("five wrong passwords lock it for 30 s", waits == [0, 0, 0, 0, 30], waits)
        r = await password(anon, "qa-password-123")
        check("... even for the right one", r.status == 401 and (await r.json())["wait"] > 0, r.status)

    print(f"{sum(res)}/{len(res)} passed")
    sys.exit(0 if all(res) else 1)


asyncio.run(main())
