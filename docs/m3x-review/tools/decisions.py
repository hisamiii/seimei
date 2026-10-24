"""The decisions of DECISIONS.md and GLOSSARY.md on the showcase: run after showcase.py, it rewrites the strings in
mockups/showcase/ in place (one-word state labels, task not to-do, Take over and Hand off, contractions, no period on a
single sentence, every fact once) and adds the BEHAVIOR.md screens 18 to 22. Usage: decisions.py MOCKUPS_DIR"""
import re, sys

D = sys.argv[1].rstrip("/") + "/showcase/"
GONE = object()

# (file prefix or "*", old, new); new GONE removes the element whose whole text is `old`
EDITS = [
    # 01 login
    ("01", "Unlock to reach your server.", GONE),
    # Chat
    ("*", "<span>Summoned <b>", "<span><b>"),
    ("*", "dev finished, asr-bench failed and 1 more", "dev finished, asr-bench failed, and 1 more"),
    ("*", "They are #61 and #62 on the board.", "They're #61 and #62 on the board."),
    ("*", "It is installing now and writing the EFI entry, 3 of 5 steps done.", "It's installing now and writing the EFI entry, 3 of 5 steps done."),
    ("*", "home-tidy is done and waits for your look (#65). The duplicates are set aside, not deleted.", "It's on the board as #65, for your review"),
    ("*", "asr-bench lost the GPU queue three times. I'll run it again tonight, when the queue is quiet.", "I'll run it again tonight, when the queue is quiet"),
    ("*", '<span class="left">Waits for you</span>', '<span class="left"></span>'),
    ("*", "Shikigami reports, restarts and what seimei did about them show here.", "Nothing yet"),
    ("*", "after dev finished", GONE),
    ("*", "after asr-bench failed", GONE),
    ("*", "Use your fingerprint next time?", "Log in with your fingerprint next time?"),
    ("*", "Save a passkey on this device and skip the password.", GONE),
    ("*", "The server went quiet", "Can't reach the server"),
    ("*", "It is probably asleep. Everything is paused where it was, and this page reconnects on its own when it wakes.", "Reconnects on its own"),
    # Headers: the sections, the nav badge and the offline card already say it
    ("*", re.compile(r'<span class="status" id="status" data-s="(waiting|away)"><span class="dot"></span><span id="status-text">[^<]*</span></span>'), ""),
    # Agents
    ("*", '<span class="line">Running ', '<span class="line">'),
    ("*", '<span class="line">Idle</span>', '<span class="line">Pushed the login redirect fix</span>'),
    ("*", "<h2>Finished shikigami</h2>", "<h2>Finished</h2>"),
    ("*", "2 shikigami and sessions", "2"),
    ("*", re.compile(r'aria-label="Give ([^ ]+) to me: it goes on as my own session on the server">Give to me</button>'), r'aria-label="Take over \1">Take over</button>'),
    ("*", re.compile(r'(aria-label="[^"]*")?>Hand to seimei<'), ' aria-label="Hand kv-gate off to seimei">Hand off<'),
    ("*", "Something else? Answer in your own words below.", GONE),
    ("*", re.compile(r'<button class="chip sl" type="button"><span class="spin"></span><span>Asked you a question</span><svg.*?</svg></button>', re.S), ""),
    # The project before the line goes when it is the row's own name
    ("*", re.compile(r'(<b>([^<]+)</b><time>[^<]*</time></span><span class="sub">)<span class="proj"[^>]*>\2</span> · '), r"\1"),
    # Usage
    ("*", "Updated 06:51 PM. Claude Code reports usage when a session takes a turn.", "Updated 06:51 PM"),
    ("*", "shikigami home-tidy", "home-tidy"),
    ("*", "At API prices, checked 2026-10-20. Other machines on the account are not included.", "Prices of 2026-10-20. This server only."),
    ("*", "8 shikigami, $4.83 at API prices. 0 of 2 that finished got more work after.", "$4.83 in all · 0 of 2 reopened"),
    ("*", "<dt>Rounds</dt><dd>one go</dd>", "<dt>Reopened</dt><dd>Never</dd>"),
    ("*", "<dt>Rounds</dt><dd>1 ask</dd>", "<dt>Reopened</dt><dd>Never · 1 question</dd>"),
    ("*", re.compile(r"(· \d+ min) · asking<"), r"\1 · question<"),
    # Sheets
    ("*", "Light or dark", GONE),
    ("*", "Every tool call by seimei and the shikigami, newest first.", GONE),
    ("*", "seimei for Android 0.9.4, 14.2 MB. Open the file on the phone to install or update it.", "Android 0.9.4 · 14.2 MB"),
    ("*", "<b>Pixel 9 (this phone)</b><span>Paired Oct 2, last opened now</span>", "<b>Pixel 9</b><span>This phone · paired Oct 2</span>"),
    # Tasks
    ("*", "gentoo has a question", "gentoo · question"),
    ("*", "home-tidy · ready to review", "home-tidy · review"),
    ("*", ">your to-do<", ">your task<"),
    ("*", "2 more to-dos", "2 more of yours"),
    ("*", ">1 to-do<", ">1 for you<"),
    ("*", ">1 to review<", ">1 review<"),
    ("*", ">has a question · <", ">question · <"),
    ("*", re.compile(r"Up next\s*<small>one per owner</small>"), "Up next"),
    ("*", "18 open in 6 projects · 2 paused · done", "18 open · 6 projects"),
]

# The connected button group of the spec: 40px, 2px gap, 8px inner corners, the selected one a full pill, the rest
# tinted, no check. The busy state of an answer: an indicator before the label, which stays.
CSS = """
/* decisions.py */
.tview, .tseg { gap: 2px; border: 0; }
.tview button, .tseg button { height: 40px; min-height: 40px; padding: 0 16px; border: 0; border-radius: 8px; background: var(--tonal); color: var(--on-tonal); font: 500 14px/20px var(--sans); }
.tview button:first-child, .tseg button:first-child { border-radius: 999px 8px 8px 999px; }
.tview button:last-child, .tseg button:last-child { border-radius: 8px 999px 999px 8px; }
.tview button.on, .tseg button.on { background: var(--primary); color: var(--on-primary); border-radius: 999px; }
.btn.busy { gap: 8px; }
.btn.busy:disabled, .btn.held:disabled { opacity: 1; }
.btn.held:disabled { opacity: .38; }
.btn.busy::before { content: ""; flex: none; width: 16px; height: 16px; border-radius: 50%; border: 2px solid currentColor; border-right-color: transparent; animation: spin .8s linear infinite; }
.snack.sc-on { display: flex; }
.snack.sc-on.sc-chat { bottom: 172px; }
.jump { position: fixed; right: 16px; bottom: 156px; display: flex; align-items: center; gap: 8px; height: 48px; padding: 0 18px 0 14px; border: 0; border-radius: 16px; background: var(--tonal); color: var(--on-tonal); font: 500 14px/20px var(--sans); box-shadow: 0 2px 6px rgb(0 0 0 / .3); z-index: 5; }
.jump b { min-width: 20px; height: 20px; padding: 0 6px; border-radius: 10px; background: var(--primary); color: var(--on-primary); font: 600 12px/20px var(--sans); text-align: center; }
"""


def edit(name, s):
    for where, old, new in EDITS:
        if where != "*" and not name.startswith(where):
            continue
        if isinstance(old, re.Pattern):
            s = old.sub(new, s)
        elif new is GONE:
            s = re.sub(rf"<(\w+)\b[^>]*>\s*{re.escape(old)}\s*</\1>", "", s)
        else:
            s = s.replace(old, new)
    return s


import glob, os
names = sorted(os.path.basename(p) for p in glob.glob(D + "[01][0-9]-*.html"))
for n in names:
    p = D + n
    s = edit(n, open(p).read())
    open(p, "w").write(s)
css = open(D + "../showcase.css").read()
if "/* decisions.py */" not in css:
    open(D + "../showcase.css", "w").write(css + CSS)

# Every edit must have landed somewhere
allhtml = "".join(open(D + n).read() for n in names)
for where, old, new in EDITS:
    if isinstance(old, re.Pattern):
        continue
    if old in allhtml and new is not GONE and old not in str(new):
        print("NOT APPLIED:", old[:70])


def screen(src, out, fn):
    s = open(D + src).read()
    s = s.replace(f'data-screen="{src[3:-5]}"', f'data-screen="{out[3:]}"')
    open(D + out + ".html", "w").write(fn(s))


SPIN_ALLOW = '<button class="btn filled sl morph busy" type="button" disabled>Allow</button>'


def sending(s):
    # 18: Allow tapped, the request has taken over 200 ms: the indicator in the button, the others held
    s = s.replace('<button class="btn icon sl morph" type="button" title="Add a note">', '<button class="btn icon sl morph held" type="button" title="Add a note" disabled>', 1)
    s = s.replace('<button class="btn tonal sl morph" type="button">Deny</button><button class="btn filled sl morph" type="button">Allow</button>',
                  '<button class="btn tonal sl morph held" type="button" disabled>Deny</button>' + SPIN_ALLOW, 1)
    return s


def failed(s):
    # 19: it didn't go through: the buttons are back and the snackbar says so, with Retry; no auto-dismiss. The
    # thread makes room so the card's buttons stay in sight above it
    s = s.replace("</head>", "<style>#thread { padding-bottom: 76px; }</style>\n</head>", 1)
    return s.replace('<div class="snack" id="snack" role="status" hidden=""></div>',
                     '<div class="snack sc-on sc-chat" id="snack" role="status"><span>Couldn\'t send your answer</span><button class="sl" type="button">Retry</button></div>', 1)


def undo(s):
    # 20: the user ticked "Back up the signing key": the next of theirs moves up, Undo waits until used
    s = s.replace("Back up the signing key", "Renew the seimei.dev domain").replace("2 more of yours", "1 more of yours")
    s = re.sub(r'(<h2 class="tg ask">Needs you <small>)5(</small>)', r"\g<1>4\2", s)
    return s.replace('<div class="snack" id="snack" role="status" hidden=""></div>',
                     '<div class="snack sc-on" id="snack" role="status"><span>Task done</span><button class="sl" type="button">Undo</button></div>', 1)


ARROW = '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 5v14M6 13l6 6 6-6"/></svg>'


def jump(s):
    # 21: scrolled up to read: the newest rows arrived below, the button says how many
    s = s.replace('<div class="snack" id="snack"', f'<button class="jump sl" type="button" aria-label="Jump to latest, 3 new">{ARROW}<span>Latest</span><b>3</b></button>\n<div class="snack" id="snack"', 1)
    return s.replace("</body>", "<script>addEventListener('load', () => { document.getElementById('log').scrollTop = 0; });</script>\n</body>", 1)


screen("02-chat.html", "18-answer-sending", sending)
screen("02-chat.html", "19-answer-failed", failed)
screen("06-tasks-now.html", "20-task-undo", undo)
screen("02-chat.html", "21-jump-to-latest", jump)
print("decisions applied to", len(names), "screens, wrote 18 to 21")
