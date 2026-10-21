"""#68 on the web: reply to seimei's messages and to cards (an ask in the chat, a report in the trajectory, an approval)
by the hover button or a long press; the composer quotes it, the sent message shows the anchor, seimei gets
[re: "..."] first."""
import asyncio, json, sys; sys.path.insert(0, __import__('os').path.dirname(__file__))
from h import *
U = "http://localhost:8431"; O = {"Origin": U, "Content-Type": "application/json"}
J = SHOTS + "/"
LOG = QA + "/delivered.log"
res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300], flush=True)


def delivered():
    try:
        return [json.loads(x) for x in open(LOG)]
    except OSError:
        return []


async def send(pg, text):
    await pg.locator("#input").fill(text)
    await pg.locator("#input").press("Enter")
    await pg.wait_for_timeout(1500)


async def main():
    open(LOG, "w").close()
    async with async_playwright() as p:
        b, ctx, pg = await browser(p, 1280, 900, touch=False, mobile=False)
        await ctx.request.post(U + "/api/auth/password", data='{"password": "qa-password-123"}', headers=O)
        await pg.goto(U + "/"); await pg.wait_for_timeout(2500)

        # 1. Hover a message of seimei's in the chat: the reply button shows, a click quotes it.
        msg = pg.locator("#thread .msg.assistant.replyable").last
        btn = msg.locator(":scope > .rbtn")
        check("assistant messages are replyable", await msg.count() == 1)
        op0 = await btn.evaluate("b => getComputedStyle(b).opacity")
        await msg.hover(); await pg.wait_for_timeout(300)
        op1 = await btn.evaluate("b => getComputedStyle(b).opacity")
        check("reply button shows on hover only", op0 == "0" and op1 == "1", (op0, op1))
        await pg.screenshot(path=J + "reply-hover.png")
        await btn.click(); await pg.wait_for_timeout(300)
        quote = await pg.locator("#quote-text").inner_text()
        said = (await msg.evaluate("n => [...n.children].filter(c => !c.matches('.re, .rbtn, .time')).map(c => c.textContent).join(' ')")).split()[:3]
        check("composer quotes the message", await pg.locator("#quote").is_visible() and " ".join(said) in quote, (quote, said))
        await send(pg, "yes, that one")
        last = pg.locator("#thread .msg.user").last
        check("sent message shows the anchor", await last.locator(".re").count() == 1, await last.inner_html())
        d = delivered()
        check("seimei gets [re: ...] first", d and d[-1].startswith('[re: "') and d[-1].split("\n", 1)[1].startswith("yes, that one"), d[-1:] if d else d)
        check("the quote is gone after sending", not await pg.locator("#quote").is_visible())
        await pg.wait_for_timeout(4000)  # the fake turn answers

        # 2. The ask card in the chat.
        card = pg.locator("#thread .wcard.asking.replyable, #thread .wcard.asked.replyable").first
        check("ask card is replyable", await card.count() == 1)
        await card.hover(); await card.locator(":scope > .rbtn").click(); await pg.wait_for_timeout(300)
        quote = await pg.locator("#quote-text").inner_text()
        check("composer quotes the card", "asks:" in quote, quote)
        await send(pg, "use the second")
        d = delivered()
        check("card reply names the shikigami and id", d and d[-1].startswith('[re: "shikigami wallpaper-grade-final-pass-very-long-name-x (')
              and ') asks: ' in d[-1].split("\n")[0], d[-1:] if d else d)
        await pg.wait_for_timeout(4000)

        # 3. A report in the trajectory: reply switches to the chat, the anchor goes back to the trajectory.
        await pg.locator("#lt-traj").click(); await pg.wait_for_timeout(1200)
        rep = pg.locator("#traj .wcard.done.replyable").first
        check("report card is replyable", await rep.count() == 1)
        await rep.hover(); await rep.locator(":scope > .rbtn").click(); await pg.wait_for_timeout(500)
        quote = await pg.locator("#quote-text").inner_text()
        check("report quote, back in the chat", "finished:" in quote and await pg.locator("#thread").is_visible(), quote)
        await send(pg, "great, ship it")
        d = delivered()
        check("report reply line", d and ") finished: " in d[-1].split("\n")[0], d[-1:] if d else d)
        anchor = pg.locator("#thread .msg.user").last.locator(".re")
        check("anchor points to the trajectory", await anchor.get_attribute("data-lane") == "t")
        await anchor.click(); await pg.wait_for_timeout(1000)
        check("tapping it opens the trajectory", await pg.locator("#traj").is_visible())
        await pg.locator("#lt-chat").click(); await pg.wait_for_timeout(4000)

        # 4. An approval card (Hand to seimei asks through one).
        await pg.goto(U + "/#agents"); await pg.wait_for_timeout(1500)
        await pg.locator("button.agent", has_text="prefill-tuning").first.click(); await pg.wait_for_timeout(2000)
        await pg.locator(".sbar .handoff").click(); await pg.wait_for_timeout(1500)
        await pg.goto(U + "/"); await pg.wait_for_timeout(2500)
        ap = pg.locator("#thread .wcard.approval.replyable").last
        check("approval card is replyable", await ap.count() == 1)
        await ap.hover(); await ap.locator(":scope > .rbtn").click(); await pg.wait_for_timeout(300)
        quote = await pg.locator("#quote-text").inner_text()
        check("approval quote", "wants to" in quote, quote)
        await send(pg, "why?")
        d = delivered()
        check("approval reply line", d and d[-1].startswith('[re: "approval card: '), d[-1:] if d else d)
        evs = (await (await ctx.request.get(U + "/api/history?limit=80")).json())["events"]
        card = next((e for e in reversed(evs) if e.get("type") == "approval"), None)
        if card:
            await ctx.request.post(U + f"/api/approvals/{card['aid']}", data='{"decision": "deny"}', headers=O)
        await pg.wait_for_timeout(4000)
        check("no console errors", not pg.errs, pg.errs)
        await b.close()

        # 5. Touch: a long press replies; a short tap does not.
        b, ctx, pg = await browser(p, 412, 892, dpr=2)
        await ctx.request.post(U + "/api/auth/password", data='{"password": "qa-password-123"}', headers=O)
        await pg.goto(U + "/"); await pg.wait_for_timeout(2500)
        check("no hover button on touch", not await pg.locator("#thread .rbtn").first.is_visible())
        msg = pg.locator("#thread .msg.assistant.replyable").last
        await msg.scroll_into_view_if_needed()
        box = await msg.bounding_box()
        x, y = box["x"] + 30, box["y"] + 12

        async def press(ms):
            await msg.dispatch_event("pointerdown", {"pointerType": "touch", "clientX": x, "clientY": y, "bubbles": True})
            await pg.wait_for_timeout(ms)
            await msg.dispatch_event("pointerup", {"pointerType": "touch", "clientX": x, "clientY": y, "bubbles": True})
            await msg.dispatch_event("click", {"bubbles": True})
            await pg.wait_for_timeout(500)
        await press(120)
        check("short tap does not reply", not await pg.locator("#quote").is_visible())
        await press(700)
        check("long press replies", await pg.locator("#quote").is_visible())
        await pg.screenshot(path=J + "reply-phone.png")
        await pg.locator("#quote-x").click(); await pg.wait_for_timeout(700)
        before = await msg.locator(":scope > .time").count()
        await press(120)
        check("the tap after a long press is not swallowed for long", not await pg.locator("#quote").is_visible()
              and await msg.locator(":scope > .time").count() != before)
        check("no console errors (phone)", not pg.errs, pg.errs)
        await b.close()
asyncio.run(main())
print(f"{sum(res)}/{len(res)} passed")
