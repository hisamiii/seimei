"""The showcase contact sheet as a page: every img/showcase/NN-name.png at 1x, a row per part of the app (chat, then
tasks and agents, then usage, account and offline, then the BEHAVIOR.md screens), the number and name above each, in the app's font. Shoot it at
1x for img/showcase/all.png. Usage: showcase_sheet.py IMG_SHOWCASE_DIR OUT_HTML"""
import glob, html, os, sys

D, OUT = sys.argv[1], sys.argv[2]
ROWS = [("01", "05"), ("06", "11"), ("12", "17"), ("18", "22"), ("23", "99")]  # 23 on: the final set's extra states
here = os.path.dirname(os.path.abspath(OUT))
font = os.path.relpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "../../../web/fonts"), here)
rel = os.path.relpath(os.path.abspath(D), here)
shots = sorted(os.path.basename(p) for p in glob.glob(os.path.join(D, "[0-9][0-9]-*.png")))
rows = []
for a, b in ROWS:
    cells = "".join(f'<figure><figcaption><span>{n[:2]}</span>{html.escape(n[3:-4].replace("-", " ").capitalize())}'
                    f'</figcaption><img src="{rel}/{n}" width="412"></figure>' for n in shots if a <= n[:2] <= b)
    if cells:
        rows.append(f'<div class="row">{cells}</div>')
open(OUT, "w").write(f"""<!doctype html><meta charset="utf-8"><title>Showcase</title>
<style>
@font-face {{ font-family: "Google Sans Flex"; src: url("{font}/google-sans-flex-latin.woff2"); font-weight: 1 1000; }}
body {{ margin: 0; padding: 32px; background: #11111b; color: #cdd6f4; font: 500 22px/28px "Google Sans Flex", sans-serif; width: max-content; }}
.row {{ display: flex; gap: 24px; align-items: flex-start; }}
.row + .row {{ margin-top: 48px; }}
figure {{ margin: 0; }}
figcaption {{ height: 40px; }}
figcaption span {{ color: #a6adc8; margin-right: 12px; font-variant-numeric: tabular-nums; }}
img {{ display: block; }}
</style>
{"".join(rows)}
""")
print("wrote", OUT, len(shots), "screens")
