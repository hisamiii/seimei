# Shared helpers for QA browser scripts.
import asyncio, subprocess, json, sys
from playwright.async_api import async_playwright
BASE = "http://localhost:8431"
import os, shutil
QA = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "qa")
SHOTS = os.environ.get("SEIMEI_SHOTS") or os.path.join(QA, ".shots")
os.makedirs(SHOTS, exist_ok=True)
async def browser(p, w=412, h=892, dpr=1, **kw):
    chrome = __import__("os").environ.get("SEIMEI_CHROME", "/usr/bin/google-chrome-stable")
    b = await p.chromium.launch(executable_path=chrome if __import__("os").path.exists(chrome) else None, args=["--disable-gpu"])
    ctx = await b.new_context(viewport={"width": w, "height": h}, device_scale_factor=dpr, has_touch=kw.pop("touch", True), is_mobile=kw.pop("mobile", True), **kw)
    pg = await ctx.new_page()
    pg.errs = []
    pg.on("pageerror", lambda e: pg.errs.append("pageerror: " + str(e)))
    pg.on("console", lambda m: m.type in ("error", "warning") and pg.errs.append(f"console {m.type}: {m.text}"))
    return b, ctx, pg
async def shot(pg, name, full=False, **kw):
    path = f"{SHOTS}/{name}.png"
    await pg.screenshot(path=path, full_page=full, **kw)
    if shutil.which("magick"):
        subprocess.run(["magick", path, "-resize", "412x>", "-quality", "80", f"{SHOTS}/{name}.s.jpg"])
    return path
