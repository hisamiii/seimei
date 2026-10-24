"""Review tile glyph options: the real Needs you group of tasks-a-now.html, once per candidate in the review tile.
Same 24px grid and 2px stroke as the question glyph next to it. Usage: review_icons.py MOCKUPS_DIR"""
import re, sys

D = sys.argv[1].rstrip("/") + "/"
now = open(D + "tasks-a-now.html").read()
root = re.search(r"<html[^>]*>", now).group(0)
group = re.search(r'<div class="tgrp needs">.*?</div>(?=<h2 class="tg run">)', now, re.S).group(0)
group = re.sub(r'<button class="tmore">.*?</button>', "", group)
review_svg = re.search(r'<span class="tt review"><svg[^>]*>(.*?)</svg>', group).group(1)

FAV = "Clipboard check"
OPTS = [
    ("Clipboard check", "assignment_turned_in: the board's clip and a check on the sheet",
     '<path d="M9 4.5H6.5A1.5 1.5 0 0 0 5 6v13.5A1.5 1.5 0 0 0 6.5 21h11a1.5 1.5 0 0 0 1.5-1.5V6a1.5 1.5 0 0 0-1.5-1.5H15"/>'
     '<rect x="9" y="3" width="6" height="3" rx=".8"/><path d="M8.5 13.5l2.5 2.5 4.5-4.5"/>'),
    ("Checklist", "playlist_add_check: two lines of work, a short one and its check",
     '<path d="M4 5.5h16M4 10.5h16M4 15.5h6M13 16.5l2.5 2.5 5-5.5"/>'),
    ("Hanko", "approval: the seal stamp and its mark, to sign the work off",
     '<path d="M12 3a3 3 0 0 1 3 3c0 1.2-.7 2-1.2 2.8-.3.5-.3 1-.3 1.7V12h3.5a2 2 0 0 1 2 2v2H5v-2a2 2 0 0 1 2-2h3.5v-1.5c0-.7 0-1.2-.3-1.7C9.7 8 9 7.2 9 6a3 3 0 0 1 3-3zM5 20h14"/>'),
    ("Done all", "done_all: two checks, the job says it is finished",
     '<path d="M2 12.5 6.5 17 16.5 7M10.5 15.5 12 17 22 7"/>'),
    ("Shipped box", "deployed_code: the work is delivered and waits for you to open it; no check at all",
     '<path d="M12 2.8l8 4.6v9.2l-8 4.6-8-4.6V7.4zM4 7.4l8 4.6 8-4.6M12 12v9.2"/>'),
]

secs = []
for name, note, glyph in OPTS:
    fav = ' <b class="fav">my pick</b>' if name == FAV else ""
    secs.append(f'<section class="opt"><h2>{name}{fav}<small>{note}</small></h2>'
                f'<div class="tasks">{group.replace(review_svg, glyph, 1)}</div></section>')

page = f"""<!doctype html>
{root}<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Review tile icon options</title>
<link rel="stylesheet" href="../../../web/style.css"><link rel="stylesheet" href="m3x-tokens.css"><link rel="stylesheet" href="tasks-after.css">
<style>
body {{ display: block; background: var(--surface-low); padding: 16px 0 8px; }}
h1 {{ margin: 0 16px 4px; font: 500 20px/28px var(--sans); color: var(--text); }}
p.lead {{ margin: 0 16px 12px; font-size: 14px; line-height: 20px; color: var(--text-3); }}
.opt {{ margin-top: 16px; }}
.opt h2 {{ margin: 0 16px 8px; font: 500 14px/20px var(--sans); color: var(--text); display: flex; align-items: baseline; gap: 8px; flex-wrap: wrap; }}
.opt h2 small {{ flex-basis: 100%; font-size: 12px; line-height: 16px; font-weight: 400; color: var(--text-3); }}
.opt h2 .fav {{ font-size: 12px; font-weight: 500; color: var(--primary); }}
.opt .tasks {{ display: block; padding: 0 16px; }}
</style></head>
<body><h1>Review tile icon</h1><p class="lead">The real Needs you group, the review tile in the middle. Same 24px grid and stroke as the question glyph. No eye, no inbox, no check in a circle.</p>
{"".join(secs)}
</body></html>
"""
open(D + "review-icons.html", "w").write(page)
print("wrote review-icons.html,", len(OPTS), "options")
