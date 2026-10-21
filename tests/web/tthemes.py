"""#81 on the web: the theme picker. A tile sets the theme at once, light or dark follows the system unless pinned,
Catppuccin's accent changes the primary, Material You builds its scheme from a seed (picked here until a phone sends
its wallpaper's, which then arrives live), and the next visit paints the kept theme before the scripts run."""
import asyncio, json, sys; sys.path.insert(0, __import__('os').path.dirname(__file__))
from h import *
U = "http://localhost:8431"; O = {"Origin": U, "Content-Type": "application/json"}
J = SHOTS + "/"
res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300], flush=True)
VAR = "k => getComputedStyle(document.documentElement).getPropertyValue('--' + k).trim()"


async def main():
    async with async_playwright() as p:
        b, ctx, pg = await browser(p, 412, 892, dpr=2, color_scheme="dark")
        await ctx.request.post(U + "/api/auth/password", data='{"password": "qa-password-123"}', headers=O)
        check("no seed before any phone sent one", await (await ctx.request.get(U + "/api/theme")).json() == {})
        await pg.goto(U + "/"); await pg.wait_for_timeout(1500)
        check("default: Catppuccin Mocha", await pg.evaluate(VAR, "surface") == "#1e1e2e", await pg.evaluate(VAR, "surface"))
        await pg.locator("#menu-btn").click(); await pg.locator("#theme").click(); await pg.wait_for_timeout(800)
        tiles = pg.locator(".ttile")
        check("five tiles, Catppuccin picked", await tiles.count() == 5 and "Catppuccin" in await pg.locator(".ttile.sel").inner_text())
        surf = await pg.locator(".ttile .tprev").evaluate_all("ps => ps.map(p => getComputedStyle(p).backgroundColor)")
        check("each tile in its own colors", len(set(surf)) == 5, surf)
        check("accents show for Catppuccin", await pg.locator(".tdot").count() == 14)
        await shot(pg, "themes-dark")
        await tiles.nth(2).click(); await pg.wait_for_timeout(400)
        check("Gruvbox applies at once", await pg.evaluate(VAR, "surface") == "#282828")
        check("no accents for Gruvbox", await pg.locator(".tdot").count() == 0)
        await pg.locator(".tseg button", has_text="Light").click(); await pg.wait_for_timeout(400)
        check("pinned light: Gruvbox Light", await pg.evaluate(VAR, "surface") == "#fbf1c7")
        await pg.emulate_media(color_scheme="dark"); await pg.wait_for_timeout(300)
        check("a pinned side ignores the system", await pg.evaluate(VAR, "surface") == "#fbf1c7")
        await pg.locator(".tseg button", has_text="System").click(); await pg.emulate_media(color_scheme="light"); await pg.wait_for_timeout(400)
        check("System follows the system", await pg.evaluate(VAR, "surface") == "#fbf1c7")
        await pg.emulate_media(color_scheme="dark"); await pg.wait_for_timeout(400)
        check("... both ways", await pg.evaluate(VAR, "surface") == "#282828")
        await tiles.nth(0).click(); await pg.wait_for_timeout(300)
        await pg.locator(".tdot[title=blue]").click(); await pg.wait_for_timeout(300)
        check("Catppuccin accent: blue", await pg.evaluate(VAR, "primary") == "#89b4fa")
        check("the dot shows picked", await pg.locator(".tdot.on").get_attribute("title") == "blue")
        # Material You, no phone yet: a seed picker on its tile.
        seed = pg.locator(".tseed")
        check("Material You offers a seed color", await seed.count() == 1)
        await seed.evaluate("e => { e.value = '#006a6a'; e.dispatchEvent(new Event('change')); }"); await pg.wait_for_timeout(600)
        my = await pg.evaluate(VAR, "primary")
        check("the picked seed builds the scheme", await pg.evaluate("() => JSON.parse(localStorage.getItem('seimei.theme')).theme") == "material-you"
              and my not in ("#89b4fa", "#cba6f7", "") and await pg.evaluate(VAR, "surface") != "#1e1e2e", my)
        await shot(pg, "themes-seed")
        # A phone sends its wallpaper's seed: the page follows it live, and the tile says where it came from.
        r = await ctx.request.post(U + "/api/theme", data=json.dumps({"seed": "#8f4c38"}), headers=O)
        check("the seed is kept", r.ok and (await (await ctx.request.get(U + "/api/theme")).json())["seed"] == "#8f4c38")
        bad = await ctx.request.post(U + "/api/theme", data=json.dumps({"seed": "red"}), headers=O)
        check("a bad seed is refused", bad.status == 400)
        await pg.wait_for_timeout(1200)
        check("the phone's seed arrives live", await pg.evaluate(VAR, "primary") not in (my, ""), await pg.evaluate(VAR, "primary"))
        check("the tile names it, no picker", await seed.count() == 0 and "wallpaper" in await pg.locator(".ttile").nth(4).inner_text())
        await shot(pg, "themes-phone")
        kept = await pg.evaluate(VAR, "surface")
        await pg.goto(U + "/"); await pg.wait_for_timeout(100)
        check("the next visit paints the kept theme", await pg.evaluate(VAR, "surface") == kept, kept)
        check("theme-color follows", await pg.locator('meta[name="theme-color"]').get_attribute("content") == kept)
        check("no page errors", not pg.errs, pg.errs)
        await b.close()
    print(f"{sum(res)}/{len(res)} passed")
asyncio.run(main())
