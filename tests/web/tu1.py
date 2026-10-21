"""U1: optimistic send: pending bubble at once, one bubble after the echo, failure offers retry."""
import asyncio, sys; sys.path.insert(0, __import__('os').path.dirname(__file__))
from h import *
res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, x)
async def main():
    async with async_playwright() as p:
        b, ctx, pg = await browser(p)
        errs = []; pg.on("pageerror", lambda e: errs.append(str(e)))
        await ctx.request.post(BASE + "/api/auth/password", data='{"password": "qa-password-123"}', headers={"Origin": BASE, "Content-Type": "application/json"})
        await pg.goto(BASE + "/"); await pg.wait_for_timeout(1500)
        async def slow(route):
            await asyncio.sleep(3); await route.continue_()
        await pg.route("**/api/send", slow)
        await pg.fill("#input", "optimistic one"); await pg.click("#send"); await pg.wait_for_timeout(300)
        check("pending bubble at once", await pg.locator(".msg.user.pending:has-text('optimistic one')").count() == 1)
        await pg.wait_for_timeout(4500)
        n = await pg.locator(".msg.user:has-text('optimistic one')").count()
        check("one bubble after the echo, not pending", n == 1 and await pg.locator(".msg.user.pending").count() == 0, n)
        await pg.unroute("**/api/send")
        await pg.route("**/api/send", lambda r: r.abort())
        await pg.fill("#input", "doomed message"); await pg.click("#send"); await pg.wait_for_timeout(800)
        check("failure shows retry", await pg.locator(".msg.user.failed .retry").count() == 1)
        await pg.unroute("**/api/send")
        await pg.click(".msg.user.failed .retry"); await pg.wait_for_timeout(2000)
        n = await pg.locator(".msg.user:has-text('doomed message')").count()
        check("retry sends it, one bubble", n == 1 and await pg.locator(".msg.user.failed").count() == 0, n)
        await pg.reload(); await pg.wait_for_timeout(1500)
        check("after reload exactly one of each", await pg.locator(".msg.user:has-text('optimistic one')").count() == 1 and await pg.locator(".msg.user:has-text('doomed message')").count() == 1)
        check("no page errors", not errs, errs)
        await b.close()
    print(f"{sum(res)}/{len(res)} passed")
asyncio.run(main())
