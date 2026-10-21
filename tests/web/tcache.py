"""#53 on the web: the chat and agent pages kept in localStorage, drawn before the server answers, then only what is
new fetched (history?limit=0, ?after=, ?from=) and merged without doubles; the login page clears them."""
import asyncio, json, re, sys; sys.path.insert(0, __import__('os').path.dirname(__file__))
from h import *
U = "http://localhost:8431"; O = {"Origin": U, "Content-Type": "application/json"}
res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300], flush=True)
async def main():
    async with async_playwright() as p:
        b, ctx, pg = await browser(p, 412, 892, dpr=2)
        await ctx.request.post(U + "/api/auth/password", data='{"password": "qa-password-123"}', headers=O)
        wid = next(w["id"] for w in (await (await ctx.request.get(U + "/api/workers")).json())["workers"] if w["name"] == "app-tester")
        await pg.goto(U + "/"); await pg.wait_for_timeout(2500)
        keys = lambda: pg.evaluate("Object.keys(localStorage).filter(k => k.startsWith('seimei.keep.'))")
        chat = await pg.evaluate("JSON.parse(localStorage.getItem('seimei.keep.chat') || 'null')")
        check("the chat is kept", chat and len(chat["events"]) > 5, chat and len(chat["events"]))
        await pg.goto(U + "/#agents"); await pg.wait_for_timeout(1500)
        await pg.locator("button.agent", has_text="app-tester").first.click(); await pg.wait_for_timeout(2000)
        check("worker page shows", "3 bugs fixed" in await pg.locator(".sthread").inner_text())
        await pg.go_back(); await pg.wait_for_timeout(800)
        await pg.locator("button.agent", has_text="prefill-tuning").first.click(); await pg.wait_for_timeout(2000)
        check("session page shows", "Plan" in await pg.locator(".sthread").inner_text())
        await pg.go_back(); await pg.wait_for_timeout(1500)
        k = await keys()
        check("worker and session pages kept", f"seimei.keep.worker-{wid}" in k and any(x.startswith("seimei.keep.session-") for x in k), k)

        # The server answers slowly: the kept copies draw first, and only what is new is asked for.
        asked = []
        async def slow(route):
            asked.append(route.request.url.replace(U, ""))
            await asyncio.sleep(3); await route.continue_()
        await pg.route(re.compile(r".*/api/(history|workers/[^/]+(\?.*)?$|sessions/\d+(\?.*)?$)"), slow)
        await pg.goto(U + "/"); await pg.wait_for_timeout(1000)
        n = await pg.locator("#thread .msg, #thread .bubble, #thread > *").count()
        check("a reload draws the kept chat before the server answers", n > 3, n)
        await pg.wait_for_timeout(3000)
        check("and asks only history?limit=0", any(a.startswith("/api/history?limit=0") for a in asked) and not any("limit=200" in a for a in asked), asked)
        await pg.goto(U + "/#agents"); await pg.wait_for_timeout(1500)
        asked.clear()
        await pg.locator("button.agent", has_text="app-tester").first.click(); await pg.wait_for_timeout(800)
        check("the kept worker page draws before the server answers", "3 bugs fixed" in await pg.locator(".sthread").inner_text())
        await pg.wait_for_timeout(3500)
        check("it asks only for events after its last", any(re.match(rf"/api/workers/{wid}\?after=\d+$", a) for a in asked), asked)
        await pg.unroute(re.compile(r".*/api/(history|workers/[^/]+(\?.*)?$|sessions/\d+(\?.*)?$)"))
        await ctx.request.post(U + f"/api/workers/{wid}/send", data=json.dumps({"text": "web cache check"}), headers=O)
        await pg.wait_for_timeout(4500)
        t = await pg.locator(".sthread").inner_text()
        check("a new message merges in once", t.count("(fake) got: web cache check") == 1, t[-400:])
        await pg.go_back(); await pg.wait_for_timeout(500)
        await pg.reload(); await pg.wait_for_timeout(2500)
        await pg.goto(U + "/#agents"); await pg.wait_for_timeout(1500)
        await pg.locator("button.agent", has_text="app-tester").first.click(); await pg.wait_for_timeout(2500)
        t = await pg.locator(".sthread").inner_text()
        check("no doubles after reopening from the kept copy", t.count("(fake) got: web cache check") == 1 and t.count("web cache check") == 2, t[-400:])
        await pg.go_back(); await pg.wait_for_timeout(500)

        # Logging out leads to the login page, which clears what was kept.
        await pg.evaluate("fetch('api/auth/logout', {method: 'POST', headers: {'Content-Type': 'application/json'}, body: '{}'})")
        await pg.wait_for_timeout(500)
        await pg.reload(); await pg.wait_for_timeout(1500)
        check("the login page shows", await pg.locator("#pwform").count() == 1)
        check("nothing kept after logging out", await keys() == [], await keys())
        check("no page errors", not pg.errs, pg.errs)
        await b.close()
    print(f"{sum(res)}/{len(res)} passed")
asyncio.run(main())
