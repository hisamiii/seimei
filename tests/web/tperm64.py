"""#64 web: permission cards (QA !perm: a shikigami's command card with Claude Code's choices, an edit as a diff, a
plan card), answering with a rule, a note and Go ahead; then seimei's own card with its countdown, settled by a chat
message."""
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
        await pg.fill("#input", "!perm"); await pg.click("#send"); await pg.wait_for_timeout(2500)
        cards = pg.locator(".wcard.approval.perm:not(.settled)")
        check("three permission cards", await cards.count() == 3, await cards.count())
        cmd = pg.locator(".wcard.approval.perm:has-text('wants to run a command')")
        check("command card shows the description", await cmd.locator(".sub").inner_text() == "Run the unit tests")
        rows = await cmd.locator(".choice").all_inner_texts()
        check("its choices: always allow rule, edits, broad rule flagged", len(rows) == 3 and "uv run pytest:*" in rows[0]
              and "in this project" in rows[0] and rows[1].startswith("Allow edits") and "runs any code" in rows[2], rows)
        check("no countdown for a shikigami", (await cmd.locator(".foot .left").inner_text()) == "Waits for you")
        edit = pg.locator(".wcard.approval.perm:has-text('wants to edit a file')")
        check("edit shows a diff", await edit.locator(".body .del").count() == 1 and await edit.locator(".body .add").count() == 1)
        plan = pg.locator(".wcard.approval.perm:has-text('start on its plan')")
        check("plan rendered", await plan.locator(".body.plan li").count() == 3)
        check("plan buttons", (await plan.locator(".foot button").all_inner_texts())[-2:] == ["Keep planning", "Go ahead"],
              await plan.locator(".foot button").all_inner_texts())
        await pg.evaluate("document.querySelector('.wcard.approval.perm').scrollIntoView()")
        await shot(pg, "perm64-cards")
        # Answer: the rule, a note on the edit, Go ahead.
        await cmd.locator(".choice").first.click()
        await edit.locator(".foot .btn.icon").click()
        await edit.locator("textarea.note").fill("use weight_of(seed) instead")
        await shot(pg, "perm64-note")
        await edit.locator("button:has-text('Deny with note')").click()
        await plan.locator("button:has-text('Go ahead')").click()
        await pg.wait_for_timeout(2500)
        hist = await (await ctx.request.get(BASE + "/api/history?limit=50")).json()
        evs = hist.get("events", hist) if isinstance(hist, dict) else hist
        reply = next((e["text"] for e in reversed(evs) if e["type"] == "assistant" and e["text"].startswith("Cards came back")), "")
        check("the server got rule, note and go ahead", "approved (s0)" in reply and "denied (deny, use weight_of(seed) instead)" in reply
              and "approved (once)" in reply, reply)
        check("settled: always allowed rule", "Always allowed uv run pytest:* in this project" in await cmd.locator(".foot .left").inner_text(),
              await cmd.locator(".foot .left").inner_text())
        check("settled: denied with the note", "use weight_of(seed) instead" in await edit.locator(".foot .left").inner_text())
        left = await pg.locator(".wcard.approval.perm.settled button:not(.rbtn)").evaluate_all("bs => bs.map(b => b.outerHTML.slice(0, 120))")
        check("settled cards keep no buttons", not left, left)
        await shot(pg, "perm64-settled")
        # seimei's own card: countdown, and a chat message settles it.
        await pg.fill("#input", "!permseimei"); await pg.click("#send"); await pg.wait_for_timeout(2000)
        own = pg.locator(".wcard.approval.perm:not(.settled)")
        check("seimei's card counts down", "Auto-denies in 30 min" in await own.locator(".foot .left").inner_text(),
              await own.locator(".foot .left").inner_text())
        await pg.fill("#input", "no, wait"); await pg.click("#send"); await pg.wait_for_timeout(2000)
        check("a chat message settled it", await pg.locator(".wcard.approval.perm:not(.settled)").count() == 0)
        check("it says why", "you wrote in the chat" in await pg.locator(".wcard.approval.perm").last.locator(".foot .left").inner_text())
        await pg.reload(); await pg.wait_for_timeout(2000)
        check("after reload the outcomes stay", await pg.locator(".wcard.approval.perm.settled").count() == 4,
              await pg.locator(".wcard.approval.perm.settled").count())
        check("no page errors", not pg.errs, pg.errs)
        await b.close()
    print(f"{sum(res)}/{len(res)} passed")
asyncio.run(main())
