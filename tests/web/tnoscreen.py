"""#52: the Screen tab and the display stream are gone; the other tabs still work."""
import asyncio, json, sys; sys.path.insert(0, __import__('os').path.dirname(__file__))
from h import *
U = "http://localhost:8431"; J = SHOTS + "/"
res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300], flush=True)
async def main():
    async with async_playwright() as p:
        b, ctx, pg = await browser(p, 412, 892, dpr=2)
        await ctx.request.post(U + "/api/auth/password", data='{"password": "qa-password-123"}', headers={"Origin": U, "Content-Type": "application/json"})
        for path in ("/api/screen?mode=screen", "/api/desktop", "/api/stream/screen"):
            r = await ctx.request.get(U + path)
            check(f"{path} is gone", r.status == 404, r.status)
        await pg.goto(U + "/"); await pg.wait_for_timeout(2500)
        tabs = await pg.locator(".nav-item").evaluate_all("els => els.map(e => e.dataset.tab)")
        check("three tabs: chat, agents, usage", tabs == ["chat", "agents", "usage"], tabs)
        check("no Screen page in the DOM", await pg.locator("#screen").count() == 0)
        for t in ("agents", "usage", "chat"):
            await pg.locator(f'.nav-item[data-tab="{t}"]').click(); await pg.wait_for_timeout(800)
            check(f"{t} tab shows", await pg.evaluate("document.body.dataset.tab") == t)
        await pg.locator('.nav-item[data-tab="usage"]').click(); await pg.wait_for_timeout(1200)
        await pg.screenshot(path=J + "noscreen-usage.png")
        await pg.goto(U + "/#screen"); await pg.wait_for_timeout(1500)
        check("an old #screen link falls back to the chat", await pg.evaluate("document.body.dataset.tab") == "chat")
        check("no page errors", not pg.errs, pg.errs)
        await b.close()
    print(f"{sum(res)}/{len(res)} passed")
asyncio.run(main())
