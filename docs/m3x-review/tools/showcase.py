"""The showcase: every screen of the web app in the final M3X design, as static pages in mockups/showcase/.
The input is DOM dumps of the QA page (fake claude, a seeded world of shikigami, tasks and chat; no live state),
one per screen, plus the Tasks pages from tasks_mock.py. This applies the decisions in force on top: the after CSS,
the markup edits of the earlier mockups, and the slop fixes. Usage: showcase.py DUMPS_DIR MOCKUPS_DIR"""
import re, sys

DUMPS, D = (a.rstrip("/") + "/" for a in sys.argv[1:3])
OUT = D + "showcase/"

SCREENS = [
    ("01-login", "login"), ("02-chat", "chat"), ("03-chat-working", "chat-working"), ("04-new-chat", "new-chat"),
    ("05-trajectory", "trajectory"), ("06-tasks-now", "tasks-a-now"), ("07-tasks-all", "tasks-b-quiet"),
    ("08-agents", "agents"), ("09-shikigami-asking", "agent-asking"), ("10-shikigami-working", "agent-working"),
    ("11-session-asking", "session-asking"), ("12-usage", "usage"), ("13-menu", "menu"), ("14-theme", "theme"),
    ("15-activity", "activity"), ("16-phone-app", "phones"), ("17-offline", "offline"),
]
CSS = ["m3x-tokens", "m3x-loading", "approval-after", "working-after", "agents-after", "usage-after", "offline-after"]

SPARKLE = '<path d="M12 3 13.9 8.6 19.5 10.5 13.9 12.4 12 18 10.1 12.4 4.5 10.5 10.1 8.6ZM18.5 15.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z"></path>'
GRID = ('<path d="M6 4h3.5A1.5 1.5 0 0 1 11 5.5V9a1.5 1.5 0 0 1-1.5 1.5H6A1.5 1.5 0 0 1 4.5 9V5.5A1.5 1.5 0 0 1 6 4zM14.5 4H18a1.5 1.5 0 0 1 '
        '1.5 1.5V9A1.5 1.5 0 0 1 18 10.5h-3.5A1.5 1.5 0 0 1 13 9V5.5A1.5 1.5 0 0 1 14.5 4zM6 13.5h3.5A1.5 1.5 0 0 1 11 15v3.5A1.5 1.5 0 0 1 9.5 '
        '20H6a1.5 1.5 0 0 1-1.5-1.5V15A1.5 1.5 0 0 1 6 13.5zM14.5 13.5H18a1.5 1.5 0 0 1 1.5 1.5v3.5A1.5 1.5 0 0 1 18 20h-3.5a1.5 1.5 0 0 '
        '1-1.5-1.5V15a1.5 1.5 0 0 1 1.5-1.5z"/>')
CHEV = '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9.5 6l6 6-6 6"/></svg>'
# The fake claude's !run script keeps two shikigami working (a hanging Bash call); this is what they would be doing
DOING = {"kernel": "doas make modules_install install", "m3x-review": "python3 tools/showcase.py"}


def card_end(s, i):
    """Index just past the <div> that opens at i."""
    depth, j = 0, i
    for m in re.finditer(r"<(/?)div\b[^>]*>", s[i:]):
        depth += -1 if m.group(1) else 1
        if depth == 0:
            return i + m.end()
    raise ValueError("unclosed div")


def cards(s, cls, fn):
    """Replace each chat card whose class starts with `wcard <cls>` by fn(html)."""
    out, pos = [], 0
    for m in re.finditer(rf'<div class="wcard {cls}\b[^"]*"', s):
        if m.start() < pos:
            continue
        end = card_end(s, m.start())
        out += [s[pos:m.start()], fn(s[m.start():end])]
        pos = end
    return "".join(out) + s[pos:]


def attr(h, name):
    m = re.search(rf'{name}="([^"]*)"', h)
    return m.group(1) if m else ""


def summoned(h):
    # One quiet line with a chevron, like the tool chip: the assistant's message above already said what it is for
    who = re.search(r"<b>Summoned (.*?)</b>", h).group(1)
    meta = re.search(r'<span class="meta">(.*?)</span>', h)
    model = " · ".join(meta.group(1).split(" · ")[:2]) if meta else ""
    return (f'<button class="evline sl" type="button" data-worker="{attr(h, "data-worker")}"><span>Summoned <b>{who}</b>'
            f'<small>{model}</small></span>{CHEV}</button>')


def finished(h):
    # The report: a quiet line, then the report itself as plain text in a low container; no tile, no badge
    who = re.search(r"<b>(.*?) finished</b>", h).group(1)
    body = re.search(r'<div class="body">(.*?)</div>', h, re.S).group(1)
    return (f'<div class="report" data-worker="{attr(h, "data-worker")}"><button class="evline sl" type="button">'
            f'<span><b>{who}</b> finished</span>{CHEV}</button><div class="rbody">{body}</div></div>')


def failed(h):
    # A failure keeps the error tone on the line, not a card
    who = re.search(r"<b>(.*?) failed</b>", h).group(1)
    first = re.search(r'<div class="body"><p>(.*?)</p>', h, re.S).group(1)
    return (f'<button class="evline failed sl" type="button" data-worker="{attr(h, "data-worker")}"><span><b>{who}</b> failed'
            f'<small>{first}</small></span>{CHEV}</button>')


def asking(h):
    # A decision: keeps the tile and the card, in the warm container, with its answer one tap away; the tile has the
    # question glyph of the Agents and Tasks ask tiles
    h = re.sub(r'<span class="badge">.*?</span>', f'<span class="badge">{QIC}</span>', h, count=1, flags=re.S)
    h = re.sub(r'<button class="rbtn sl"[^>]*></button>', "", h)
    who = re.search(r"<b>(.*?) asks</b>", h).group(1)
    foot = (f'<div class="foot"><button class="btn tonal sl morph" type="button">Open {who}</button>'
            '<button class="btn filled sl morph" type="button">Answer</button></div>')
    return h[:-len("</div>")] + foot + "</div>"


def chat(s):
    s = cards(s, "summoned", summoned)
    s = cards(s, "done", finished)
    s = cards(s, "failed", failed)
    s = cards(s, "asking", asking)
    # The limit note in the app's own words, on the same 5-hour window as the Usage tab
    s = s.replace("5-hour limit at 80%, resets 19:50", "5-hour 80% used, ahead of pace, resets 08:23 PM")
    s = re.sub(r'<div class="note"[^>]*>You messaged [^<]*: !run</div>', "", s)
    # The Tasks bar says 3 of 5 for the kernel: the chat says the same
    s = s.replace("It is installing now and writing the EFI entry, step 4 of 5.", "It is installing now and writing the EFI entry, 3 of 5 steps done.")
    # The approval came in as a second event of the day: one "Today" is enough
    s = re.sub(r'<div class="note day" data-id="9\d\d">Today</div>', "", s)
    # Approval card: the after markup of M1
    s = s.replace('<div class="choices">', '<div class="choices m3x-seg">')
    s = s.replace('class="btn icon sl"', 'class="btn icon sl morph"').replace('class="btn text sl"', 'class="btn tonal sl morph"')
    s = s.replace('class="btn filled sl"', 'class="btn filled sl morph"')
    # Working: the contained loading indicator and the square Stop of M2
    s = re.sub(r'<div class="working" id="working"( hidden="")?>.*?</div>',
               lambda m: f'<div class="working" id="working"{m.group(1) or ""}><span class="m3x-loading"><i></i></span><span>Working</span></div>', s, flags=re.S)
    s = s.replace('class="fab tonal sl" type="button" id="stop"', 'class="fab tonal sl morph" type="button" id="stop"')
    # Empty chat: no second logo and no "Ask seimei anything"
    s = re.sub(r'(<div class="hello" id="hello"[^>]*>).*?(</div>)', r"\1\2", s, count=1, flags=re.S)
    return s


def agent_row(m):
    h = m.group(0)
    who = re.search(r"<b>(.*?)</b>", h).group(1)
    return h.replace("Running a command: make -j16 render", "Running " + DOING.get(who, ""))


def agents(s):
    s = re.sub(r'<span class="tile work"([^>]*)>(<svg.*?</svg>)?</span>', r'<span class="tile work"\1><span class="m3x-loading bare small"><i></i></span></span>', s, flags=re.S)
    # The pill repeats what the row already is
    s = s.replace('<span class="tag">shikigami</span>', "")
    # Idle is "your turn", not done: a clock instead of the finished check
    s = re.sub(r'(<span class="tile idle"[^>]*>)<svg.*?</svg>', rf"\1{CLOCK}", s, flags=re.S)
    return re.sub(r'<button class="agent sl.*?</button>', agent_row, s, flags=re.S)


CLOCK = ('<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" '
         'stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="8"/><path d="M12 8v4l2.5 2"/></svg>')
LOADING = '<span class="m3x-loading bare small"><i></i></span>'


def page(s):
    s = re.sub(r'<p class="qnote">Answer below\.</p>', "", s)
    # The working shikigami: no "!run" from the fake; its running call has the page's one loading indicator
    s = re.sub(r'<div class="msg user"[^>]*>!run</div>', "", s)
    s = s.replace('<span class="spin"></span><span>Ran a command</span>', f'{LOADING}<span>Running a command</span>')
    s = s.replace("<span>make -j16 render</span>", f'<span>{DOING["m3x-review"]}</span>')
    s = re.sub(r'(class="ring sl"[^>]*aria-label="Context )0%(.*?<span>)0</span>', r"\g<1>34%\g<2>34</span>", s, flags=re.S)
    return s


# The QA seed's usage totals are one day's; a real server has weeks behind it
MINUTES = {"m3x-review": "52 min", "kernel": "38 min", "gentoo": "31 min", "home-tidy": "12 min", "asr-bench": "6 min",
           "dev": "1 h 40 min", "efi-agent": "9 min", "player": "24 min"}


def usage(s):
    s = re.sub(r"What this server's Claude Code use would cost on pay-as-you-go API prices \(checked ([^)]*)\)\.", r"At API prices, checked \1.", s)
    s = s.replace("shikigami app-tester", "shikigami home-tidy")
    for what, amt, calls in (("This week", "$58.40", "214"), ("All time", "$612.90", "3,418")):
        s = re.sub(rf'(<b>{what}</b><span class="amt">)\$11\.77(</span>(?:<svg.*?</svg>)?<small>[^<]*?)23 API calls', rf"\g<1>{amt}\g<2>{calls} API calls", s, flags=re.S)
    for who, t in MINUTES.items():
        s = re.sub(rf'(<b>{who}</b><span class="amt">[^<]*</span>(?:<svg.*?</svg>)?<small>[^<]*?· )3[01] min', rf"\g<1>{t}", s, flags=re.S)
    return s


def theme(s):
    # The seed picker shows the seed a Material You user would have, not the input's default black
    return s.replace('<input class="tseed" type="color"', '<input class="tseed" type="color" value="#8839ef"')


def activity(s):
    # The two calls the fake's !run left behind
    return re.sub(r'<div class="arow "><div class="atop"><time>[^<]*</time><b>[^<]*</b><span class="tool">Bash</span></div><code>make -j16 render</code></div>', "", s)


PHONES = ('<h2 class="list">Phone app</h2>'
          '<div class="pblock"><p class="usage-note">seimei for Android 0.9.4, 14.2 MB. Open the file on the phone to install or update it.</p>'
          '<a class="btn tonal">Download the app</a></div>'
          '<div class="pblock"><button class="btn filled" type="button">Pair a phone</button></div>'
          '<h3 class="psec">Paired phones</h3><div class="alist">'
          '<div class="prow"><div class="ptext"><b>Pixel 9 (this phone)</b><span>Paired Oct 2, last opened now</span></div>'
          '<button class="btn text" type="button">Unpair</button></div>'
          '<div class="prow"><div class="ptext"><b>Galaxy Tab S9</b><span>Paired Sep 14, last opened Oct 21</span></div>'
          '<button class="btn text" type="button">Unpair</button></div></div>')


def phones(s):
    # The QA server has no APK and no paired phone; this is the sheet on a set-up server
    return re.sub(r'(<div class="sheet-body scroll" id="sheet-body">)<h2 class="list">Phone app</h2>.*?(</div>\s*</section>)',
                  lambda m: m.group(1) + PHONES + m.group(2), s, count=1, flags=re.S)


def common(s, tasks):
    s = s.replace('="../../../web/', '="../../../../web/')
    head = '<link rel="stylesheet" href="../../../../web/style.css">'
    assert head in s
    links = "".join(f'\n<link rel="stylesheet" href="../{c}.css">' for c in CSS + (["tasks-after"] if tasks else []))
    s = re.sub(r'\n<link rel="stylesheet" href="(m3x-[a-z]+|[a-z]+-after)\.css">', "", s)
    s = s.replace(head, head + links + '\n<link rel="stylesheet" href="../showcase.css">', 1)
    s = s.replace(SPARKLE, GRID).replace(SPARKLE.replace("></path>", "/>"), GRID)
    return s


def settled(h):
    # What chat.js settle() leaves: no choices, no buttons, the outcome in the foot
    h = h.replace('class="wcard approval perm', 'class="wcard approval perm settled approved', 1)
    h = re.sub(r'<div class="choices[^"]*">.*?</div>', "", h, count=1, flags=re.S)
    return re.sub(r'<div class="foot">.*?</div>', '<div class="foot"><span class="left">Allowed once</span></div>', h, count=1, flags=re.S)


# The ask glyph the app's own Agents rows use
QIC = re.search(r'<span class="tile ask"[^>]*>(<svg.*?</svg>)', open(DUMPS + "sc-agents.html").read(), re.S).group(1)
for out, src in SCREENS:
    tasks = src.startswith("tasks-")
    s = open((D if tasks else DUMPS + "sc-") + src + ".html").read()
    if not tasks:
        s = theme(phones(activity(agents(page(usage(chat(s)))))))
    if src == "chat-working":
        s = cards(s, "approval", settled)
    s = common(s, tasks)
    s = s.replace("<body ", f'<body data-screen="{out[3:]}" ', 1)
    open(OUT + out + ".html", "w").write(s)
print("wrote", len(SCREENS), "screens to", OUT)
