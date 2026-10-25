"""Lays out the before and after images of the expressive pass: cuts the frames out of the 2x shots (rects.json says
where) and writes one HTML page per moment into SHOTDIR/pairs, which shoot_pairs.py shoots into img/expressive.
Usage: pairs.py SHOTDIR"""
import html, json, os, sys
from PIL import Image

SHOTS = sys.argv[1]
R = json.load(open(os.path.join(SHOTS, "rects.json")))
DIR = os.path.join(SHOTS, "pairs")
CARD, RUN = ".wcard.approval.perm", ".tg.run"
CSS = """body { margin: 0; padding: 28px 32px 32px; background: #11111b; color: #cdd6f4; font: 400 19px/26px 'Google Sans Flex', sans-serif; width: max-content; }
h1 { margin: 0 0 6px; font-weight: 600; font-size: 30px; line-height: 38px; } p { margin: 0 0 24px; color: #a6adc8; max-width: 1100px; }
.row { display: flex; gap: 28px; align-items: flex-start; } .row.stack { flex-direction: column; }
figure { margin: 0; } figcaption { font-weight: 500; font-size: 17px; color: #a6adc8; margin-bottom: 10px; } img { display: block; }"""


def shot(name):
    return Image.open(os.path.join(SHOTS, name + ".png")).convert("RGB")


def full(name):
    return shot(name).resize((412, 915), Image.LANCZOS)


def crop(name, box, pad=8):
    # box in CSS px (x0, y0, x1, y1), cut from the 2x shot
    x0, y0, x1, y1 = box
    return shot(name).crop((int((x0 - pad) * 2), int((y0 - pad) * 2), int((x1 + pad) * 2), int((y1 + pad) * 2)))


def rect(name, sel, i=0):
    x, y, w, h = R[name][sel][i]
    return (x, y, x + w, y + h)


def span(name, first, last):
    a, b = rect(name, *first), rect(name, *last)
    return (min(a[0], b[0]), a[1], max(a[2], b[2]), b[3])


def page(name, title, caption, frames, stack):
    figs = []
    for i, (label, im) in enumerate(frames):
        f = f"{name}-{i}.png"
        im.save(os.path.join(DIR, f))
        figs.append(f'<figure><figcaption>{html.escape(label)}</figcaption><img src="{f}" width="{im.width}" height="{im.height}"></figure>')
    doc = f'<!doctype html><meta charset="utf-8"><title>{html.escape(title)}</title><style>{CSS}</style><h1>{html.escape(title)}</h1><p>{html.escape(caption)}</p><div class="row{" stack" if stack else ""}">{"".join(figs)}</div>\n'
    open(os.path.join(DIR, name + ".html"), "w").write(doc)


PAIRS = [
    ("01-needs-you", "1 · Needs you, the hero", "Its own shape set (28 outer, 8 inner corners), the asker's library shape, title large and title medium emphasized",
     lambda: [("Before", full("b-tasks")), ("After", full("a-tasks"))], False),
    ("02-approval", "2 · The decision on a card", "Allow a filled medium button (56) with its icon, round at rest, 12 corners and wider under the finger; Deny tonal",
     lambda: [("Before", crop("b-chat", rect("b-chat", CARD))), ("After, at rest", crop("a-chat", rect("a-chat", CARD))),
              ("After, Allow pressed", crop("a-chat-pressed", rect("a-chat-pressed", CARD)))], False),
    ("03-nav-bar", "3 · Nav bar", "A primary pill behind the active icon (7.3:1 on the bar, was 1.3:1), the active label emphasized, the first tab named seimei",
     lambda: [("Before", crop("b-tasks", rect("b-tasks", ".navbar"), 0)), ("After, seimei", crop("a-tasks", rect("a-tasks", ".navbar"), 0)),
              ("After, Agents", crop("a-agents", rect("a-tasks", ".navbar"), 0)), ("After, Usage", crop("a-usage", rect("a-tasks", ".navbar"), 0))], True),
    ("04-wavy-progress", "4 · Work in progress waves", "Running tasks get the wavy indicator (amplitude 3, wavelength 40, travelling); usage meters are amounts and stay flat",
     lambda: [("Before", crop("b-tasks", span("b-tasks", (RUN,), (".tr.k-run", 1)))), ("After", crop("a-tasks", span("a-tasks", (RUN,), (".tr.k-run", 1)))),
              ("After, usage stays flat", crop("a-usage", span("a-usage", (".meter", 0), (".meter", 1))))], False),
    ("05-top-app-bar", "5 · Large flexible top app bar", "Agents, Tasks and Usage open on a 120 bar with a display small emphasized title; it collapses to the small bar on scroll",
     lambda: [("Before", full("b-agents")), ("After, at rest", full("a-agents")), ("After, scrolled", full("a-agents-collapsed")),
              ("Usage, at rest", full("a-usage")), ("Tasks, scrolled", full("a-tasks-collapsed"))], False),
    ("06-shikigami-tiles", "6 · Shikigami tiles", "A library shape is each shikigami's identity, the tone its state; working ones morph to the soft burst and turn; finished ones stay circles",
     lambda: [("Before", crop("b-agents", span("b-agents", (".agroup", 0), (".agroup", 2)))), ("After", crop("a-agents", span("a-agents", (".agroup", 0), (".agroup", 2))))], False),
    ("07-all-clear", "7 · All clear", "When nothing needs you, the hero's place holds a quiet moment: a very sunny shape that settles in, and since when",
     lambda: [("Before", full("b-clear")), ("After", full("a-clear"))], False),
    ("08-press", "8 · Press", "Shape first (one corner step down), then a squish toward the finger; the pressed button of a group widens and its neighbour gives way",
     lambda: [("Before", shot("b-press")), ("After", shot("a-press"))], False),
]

if __name__ == "__main__":
    os.makedirs(DIR, exist_ok=True)
    for name, title, caption, frames, stack in PAIRS:
        page(name, title, caption, frames(), stack)
        print(name)
