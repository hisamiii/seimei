"""#59 on the web: Give to me on a shikigami page releases it (its page closes, it leaves the list, its state is
released); Hand to seimei on a session page asks through the approval card in the chat."""
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
        ws = (await (await ctx.request.get(U + "/api/workers")).json())["workers"]
        wid = next(w["id"] for w in ws if w["name"] == "app-tester")
        await pg.goto(U + "/#agents"); await pg.wait_for_timeout(1500)
        await pg.locator("button.agent", has_text="app-tester").first.click(); await pg.wait_for_timeout(2500)
        hand = pg.locator(".sbar .handoff")
        check("Give to me on a shikigami page", await hand.is_visible() and (await hand.inner_text()).strip() == "Give to me", await hand.count())
        await pg.screenshot(path=J + "handoff-worker.png")
        await hand.click(); await pg.wait_for_timeout(1500)
        ws = (await (await ctx.request.get(U + "/api/workers")).json())["workers"]
        check("released", next(w for w in ws if w["id"] == wid)["state"] == "released")
        await pg.goto(U + "/#agents"); await pg.wait_for_timeout(2000)
        names = await pg.locator("button.agent").all_inner_texts()
        check("out of the Agents list", names and not any("app-tester" in n for n in names), names)
        log = open(QA + "/released.log").read()
        check("opened as the user's session", "claude-queue app-tester --resume" in log and "--name app-tester" in log, log)
        check("the page closed with a note", "your session now" in (await pg.locator("body").inner_text()))
        r = await ctx.request.post(U + f"/api/workers/{wid}/send", data='{"text": "hi"}', headers=O)
        check("messages to it are refused", r.status >= 400, r.status)
        r = await ctx.request.post(U + f"/api/workers/{wid}/release", data="{}", headers=O)
        check("releasing again says so", r.status == 409, r.status)

        s = {"name": "prefill-tuning"}  # one of the QA user's interactive sessions
        await pg.goto(U + "/#agents"); await pg.wait_for_timeout(1500)
        await pg.locator("button.agent", has_text=s["name"]).first.click(); await pg.wait_for_timeout(2500)
        hand = pg.locator(".sbar .handoff")
        check("Hand to seimei on a session page", await hand.is_visible() and (await hand.inner_text()).strip() == "Hand to seimei")
        await pg.screenshot(path=J + "handoff-session.png")
        await hand.click(); await pg.wait_for_timeout(1500)
        check("asks to approve the card", "Approve the card in Chat" in (await pg.locator("body").inner_text()))
        evs = (await (await ctx.request.get(U + "/api/history?limit=50")).json())["events"]
        card = next((e for e in reversed(evs) if e.get("type") == "approval" and f"adopt your session \"{s['name']}\"" in e.get("target", "")), None)
        check("the card is in the chat", card, [e.get("target") for e in evs if e.get("type") == "approval"])
        if card:
            r = await ctx.request.post(U + f"/api/approvals/{card['aid']}", data='{"decision": "deny"}', headers=O)
            check("denied: nothing changes", r.ok)
        check("no console errors", not pg.errs, pg.errs)
        await b.close()
asyncio.run(main())
print(f"{sum(res)}/{len(res)} passed")
