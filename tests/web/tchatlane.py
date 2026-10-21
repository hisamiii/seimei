"""Trajectory follow-up on the web: the part of an event turn that comes to the chat opens with what started it."""
import asyncio, sys; sys.path.insert(0, __import__('os').path.dirname(__file__))
from h import *
U = "http://localhost:8431"
res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300], flush=True)
async def main():
    async with async_playwright() as p:
        b, ctx, pg = await browser(p, 412, 892, dpr=2)
        r = await ctx.request.post(U + "/api/auth/password", data='{"password": "qa-password-123"}',
                                   headers={"Origin": U, "Content-Type": "application/json"})
        assert r.ok, await r.text()
        await pg.goto(U + "/"); await pg.wait_for_timeout(1500)
        await pg.evaluate("""async () => { const m = await import(document.querySelector('script[src*="app.js"]').src.replace('app.js', 'chat.js')); const t = Date.now() / 1000;
          m.addEvent({type: 'tool', tool_id: 'z1', name: 'Read', summary: 'notes', lane: 't', turn: 90001, why: 'shikigami seimei-icon finished', id: 90001, t});
          m.addEvent({type: 'image', file: 'nope.png', caption: 'M3 sheet', lane: 'c', from: 90001, why: 'shikigami seimei-icon finished', id: 90002, t});
          m.addEvent({type: 'assistant', text: 'Here is the M3 sheet.', lane: 'c', from: 90001, why: 'shikigami seimei-icon finished', id: 90003, t});
          m.addEvent({type: 'user', text: 'nice', id: 90004, t});
          m.addEvent({type: 'assistant', text: 'Thanks!', lane: 'c', re: 90004, id: 90005, t}); }""")
        await pg.wait_for_timeout(500)
        chat = pg.locator("#thread")
        trig = chat.locator(".trig")
        check("one started-by line in the chat", await trig.count() == 1 and await trig.first.inner_text() == "after shikigami seimei-icon finished", [await t.inner_text() for t in await trig.all()])
        order = await chat.evaluate("""c => [...c.children].slice(-6).map(n => n.className.split(' ')[0] + ':' + n.innerText.slice(0, 30).replace(/\\n/g, ' '))""")
        check("it sits right before the picture and the reply", any("trig" in x for x in order) and "Here is the M3 sheet." in await chat.inner_text(), order)
        check("the user's own exchange after it gets no line", order[-1].startswith("msg") and "Thanks" in order[-1], order)
        await pg.screenshot(path=SHOTS + "/chatlane.png")
        await pg.evaluate("""async () => { const m = await import(document.querySelector('script[src*="app.js"]').src.replace('app.js', 'chat.js')); const t = Date.now() / 1000;
          m.addEvent({type: 'user', text: 'fix the waybar clock please', id: 90010, t});
          m.addEvent({type: 'assistant', text: 'Notes: read config.jsonc', lane: 't', turn: 90011, why: 'your message', re: 90010, id: 90011, t});
          m.addEvent({type: 'assistant', text: 'Fixed.', lane: 'c', re: 90010, id: 90012, t}); }""")
        await pg.wait_for_timeout(400)
        folds = [await f.inner_text() for f in await chat.locator(".fold").all()]
        check("the chat folds it as a reply to you", any(f.startswith("a reply to you") and "Trajectory" in f for f in folds), folds)
        check("the notes are not in the chat", "Notes: read config.jsonc" not in await chat.inner_text())
        await chat.locator(".fold").last.click(); await pg.wait_for_timeout(500)
        traj = pg.locator("#traj")
        check("the trajectory has them under their message", "after your message" in await traj.inner_text() and await traj.locator(".re").last.inner_text() == "fix the waybar clock please" and "Notes: read config.jsonc" in await traj.inner_text(), (await traj.inner_text())[-300:])
        await pg.screenshot(path=SHOTS + "/trajmark.png")
        await pg.goto(U + "/"); await pg.wait_for_timeout(1500)
        await pg.evaluate("""async () => { const m = await import(document.querySelector('script[src*="app.js"]').src.replace('app.js', 'chat.js')); const t = Date.now() / 1000;
          m.addEvent({type: 'user', text: 'what about the clock?', id: 90020, t});
          m.addEvent({type: 'assistant', text: 'Pinned reply.', lane: 'c', re: 90020, re_pin: true, re_text: 'what about the clock?', id: 90021, t});
          m.addEvent({type: 'assistant', text: 'Old one.', lane: 'c', re: 12, re_pin: true, re_text: 'a message from long ago', id: 90022, t}); }""")
        await pg.wait_for_timeout(400)
        chips = await chat.locator(".msg.assistant .re").all_inner_texts()
        check("re_pin shows the chip even right after the message", "what about the clock?" in chips, chips)
        check("an unloaded message is quoted from re_text", "a message from long ago" in chips, chips)
        await b.close()
    print(f"{sum(res)}/{len(res)} passed")
asyncio.run(main())
