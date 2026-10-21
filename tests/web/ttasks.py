"""#55 on the web: the Tasks lane. Projects in order with their tint rule, done folded, paused shut, the user's
items tick and untick (and reach seimei), a task with a shikigami opens its page, live updates over the stream,
kept for the next visit."""
import asyncio, json, re, sys; sys.path.insert(0, __import__('os').path.dirname(__file__))
from h import *
U = "http://localhost:8431"; O = {"Origin": U, "Content-Type": "application/json"}
J = SHOTS + "/"
res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300], flush=True)
async def main():
    async with async_playwright() as p:
        b, ctx, pg = await browser(p, 412, 892, dpr=2)
        await ctx.request.post(U + "/api/auth/password", data='{"password": "qa-password-123"}', headers=O)
        api = await (await ctx.request.get(U + "/api/tasks")).json()
        check("the board is seeded", len(api["tasks"]) == 13 and {"#55", "t1", "t7"} <= {t["id"] for t in api["tasks"]}, [t["id"] for t in api["tasks"]])
        await pg.goto(U + "/"); await pg.wait_for_timeout(2000)
        await pg.locator("#lt-tasks").click(); await pg.wait_for_timeout(600)
        check("Tasks lane shows, composer hidden", await pg.locator("#tasks").is_visible() and not await pg.locator("#composer").is_visible())
        heads = await pg.locator(".pj .ph b").all_inner_texts()
        check("projects: running first, paused last", heads == ["seimei", "discord tiled", "desktop", "yah cleanup", "minecraft harness"], heads)
        sub = await pg.locator(".pj .ph small").all_inner_texts()
        check("headers say running / open / you / paused", sub == ["running", "open", "you", "paused", "paused"], sub)
        seimei = pg.locator(".pj").first
        lines = await seimei.locator(".ln").all_inner_texts()
        check("seimei: #55 running first, then the user's two", len(lines) == 3 and lines[0].startswith("#55") and "Back up the signing key" in lines[1], lines)
        check("done folds into one line", (await seimei.locator(".fold3").inner_text()).strip() == "5 done")
        check("paused projects stay shut", await pg.locator(".pj.held.shut").count() == 2 and await pg.locator(".pj.held .ln").count() == 0)
        rule = await seimei.evaluate("e => getComputedStyle(e, '::before').backgroundColor")
        check("a tinted rule per project", rule not in ("", "rgba(0, 0, 0, 0)"), rule)
        await pg.screenshot(path=J + "tasks-web.png")
        await seimei.locator(".fold3").click(); await pg.wait_for_timeout(300)
        check("the fold opens the done list", await seimei.locator(".ln.s-done").count() == 5)
        await seimei.locator(".fold3").click()
        await pg.locator(".pj.held .ph").first.click(); await pg.wait_for_timeout(300)
        check("a paused project opens on a tap", await pg.locator(".pj.held .ln").count() == 1)
        # The user's own item: tick, it reaches the laptop and folds into done; untick brings it back.
        await seimei.locator("button.g.tick").first.click(); await pg.wait_for_timeout(1200)
        api = await (await ctx.request.get(U + "/api/tasks")).json()
        t1 = next(t for t in api["tasks"] if t["id"] == "t1")
        check("ticking marks it done on the laptop", t1["state"] == "done" and t1["done_at"], t1)
        check("and it folds into done", (await seimei.locator(".fold3").inner_text()).strip() == "6 done")
        await seimei.locator(".fold3").click(); await pg.wait_for_timeout(300)
        await seimei.locator(".ln.s-done", has_text="Back up the signing key").locator("button.g").click(); await pg.wait_for_timeout(1200)
        api = await (await ctx.request.get(U + "/api/tasks")).json()
        check("unticking reopens it", next(t for t in api["tasks"] if t["id"] == "t1")["state"] == "open")
        r = await ctx.request.post(U + "/api/tasks/55/tick", data='{"done": true}', headers=O)
        check("other owners' items cannot be ticked", r.status == 403, r.status)
        # Live: a change from elsewhere (another phone ticking) shows without a reload.
        await ctx.request.post(U + "/api/tasks/t2/tick", data='{"done": true}', headers=O)
        await pg.wait_for_timeout(1200)
        check("a change on the laptop shows live", (await seimei.locator(".fold3").inner_text()).strip() in ("6 done", "hide done")
              and await seimei.locator(".ln.s-done", has_text="M4 step 0 tests").count() == 1)
        # Tap a task with a shikigami: its page opens (QA has no such shikigami, so the page is asked for).
        asked = []
        pg.on("request", lambda r: asked.append(r.url))
        await seimei.locator("button.ln", has_text="Task board").click(); await pg.wait_for_timeout(1500)
        check("a task with a shikigami opens its page", any("/api/workers/3905ef87" in u for u in asked), asked)
        k = await pg.evaluate("JSON.parse(localStorage.getItem('seimei.keep.tasks') || 'null')")
        check("the board is kept in the browser", k and len(k["tasks"]) >= 13)
        check("no page errors (but the QA-missing shikigami)", not [e for e in pg.errs if "404" not in e], pg.errs)
        await b.close()
    print(f"{sum(res)}/{len(res)} passed")
asyncio.run(main())
