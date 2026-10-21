"""#64/#74 web: a shikigami's details show Model / Effort / Permissions as tappable rows and a waiting card on top; the
Change sheet sets effort, model and permissions in one Apply, and the page shows the new values."""
import asyncio, json, sys; sys.path.insert(0, __import__('os').path.dirname(__file__))
from h import *
res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300])
H = {"Origin": BASE, "Content-Type": "application/json"}
async def main():
    async with async_playwright() as p:
        b, ctx, pg = await browser(p)
        await ctx.request.post(BASE + "/api/auth/password", data='{"password": "qa-password-123"}', headers=H)
        await pg.goto(BASE + "/"); await pg.wait_for_timeout(1500)
        await pg.fill("#input", "!perm"); await pg.click("#send"); await pg.wait_for_timeout(1500)
        await pg.click("#nav-agents"); await pg.wait_for_timeout(800)
        await pg.click(".agent:has-text('runner')"); await pg.wait_for_timeout(1500)
        await pg.click("[aria-label*='details']"); await pg.wait_for_timeout(800)
        check("waiting card on top", "Waits for your OK: run a command" in await pg.locator(".dwait").inner_text())
        rows = await pg.locator(".kv dt.tune").all_inner_texts()
        check("three tappable rows", rows == ["Model", "Effort", "Permissions"], rows)
        await shot(pg, "tune74-details")
        await pg.click(".kv dd.tune >> nth=1"); await pg.wait_for_timeout(500)
        check("change sheet", await pg.locator(".change-h").inner_text() == "Change runner")
        await pg.click(".seg button:has-text('xhigh')")
        await pg.click(".seg button:has-text('sonnet')")
        await pg.click(".radio:has-text('Allow edits')")
        await shot(pg, "tune74-sheet")
        await pg.click("button:has-text('Apply')"); await pg.wait_for_timeout(1200)
        txt = await pg.locator(".kv").inner_text()
        check("details show the new values", "sonnet" in txt and "Allow edits" in txt and "xhigh" in txt, txt)
        ws = (await (await ctx.request.get(BASE + "/api/workers")).json())
        ws = ws.get("workers", ws) if isinstance(ws, dict) else ws
        r = next(w for w in ws if w["name"] == "runner")
        check("server has them", r["model"] == "sonnet" and r["mode"] == "acceptEdits" and "xhigh" in (r.get("effort_next") or r.get("effort") or ""), r)
        check("history kept", len(r.get("tunes") or []) == 2, r.get("tunes"))
        check("no page errors", not pg.errs, pg.errs)
        await b.close()
    print(f"{sum(res)}/{len(res)} passed")
asyncio.run(main())
