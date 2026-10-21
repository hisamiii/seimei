"""#47 metrics view on the web: Usage tab section (rows, expand, show all) and the worker details sheet facts."""
import asyncio, json, sys; sys.path.insert(0, __import__('os').path.dirname(__file__))
from h import *
U = "http://localhost:8431"; J = SHOTS + "/"
res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300], flush=True)
async def main():
    async with async_playwright() as p:
        b, ctx, pg = await browser(p, 412, 892, dpr=2)
        r = await ctx.request.post(U + "/api/auth/password", data='{"password": "qa-password-123"}', headers={"Origin": U, "Content-Type": "application/json"})
        api = await (await ctx.request.get(U + "/api/metrics")).json()
        ws = api["workers"]
        check("api lists every shikigami with metrics", len(ws) >= 4 and all({"usd", "tokens", "wall", "busy", "asks", "followups", "reopened"} <= set(w) for w in ws), ws[:1])
        tester = next(w for w in ws if w["name"] == "app-tester")
        check("tokens come from its transcript", tester["tokens"]["input"] == 120000 and tester["usd"] > 0, tester)
        out = await (await ctx.request.get(U + "/api/metrics")).text()
        await pg.goto(U + "/#usage"); await pg.wait_for_timeout(2500)
        sec = pg.locator(".metrics")
        check("Usage has the Shikigami section", await sec.locator(".crow").count() == min(len(ws), 8) and "Shikigami" in await sec.inner_text(), await sec.inner_text())
        check("its summary line", "shikigami," in await sec.locator(".usage-note").first.inner_text())
        row = sec.locator(".crow", has_text="app-tester")
        await row.click(); await pg.wait_for_timeout(300)
        facts = sec.locator(".mfacts:not([hidden])")
        check("a tap opens its facts", await facts.count() == 1 and "cache read" in await facts.inner_text() and "Rounds" in await facts.inner_text(), await facts.all_inner_texts())
        await row.scroll_into_view_if_needed()
        await pg.screenshot(path=J + "metrics-usage.png")
        await pg.goto(U + "/#agents"); await pg.wait_for_timeout(1500)
        await pg.locator("button.agent", has_text="app-tester").first.click(); await pg.wait_for_timeout(1500)
        await pg.get_by_role("button", name="Session details").first.click(); await pg.wait_for_timeout(600)
        kv = await pg.locator("dl.kv:not(.mfacts)").last.inner_text()
        check("the details sheet has its cost and time", "Cost" in kv and "at API prices" in kv and "busy" in kv and "Rounds" in kv, kv)
        await pg.screenshot(path=J + "metrics-details.png")
        check("no page errors", not pg.errs, pg.errs)
        await b.close()
    print(f"{sum(res)}/{len(res)} passed")
asyncio.run(main())
