"""The final set: every showcase screen with the expressive moments merged in where they belong, and nothing bigger than
the showcase (kubus on expressive 01, dissect.md point 10). Reads mockups/showcase/ (left as it is) and writes
mockups/final/, whose pages load the shape library and final.css after showcase.css. Usage: final.py

Moments and where they land:
1 Needs you: the asker's library shape on the tiles, the group's own corner set, emphasized titles (06, 20, 23; 08 and
  the agents lane everywhere). Dropped: the large "seimei" bar title, the title-large header, the 80 rows, the 48 tiles
2 Approval: Allow filled with its icon, 12 corners and wider under the finger, Deny gives way (02, 05, 08-21, 24).
  Dropped: the 56 medium button and its 16 label; the showcase 52 stays
3 Nav bar: already in the showcase (primary pill, emphasized label, the seimei tab); the pill widens when pressed
4 Running steps: already in the showcase (the wave masks the primary, no stop dot)
5 Large flexible top app bar: dropped whole, it is only size; its non-size part, the tinted bar on scroll, is 21
6 Shikigami tiles: identity shapes at the showcase 44, working ones morph to the soft burst and turn, finished stay
  circles; the chat question card takes its asker's shape
7 All clear: its own screen (23), one row in Needs you's place at a needs row's height, a 40 very sunny tile that settles
  in, on the done tone (primary is for working). Dropped: the 72 shape, the headline type, the 120 card
8 Press: shape first, then the squish (CSS :active on every screen); 24 shows Allow under the finger
"""
import os, re

HERE = os.path.dirname(os.path.abspath(__file__))
M = os.path.join(HERE, "..", "mockups")
SHOW, OUT = os.path.join(M, "showcase"), os.path.join(M, "final")
LINKS = '<link rel="stylesheet" href="../showcase.css">'
XLINKS = LINKS + '\n<link rel="stylesheet" href="../expressive/shapes.css">\n<link rel="stylesheet" href="final.css">'

# Identity shapes, the same as the expressive pass (NOTES.md)
ME = {"kv-gate": "pentagon", "gentoo": "clover4", "m3x-review": "cookie9", "kernel": "sunny", "prefill-tuning": "cookie6",
      "seimei-web": "puffy-diamond", "dotfiles": "gem", "home-tidy": "flower"}
PHASE = {"m3x-review": -1500, "kernel": -4100, "prefill-tuning": -3650}
ALLOW = '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>'
CHECK = '<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>'


def need(s, old, new, count=1):
    assert old in s, old[:60]
    return s.replace(old, new, count)


def agent_shapes(s):
    # Live shikigami get their shape; finished ones keep the plain circle
    def tile(m):
        btn, kind, style, tail, name = m.groups()
        if name not in ME or kind.startswith("done"):
            return m.group(0)
        d = f" animation-delay: {PHASE[name]}ms, 0ms;" if name in PHASE else ""
        return f'{btn}<span class="tile sh {kind}" style="--me: var(--shape-{ME[name]}); {style}{d}{tail}'
    return re.sub(r'(<button class="agent [^"]*" type="button">)<span class="tile ([^"]*)" style="([^"]*)(".*?<b>([^<]+)</b>)', tile, s, flags=re.S)


def merge(s):
    if LINKS not in s:
        return s  # 22, the notification shade: Android's own surface, nothing to merge
    s = need(s, LINKS, XLINKS)
    s = s.replace('morph" type="button">Allow</button>', f'morph" type="button">{ALLOW}Allow</button>')
    # Needs you on Tasks: gentoo asks, home-tidy waits for review
    s = s.replace('<span class="tt ask">', f'<span class="tt ask" style="--me: var(--shape-{ME["gentoo"]})">', 1)
    s = s.replace('<span class="tt review">', f'<span class="tt review" style="--me: var(--shape-{ME["home-tidy"]})">', 1)
    # The question card in chat wears its asker's shape
    s = re.sub(r'(<div class="wcard asking[^"]*"[^>]*><div class="head"><span class="badge")(>.*?<div class="title"><b>([^<]+)</b>)',
               lambda m: f'{m.group(1)} style="--me: var(--shape-{ME[m.group(3)]})"{m.group(2)}' if m.group(3) in ME else m.group(0), s, flags=re.S)
    return agent_shapes(s)


def no_needs(s):
    # Nothing needs the user: no Needs you group, no count on the Agents tab
    i = s.index('<h2 class="tg ask">')
    j = s.index('<h2 class="tg', i + 10)
    s = s[:i] + s[j:]
    return need(s, re.search(r'<i class="nbadge attn" id="nav-count">\d+</i>', s).group(0), "")


os.makedirs(OUT, exist_ok=True)
pages = sorted(n for n in os.listdir(SHOW) if re.match(r"\d\d-.*\.html$", n))
done = {}
for n in pages:
    done[n] = merge(open(os.path.join(SHOW, n)).read())
    open(os.path.join(OUT, n), "w").write(done[n])

# 23: All clear, Tasks with nothing to answer
row = (f'<div class="allclear"><span class="shape">{CHECK}</span>'
       '<span class="txt"><b>All clear</b><span>Since 6:13 PM</span></span></div>')
clear = no_needs(done["06-tasks-now.html"])
i = clear.index('<h2 class="tg')
open(os.path.join(OUT, "23-all-clear.html"), "w").write(clear[:i] + row + clear[i:])

# 24: Allow under the finger (`.is-pressed` stands in for :active and its state layer)
chat = done["02-chat.html"]
i = chat.index('<div class="wcard approval perm')
j = chat.index('<button class="btn filled sl morph" type="button">', i)
open(os.path.join(OUT, "24-allow-pressed.html"), "w").write(chat[:j] + chat[j:].replace('class="btn filled sl morph"', 'class="btn filled sl morph is-pressed"', 1))
print("final:", len(pages) + 2, "pages")
