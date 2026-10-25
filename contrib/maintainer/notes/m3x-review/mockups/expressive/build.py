"""Writes the expressive pages next to this file from the showcase pages (which stay as they are): each page loads
shapes.css and expressive.css after showcase.css and gets the markup the moments need. Usage: build.py"""
import os, re

HERE = os.path.dirname(os.path.abspath(__file__))
SHOW = os.path.join(HERE, "..", "showcase")
LINKS = '<link rel="stylesheet" href="../showcase.css">'
XLINKS = LINKS + '\n<link rel="stylesheet" href="shapes.css">\n<link rel="stylesheet" href="expressive.css">'

# Identity shapes of the shikigami: the same one wherever the shikigami shows
ME = {"kv-gate": "pentagon", "gentoo": "clover4", "m3x-review": "cookie9", "kernel": "sunny", "prefill-tuning": "cookie6",
      "seimei-web": "puffy-diamond", "dotfiles": "gem", "home-tidy": "flower"}
CHECK = '<svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>'


def read(name):
    return open(os.path.join(SHOW, name)).read()


def write(name, s):
    open(os.path.join(HERE, name), "w").write(s)


ALLOW = '<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>'
# Working tiles sit at different points of their morph, so a still shows the shape, the burst and the way between
PHASE = {"m3x-review": -1500, "kernel": -4100, "prefill-tuning": -3650}


def allow_icon(s):
    # Allow, the medium button of the card, leads with its 24 icon (8 to the label)
    return s.replace('morph" type="button">Allow</button>', f'morph" type="button">{ALLOW}Allow</button>')


def expressive(s, body_class=""):
    s = allow_icon(s.replace(LINKS, XLINKS, 1))
    # The bottom tab is named after seimei (the top lane stays Chat)
    s = re.sub(r'(id="nav-chat"[^>]*>.*?</span>)Chat</button>', r"\1seimei</button>", s, count=1, flags=re.S)
    if body_class:
        s = s.replace("<body ", f'<body class="{body_class}" ', 1)
    return s


def scrolled(s, sel, y):
    return s.replace("</body>", f"<script>addEventListener('load', () => {{ document.querySelector('{sel}').scrollTop = {y}; }});</script>\n</body>", 1)


def needs_shapes(s):
    # The tile of a Needs you row takes the shape of whoever asks
    s = s.replace('<span class="tt ask">', f'<span class="tt ask" style="--me: var(--shape-{ME["gentoo"]})">', 1)
    return s.replace('<span class="tt review">', f'<span class="tt review" style="--me: var(--shape-{ME["home-tidy"]})">', 1)


def agent_shapes(s):
    # Live shikigami get their shape; finished ones keep the plain circle
    def tile(m):
        btn, kind, style, tail, name = m.groups()
        if name not in ME or kind.startswith("done"):
            return m.group(0)
        d = f" animation-delay: {PHASE[name]}ms, 0ms;" if name in PHASE else ""
        return f'{btn}<span class="tile sh {kind}" style="--me: var(--shape-{ME[name]}); {style}{d}{tail}'
    return re.sub(r'(<button class="agent [^"]*" type="button">)<span class="tile ([^"]*)" style="([^"]*)(".*?<b>([^<]+)</b>)', tile, s, flags=re.S)


def no_needs(s):
    # Nothing needs the user: no Needs you group, no count on the Agents tab
    s = re.sub(r'<h2 class="tg ask">.*?</h2><div class="tgrp needs">.*?</button></div>', "", s, count=1, flags=re.S)
    return re.sub(r'<i class="nbadge attn" id="nav-count">\d+</i>', "", s, count=1)


# Tasks: Needs you, wavy bars, the large title, the nav bar
tasks = needs_shapes(expressive(read("06-tasks-now.html"), "xbar"))
write("tasks.html", tasks)
write("tasks-collapsed.html", scrolled(tasks.replace('<body class="xbar"', '<body class="xbar collapsed"', 1), "#log", 330))

# Agents: Needs you, shape tiles, the large title
agents = agent_shapes(expressive(read("08-agents.html"), "xbar"))
write("agents.html", agents)
write("agents-collapsed.html", scrolled(agents.replace('<body class="xbar"', '<body class="xbar collapsed"', 1), "#agents", 300))

# Usage: the large title; the meters stay flat
usage = expressive(read("12-usage.html"), "xbar")
write("usage.html", usage)
write("usage-collapsed.html", scrolled(usage.replace('<body class="xbar"', '<body class="xbar collapsed"', 1), "#usage", 260))

# Chat: the approval card, at rest and with Allow under the finger
chat = expressive(read("02-chat.html"))
write("chat.html", chat)
i = chat.find('<div class="wcard approval perm')
j = chat.find('<button class="btn filled sl morph" type="button">', i)
assert i > 0 and j > i
write("chat-pressed.html", chat[:j] + chat[j:].replace('class="btn filled sl morph"', 'class="btn filled sl morph is-pressed"', 1))

# All clear: before (the showcase with nothing to answer) and after
base = no_needs(read("06-tasks-now.html"))
write("all-clear-before.html", base)
clear = f'<div class="clear"><span class="shape">{CHECK}</span><span><b>All clear</b><span>Since 06:13 PM</span></span></div>'
after = expressive(base, "xbar").replace('All tasks</button></div>', 'All tasks</button></div>' + clear, 1)
write("all-clear.html", after)

# Press: the controls a finger touches most, at rest and pressed (`.is-pressed` stands in for :active, with the state
# layer the real :active shows), before and after
def cut(s, start, end):
    i = s.index(start)
    return s[i:s.index(end, i) + len(end)]


src = read("02-chat.html")
head = src[:src.index("<body")]
foot = cut(src, '<div class="foot"><span class="left"></span><button class="btn icon', "Allow</button></div>")
fab = cut(src, '<button class="fab sl" type="submit" id="send"', "</button>").replace(' disabled=""', "")
nav = cut(src, '<nav class="navbar"', "</nav>")
chip = cut(src, '<button class="chip sl" type="button">', "</button>")
PRESS_CSS = """<style>
html, body { height: auto; overflow: visible; }
body { display: block; padding: 8px 16px 24px; }
.ps h3 { margin: 20px 4px 8px; font: 500 14px/20px var(--sans); color: var(--text-3); }
.ps .pair { display: grid; gap: 10px; }
.ps .lbl { font: 400 12px/16px var(--sans); color: var(--text-3); margin: 0 4px -4px; }
.ps .wcard { margin: 0; }
.ps .fabrow { display: flex; gap: 24px; align-items: center; padding: 4px; }
.ps .navbar { background: var(--container); border-radius: 16px; }
.sl.is-pressed::after { opacity: .14; }
.nav-item.is-pressed .ind::after { opacity: .14; }
.morph.is-pressed { border-radius: var(--shape-md); }
</style>"""


def pressed(frag, needle):
    return frag.replace(needle, needle.replace('class="', 'class="is-pressed ', 1), 1)


def press_page(x):
    foot_x = allow_icon(foot) if x else foot
    nav_x = re.sub(r'(id="nav-chat"[^>]*>.*?</span>)Chat</button>', r"\1seimei</button>", nav, count=1, flags=re.S) if x else nav
    rows = [
        ("Allow on an approval card", f'<div class="wcard approval perm">{foot_x}</div>',
         f'<div class="wcard approval perm">{pressed(foot_x, '<button class="btn filled sl morph"')}</div>'),
        ("Send", f'<div class="fabrow">{fab}</div>', f'<div class="fabrow">{pressed(fab, '<button class="fab sl"')}</div>'),
        ("A nav bar item", nav_x, pressed(nav_x, '<button class="nav-item sl" type="button" data-tab="agents"')),
        ("A tool chip", f'<div class="fabrow">{chip}</div>', f'<div class="fabrow">{pressed(chip, '<button class="chip sl"')}</div>'),
    ]
    body = "".join(f'<h3>{t}</h3><div class="pair"><span class="lbl">At rest</span>{a}<span class="lbl">Pressed</span>{b}</div>' for t, a, b in rows)
    h = head.replace(LINKS, XLINKS, 1) if x else head
    return h.replace("</head>", PRESS_CSS + "\n</head>", 1) + f'<body class="ps" data-screen="press">{body}</body></html>\n'


write("press-before.html", press_page(False))
write("press.html", press_page(True))
print("wrote tasks, agents, usage (each with -collapsed), chat, chat-pressed, all-clear-before, all-clear, press-before, press")
