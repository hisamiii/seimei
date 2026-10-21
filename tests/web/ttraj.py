"""#37: Chat | Trajectory lanes, fold lines, reply anchors, Reply in chat, unseen badge."""
import asyncio, sys, requests; sys.path.insert(0, __import__('os').path.dirname(__file__))
from h import *
U = "http://localhost:8431"; O = {"Origin": U}
J = SHOTS + "/"
res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, x, flush=True)
async def send(pg, text):
    await pg.fill("#input", text); await pg.click("#send")
async def main():
    async with async_playwright() as p:
        b, ctx, pg = await browser(p, 412, 892, dpr=2)
        await ctx.request.post(U + "/api/auth/password", data='{"password": "qa-password-123"}', headers={"Origin": U, "Content-Type": "application/json"})
        await pg.goto(U + "/"); await pg.wait_for_timeout(1500)
        await pg.screenshot(path=J + "tr-chat.png")
        chat = pg.locator("#thread")
        fold = chat.locator(".fold").first
        check("chat folds the reports into one line", (await fold.inner_text()).startswith("app-tester finished, asr-bench failed and 3 more") and "Trajectory" in await fold.inner_text(), await fold.inner_text())
        check("no report cards or restart notes in the chat", await chat.locator(".wcard.done, .wcard.failed").count() == 0 and "Session is back" not in await chat.inner_text())
        check("the asking card stays in the chat", await chat.locator(".wcard.asking").count() == 1)
        re = chat.locator(".msg.assistant .re")
        check("a reply after the question card shows its anchor", await re.count() == 1 and "what about the wallpaper" in await re.inner_text())
        check("first visit: nothing unseen", await pg.locator("#traj-count").is_hidden())
        await fold.click(); await pg.wait_for_timeout(600)
        traj = pg.locator("#traj")
        check("the fold opens the trajectory", await traj.is_visible() and await chat.is_hidden() and await pg.get_attribute("#lt-traj", "aria-selected") == "true")
        check("the trajectory has the cards, the turn label and notes",
              await traj.locator(".wcard.done").count() == 1 and await traj.locator(".wcard.failed").count() == 1
              and "after app-tester finished" in await traj.locator(".trig").first.inner_text() and "Session is back" in await traj.inner_text())
        await pg.screenshot(path=J + "tr-traj.png")
        await traj.locator(".reply-btn").first.click(); await pg.wait_for_timeout(400)
        check("Reply in chat goes back to the chat with a quote", await chat.is_visible() and await pg.locator("#quote").is_visible()
              and "app-tester is done" in await pg.locator("#quote-text").inner_text())
        await pg.screenshot(path=J + "tr-quote.png")
        await send(pg, "great, ship it"); await pg.wait_for_timeout(2500)
        mine = chat.locator(".msg.user").last
        check("the sent message keeps its anchor", "app-tester is done" in await mine.locator(".re").inner_text() and await pg.locator("#quote").is_hidden())
        h = (await (await ctx.request.get(U + "/api/history?limit=20")).json())["events"]
        u = [e for e in h if e["type"] == "user"][-1]
        check("the server stored the quote", u.get("quote", "").startswith("app-tester is done") and u.get("quote_id"), u)
        a = [e for e in h if e["type"] == "assistant"][-1]
        check("the echo reply answers it in lane c, without a chip", a.get("lane") == "c" and a.get("re") == u["id"]
              and await chat.locator(".msg.assistant").last.locator(".re").count() == 0, a)
        await send(pg, "!event"); await pg.wait_for_timeout(6000)
        await pg.screenshot(path=J + "tr-event.png")
        check("an event turn folds into the chat", "runner finished" in await chat.locator(".fold").last.inner_text())
        check("its text did not land in the chat", "all 12 frames" not in await chat.inner_text())
        cnt = pg.locator("#traj-count")
        check("the badge counts it", await cnt.is_visible() and int(await cnt.inner_text()) >= 2, await cnt.inner_text() if await cnt.is_visible() else "hidden")
        await pg.click("#lt-traj"); await pg.wait_for_timeout(500)
        check("the trajectory has the turn", "after runner finished" in await traj.inner_text() and "all 12 frames" in await traj.inner_text())
        check("viewing clears the badge", await cnt.is_hidden())
        await pg.reload(); await pg.wait_for_timeout(1500)
        check("after a reload nothing is unseen", await pg.locator("#traj-count").is_hidden())
        check("the lane resets to the chat", await pg.locator("#thread").is_visible())
        check("no page errors", not pg.errs, pg.errs)
        await b.close()
    print(f"{sum(res)}/{len(res)} passed")
asyncio.run(main())
