"""Builds the two Tasks tab directions from the fixture, inside the real page chrome of tasks-before.html.
A, "Now": what needs you, what runs and how far, one next per owner; the rest behind "All tasks".
B, "Quiet board": every project one collapsed row with a one-line summary; one opened to show the inside.
Also a small swipe demo page, so the static mockups show rows at rest.
Usage: tasks_mock.py MOCKUPS_DIR"""
import html, json, re, sys

D = sys.argv[1].rstrip("/") + "/"
before = open(D + "tasks-before.html").read()
board = json.load(open(D + "tasks-fixture.json"))
T, P = board["tasks"], board["projects"]
e = html.escape

ICON = {
    "ask": '<path d="M4 5h16v11H9l-5 4zM10 9.2a2 2 0 1 1 2.6 1.9c-.4.2-.6.5-.6.9M12 13.6v.1"/>',
    "check": '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
    "chev": '<path d="M9 6l6 6-6 6"/>',
    "review": '<path d="M9 4.5H6.5A1.5 1.5 0 0 0 5 6v13.5A1.5 1.5 0 0 0 6.5 21h11a1.5 1.5 0 0 0 1.5-1.5V6a1.5 1.5 0 0 0-1.5-1.5H15"/><rect x="9" y="3" width="6" height="3" rx=".8"/><path d="M8.5 13.5l2.5 2.5 4.5-4.5"/>',
    "down": '<path d="M6 9l6 6 6-6"/>',
}
def svg(name, size=20, sw=2):
    return (f'<svg width="{size}" height="{size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="{sw}" '
            f'stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{ICON[name]}</svg>')

def step_info(t):
    ss = t.get("steps") or []
    if not ss:
        return None
    done = sum(s["state"] == "completed" for s in ss)
    cur = next((s["title"] for s in ss if s["state"] == "in_progress"), None)
    return done, len(ss), cur

def progress(done, n):
    return f'<span class="prog"><i style="width: {100 * done / n:.0f}%"></i></span><span class="pn">{done} of {n}</span>'

def ref(t):
    # The board id ends the supporting line: there when you need to say "#62", out of the way of the titles
    return f' · <span class="ref">{e(t["id"])}</span>' if t["id"].startswith("#") else ""

def row(t, kind, sub, trail="", bar="", cls=""):
    # Leading tiles only where they tell state at a glance or take a tap: a question, a job to look over, your own tick
    if kind == "ask":
        tile = f'<span class="tt ask">{svg("ask")}</span>'
    elif kind == "review":
        tile = f'<span class="tt review">{svg("review")}</span>'
    elif kind == "you":
        tile = f'<span class="tt you" role="checkbox" aria-checked="false" aria-label="Tick off {e(t["title"])}"></span>'
    else:
        tile = ""
    return (f'<div class="tr k-{kind}{cls}">{tile}<span class="tx"><span class="tl">{e(t["title"])}</span>'
            f'<span class="ts">{sub}</span>{f"<span class=tb>{bar}</span>" if bar else ""}</span>{trail}</div>')

def plural(n, one, many):
    return f"{n} {one if n == 1 else many}"

live = [t for t in T if t["state"] != "done"]
paused = {p["name"] for p in P if p["paused"]}
active = [t for t in live if t["project"] not in paused and t["state"] != "paused"]

# ---------- A: Now ----------
asks = [t for t in active if t["state"] == "asks"]
review = [t for t in active if t["state"] == "review"]
mine = [t for t in active if t["owner"] == "you" and t["state"] == "open"]
running = [t for t in active if t["state"] == "running"]
nexts, seen = [], set()
for t in sorted(active, key=lambda t: t.get("pos", 0)):
    if t["state"] == "open" and t["owner"] != "you" and t["owner"] not in seen:
        seen.add(t["owner"]); nexts.append(t)
shown_mine = 1

def run_sub(t):
    # Who and where it is, nothing else: "gentoo · Install and update the EFI entry"
    si = step_info(t)
    return e(t["owner"]) + (f' · {e(si[2])}' if si and si[2] else "")

def run_bar(t):
    si = step_info(t)
    return progress(si[0], si[1]) if si else '<span class="prog ind"><i></i></span>'

a = ['<div class="tview" role="tablist"><button class="on" role="tab" aria-selected="true">Now</button>'
     f'<button role="tab">All tasks</button></div>']
a.append(f'<h2 class="tg ask">Needs you <small>{len(asks) + len(review) + len(mine)}</small></h2><div class="tgrp needs">')
for t in asks:
    a.append(row(t, "ask", f'{e(t["owner"])} has a question'))
for t in review:
    a.append(row(t, "review", f'{e(t["owner"])} · ready to review'))
for t in mine[:shown_mine]:
    a.append(row(t, "you", "your to-do"))
if len(mine) > shown_mine:
    a.append(f'<button class="tmore">{plural(len(mine) - shown_mine, "more to-do", "more to-dos")}</button>')
a.append('</div>')
a.append(f'<h2 class="tg run">Running <small>{len(running)}</small></h2><div class="tgrp">')
for t in running:
    a.append(row(t, "run", run_sub(t), bar=run_bar(t)))
a.append('</div>')
a.append(f'<h2 class="tg">Up next <small>one per owner</small></h2><div class="tgrp">')
for t in nexts:
    a.append(row(t, "open", e(t["owner"])))
a.append('</div>')
a.append(f'<button class="tall"><span><b>All tasks</b><small>{len(active)} open in {len({t["project"] for t in active})} projects · {len(paused)} paused · done</small></span>{svg("chev", 20)}</button>')

# ---------- B: Quiet board ----------
def summary(ts):
    # Words a person would say; only what needs the user gets the warm tone, only what runs gets primary
    k = lambda s: sum(t["state"] == s for t in ts)
    mine_n = sum(t["owner"] == "you" and t["state"] == "open" for t in ts)
    rest = sum(t["state"] == "open" and t["owner"] != "you" for t in ts)
    parts = []
    if k("asks"): parts.append(f'<span class="sw">{plural(k("asks"), "question", "questions")}</span>')
    if k("review"): parts.append(f'<span class="sw">{k("review")} to review</span>')
    if mine_n: parts.append(f'<span class="sw">{plural(mine_n, "to-do", "to-dos")}</span>')
    if k("running"): parts.append(f'<span class="sn">{k("running")} running</span>')
    if rest: parts.append(f'{rest} queued')
    return " · ".join(parts)

def rank(ts):
    if any(t["state"] == "asks" for t in ts): return 0
    if any(t["state"] == "review" for t in ts): return 1
    if any(t["state"] == "running" for t in ts): return 2
    return 3

projs = []
for i, p in enumerate(P):
    if p["paused"]:
        continue
    ts = [t for t in live if t["project"] == p["name"]]
    if ts:
        projs.append((rank(ts), i, p["name"], ts))
projs.sort()
OPEN = "gentoo"
# One segmented list: an open project's items are rows of the same list, same edges, so the group just grows
# B is the All tasks view of A's switch
b = ['<div class="tview" role="tablist"><button role="tab">Now</button>'
     '<button class="on" role="tab" aria-selected="true">All tasks</button></div><div class="tgrp board">']
for _, _, name, ts in projs:
    is_open = name == OPEN
    b.append(f'<button class="prow{" open" if is_open else ""}" aria-expanded="{str(is_open).lower()}">'
             f'<span class="tx"><span class="tl">{e(name)}</span><span class="ts">{summary(ts)}</span></span>'
             f'<span class="tc">{svg("down" if is_open else "chev", 20)}</span></button>')
    if is_open:
        for t in sorted(ts, key=lambda t: ({"asks": 0, "review": 1, "running": 2}.get(t["state"], 3), t.get("pos", 0))):
            own = "" if t["owner"] == t["project"] else f'{e(t["owner"])} · '
            if t["state"] == "asks":
                b.append(row(t, "ask", f'{own}has a question{ref(t)}', cls=" in"))
            elif t["state"] == "review":
                b.append(row(t, "review", f'{own}ready to review{ref(t)}', cls=" in"))
            elif t["state"] == "running":
                si = step_info(t)
                b.append(row(t, "run", own + (e(si[2]) if si and si[2] else "running") + ref(t), bar=run_bar(t), cls=" in"))
            elif t["owner"] == "you":
                b.append(row(t, "you", "your to-do", cls=" in"))
            else:
                nx = '<span class="nx">next</span>' if t in nexts else "queued"
                b.append(row(t, "open", f'{own}{nx}{ref(t)}', cls=" in"))
        done_n = sum(t["state"] == "done" and t["project"] == name for t in T)
        if done_n:
            b.append(f'<button class="tfold in">{svg("check", 18)}{done_n} done</button>')
b.append('</div>')
held = [p["name"] for p in P if p["paused"]]
b.append(f'<button class="tall quiet"><span><b>{plural(len(held), "paused project", "paused projects")}</b><small>{e(", ".join(held))}</small></span>{svg("chev", 20)}</button>')

# ---------- Swipe demo: the one moving part, shown on its own rather than frozen inside the list ----------
m0, m1 = mine[0], mine[1]
sw = ['<p class="demo-cap">Swipe a to-do right to tick it off. At rest (top) and mid-swipe (bottom):</p>'
      f'<div class="tgrp needs">{row(m0, "you", "your to-do")}'
      f'<div class="swipe swiping"><span class="under">{svg("check", 20, 2.4)}Done</span>'
      f'{row(m1, "you", "your to-do")}</div></div>']

SPARKLE = '<path d="M12 3 13.9 8.6 19.5 10.5 13.9 12.4 12 18 10.1 12.4 4.5 10.5 10.1 8.6ZM18.5 15.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z"></path>'
# The Agents nav icon: the grid of shikigami tiles, the user's pick from agents-icons.html, instead of the generic AI sparkle
GRID = '<path d="M6 4h3.5A1.5 1.5 0 0 1 11 5.5V9a1.5 1.5 0 0 1-1.5 1.5H6A1.5 1.5 0 0 1 4.5 9V5.5A1.5 1.5 0 0 1 6 4zM14.5 4H18a1.5 1.5 0 0 1 1.5 1.5V9A1.5 1.5 0 0 1 18 10.5h-3.5A1.5 1.5 0 0 1 13 9V5.5A1.5 1.5 0 0 1 14.5 4zM6 13.5h3.5A1.5 1.5 0 0 1 11 15v3.5A1.5 1.5 0 0 1 9.5 20H6a1.5 1.5 0 0 1-1.5-1.5V15A1.5 1.5 0 0 1 6 13.5zM14.5 13.5H18a1.5 1.5 0 0 1 1.5 1.5v3.5A1.5 1.5 0 0 1 18 20h-3.5a1.5 1.5 0 0 1-1.5-1.5V15a1.5 1.5 0 0 1 1.5-1.5z"/>'

def page(name, body, css):
    s = before.replace('<link rel="stylesheet" href="../../../web/style.css">',
                       '<link rel="stylesheet" href="../../../web/style.css">\n<link rel="stylesheet" href="m3x-tokens.css">\n'
                       f'<link rel="stylesheet" href="m3x-loading.css">\n<link rel="stylesheet" href="{css}">', 1)
    s, n = re.subn(r'(<div class="tasks" id="tasks"[^>]*>).*?(</div>\n\s*<div class="hello")', lambda m: m.group(1) + body + m.group(2), s, count=1, flags=re.S)
    assert n == 1, "tasks block not found"
    s = s.replace(SPARKLE, GRID)
    open(D + name, "w").write(s)
    print("wrote", name)

page("tasks-a-now.html", "".join(a), "tasks-after.css")
page("tasks-b-quiet.html", "".join(b), "tasks-after.css")
page("tasks-swipe.html", "".join(sw), "tasks-after.css")
