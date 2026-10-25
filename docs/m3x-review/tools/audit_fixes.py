"""The fixes of the ui-audit (board #149, AUDIT.md) on the showcase: run after decisions.py, it rewrites the pages in
mockups/showcase/ in place and puts its CSS between markers at the end of showcase.css (a rerun replaces the block).
Every edit has to land on at least one page or be there already; anything else stops the run.
Usage: audit_fixes.py MOCKUPS_DIR

Miss -> fix (numbers as in AUDIT.md; P = a pick where the rule left a choice, for the re-audit to judge):
 1  model names: "Opus · high", "Sonnet · medium" everywhere, the hidden lanes too
 2  snackbars: 19's thread and 20's list make room, so the bar sits below the card and the last row
 3  context ring: the arc draws its number (34 draws 34)
 4  "Your answer" placeholder, no ellipsis
 5  shikigami header: the project goes when it is the name, "Needs you · 6m"
 6  question in chat: one signal; the tile and the warm card stay, "gentoo asks" becomes "gentoo"
 7  "open", not "queued" (07)
 8  sending: the loading indicator in place of the label, as the app does (a969739), so Allow keeps its width;
    the rule row is held too
 9  shade commands break at spaces, not mid-word
 10 shade: Saturday, October 24, 6:54 in the app's 12-hour clock (one "now": resets 8:23 PM in 1 h 29 min)
 11 no retell after dev's report (only what the card lacks: "#47 is done on the board"), no period on the failure line
 12 targets: every control 48 or a 48 hit area (lane tabs, event lines, Reply in chat, fold, snackbar actions, menu,
    attach, swatches, seed, the Now/All tasks group)
 13 activity tags "Allowed", "Blocked: deletes files"
 14 usage section headers in text-2, not primary
 15 P usage warning moves off warm (round 2: to neutral, see N5); warm is needs you only
 16 badge and ring hue on 13 and 21: the PNGs were 256-colour palettes that lost the warm tone; shots are RGB now
 17 fingerprint card: buttons under the title, Set up filled
 18 P Tasks Now: the "All tasks" row goes, the toggle is the way there
 19 Tasks on the 16 grid of the other tabs
 20 07: the toggle has the same room below it as on 06
 21 07: no leading tile on the question row
 22 Archived: the count trails the row
 23 P dotfiles idle line: what it did last ("Planned the stow layout")
 24 11: no "Variant" header, the choices are a segmented list
 25 settled command: no fade when it fits, the tile stays neutral
 26 one monospace: Roboto Mono for report code and the shade
 27 approval foot: the note button is round and tonal, like its neighbours
 28 "Resets 8:23 PM", 82% on 05 and 12, the week starts at its reset (Tue 5:53 PM)
 29 no leading zero on 12-hour times
 30 activity and phone sheets: even spacing
 31 paired phones: one pattern, "This phone · paired Oct 2", "Opened Oct 21 · paired Sep 14"
 32 offline card: "Since 6:52 PM. Reconnects on its own."
 33 21: the top bar takes the container tone while scrolled
 34 shade: real status and tile icons
 35 shade question: title the question, body under 80 characters, two sentences with periods
 36 theme previews: Deny before Allow as in the app, no stray space, no hairline
 37 one card shape: 28 outer corners on every chat card (decision, report, failure line, fingerprint)
 38 failure line spans the column like the cards
 39 in-progress lines end in "…": "Working…", "Running a command…"
 40 P trajectory fold: a full-width row, left aligned, nothing cut
 41 "tokens, cost, and questions"
 round 2 (AUDIT-2.md): N1 the chat question card titles the asker, the question its regular body; N2 one dot spacing in
 the headers; N3 one 48 chip; N4 the shade's reply chips name the choices as the body does; N5 no hardcoded theme
 colour: the wave masks the primary, the caution hue is gone (pick 15 now neutral: usage on primary, error at the limit)
 aria: "need you", "Chat, trajectory, or tasks", the ring label without a period; the hidden chat lane of the Tasks
 pages (an old QA dump) is emptied
 outside review: contrast (nav pill on primary, snackbar action #7028d9, choice descriptions), bottom tab "seimei"
 (the top lane stays Chat), wavy progress on running task step bars only
"""
import glob, os, re, sys

D = sys.argv[1].rstrip("/") + "/showcase/"
PAGES = {os.path.basename(p)[:-5]: open(p).read() for p in sorted(glob.glob(D + "[0-2][0-9]-*.html"))}
TASKS = ("06-tasks-now", "07-tasks-all", "20-task-undo")
missed = []


def on(names):
    return [n for n in PAGES if n.startswith(names)] if names != "*" else list(PAGES)


def sub(names, old, new, count=0, must=True):
    """Replace on the pages that start with `names` ("*" for all); `old` a string or a compiled pattern."""
    hit = done = False
    for n in on(names):
        s = PAGES[n]
        if isinstance(old, re.Pattern):
            t = old.sub(new, s, count=count)
            done |= isinstance(new, str) and "\\" not in new and new != "" and new in s and not old.search(s)
        else:
            t = s.replace(old, new) if not count else s.replace(old, new, count)
            done |= new != "" and new in s and old not in s
        hit |= t != s
        PAGES[n] = t
    if must and not hit and not done:
        missed.append((names, old if isinstance(old, str) else old.pattern))


# 1 model names
sub("*", re.compile(r"(?<![\w/-])(opus|sonnet|haiku)( · )"), lambda m: m.group(1).capitalize() + m.group(2))

# 3 the ring draws its number; aria without the period
def ring(m):
    return re.sub(r'stroke-dasharray="\d+ 100"', f'stroke-dasharray="{m.group(2)} 100"', m.group(0), count=1)
sub("*", re.compile(r'(class="ring sl"[^>]*aria-label="Context (\d+)% used)[^"]*".*?</button>', re.S), ring)
sub("*", re.compile(r'aria-label="Context (\d+)% used\. Session details"'), r'aria-label="Context \1% used, session details"')

# 4
sub("*", 'placeholder="Your answer…"', 'placeholder="Your answer"')

# 5 the shikigami header: no project when it is the name, a separator before the time
sub("*", re.compile(r'(<h1>([^<]+)</h1><div class="sub"><i class="\w+"></i>)<b[^>]*>\2</b> · '), r"\1")
sub("*", re.compile(r'<span class="ask">Needs you (\d+m)</span>'), r'<span class="ask">Needs you · \1</span>')

# 6 the question in chat: the title is the asker alone ("asks" goes), the question stays the body in regular weight
#   (round 2, N1: as a title it was a five-line bold paragraph)
sub("*", re.compile(r'<div class="title"><b>([^<]+) asks</b></div>'), r'<div class="title"><b>\1</b></div>')

# 7
sub("07", re.compile(r"\bqueued\b"), "open")

# 8 sending: the contained indicator in Allow, the rule row held with the rest
sub("18", '<button class="btn filled sl morph busy" type="button" disabled>Allow</button>',
    '<button class="btn filled sl morph busy" type="button" disabled>Allow<span class="m3x-loading loader bare"><i></i></span></button>')
sub("18", '<button class="choice sl" type="button">Always allow', '<button class="choice sl held" type="button" disabled>Always allow')

# 11, 41 the trajectory's words
sub("*", "<p>dev's metrics view is deployed and #47 is done on the board.</p>", "<p>#47 is done on the board</p>")
sub("*", "after 3 retries.</small>", "after 3 retries</small>")
sub("*", "per-shikigami tokens, cost and asks on the Usage tab", "per-shikigami tokens, cost, and questions on the Usage tab")

# 13
sub("15", '<span class="tag">approved</span>', '<span class="tag">Allowed</span>')
sub("15", '<span class="tag">blocked: deletes files</span>', '<span class="tag">Blocked: deletes files</span>')

# 15, 28 usage: one number, sentence case, the week from its reset
sub("*", "5-hour 80% used, ahead of pace", "5-hour 82% used, ahead of pace")
sub("*", re.compile(r'<span class="sub">resets '), '<span class="sub">Resets ')
sub("*", "since Tue 06:53 PM · 214 API calls", "Since Tue 05:53 PM · 214 API calls")

# 17 fingerprint: Set up is the primary action
sub("*", '<button class="btn tonal sl morph" id="fp-yes" type="button">Set up</button>', '<button class="btn filled sl morph" id="fp-yes" type="button">Set up</button>')

# 18 Tasks Now: the toggle already leads to All tasks
sub(("06", "20"), re.compile(r'<button class="tall[^"]*"[^>]*>.*?</button>', re.S), "")

# 21 the board's question row without its tile
sub("07", re.compile(r'(<div class="tr k-ask in">)<span class="tt ask"><svg.*?</svg></span>', re.S), r"\1")

# 22 the archive count trails
sub("08", re.compile(r'<span class="txt"><b>Archived</b><span>(\d+)</span></span>'), r'<span class="txt"><b>Archived</b></span><span class="cnt">\1</span>')

# 23
sub("*", '<span class="line">Plan</span>', '<span class="line">Planned the stow layout</span>')

# 24 no header repeating the question; the choices as one group
sub("11", re.compile(r'(<span class="qt"><b>Which variant should I keep for the next round\?</b>)<small>Variant</small>'), r"\1")
sub("11", re.compile(r'(</div>)((?:<button class="qopt sl" type="button">.*?</button>)+)(</section>)', re.S), r'\1<div class="qopts">\2</div>\3')

# 29 12-hour times without the leading zero
sub("*", re.compile(r"(?<![\d:])0(\d:\d\d [AP]M)"), r"\1")

# 31, 32
sub("16", "<span>Paired Sep 14, last opened Oct 21</span>", "<span>Opened Oct 21 · paired Sep 14</span>")
sub("*", "<p>Reconnects on its own</p>", "<p>Since 6:52 PM. Reconnects on its own.</p>")

# 36 previews: the app's order, no stray space
sub("14", '<span class="tallow">Allow</span><span class="tdeny">Deny</span>', '<span class="tdeny">Deny</span><span class="tallow">Allow</span>')

# 39 in progress
sub("*", '<span class="m3x-loading"><i></i></span><span>Working</span>', '<span class="m3x-loading"><i></i></span><span>Working…</span>')
sub("*", '<span>Running a command</span>', '<span>Running a command…</span>')

# aria
sub("*", re.compile(r'aria-label="Agents, \d+ working, 2 waiting for you"'), 'aria-label="Agents, 3 working, 2 need you"')
sub("*", 'aria-label="Chat, trajectory or tasks"', 'aria-label="Chat, trajectory, or tasks"')
# The hidden chat lane of the Tasks pages is an old QA dump with pre-glossary strings: empty it
sub(TASKS, re.compile(r'(<div class="thread" id="thread" aria-live="polite" hidden="">).*?(</div>\s*<div class="thread" id="traj")', re.S), r"\1\2")

# Bottom tab: seimei (the top lane stays Chat)
sub("*", re.compile(r'(id="nav-chat"[^>]*>.*?</span>)Chat</button>', re.S), r"\1seimei</button>")

# 2, 33 page states
sub("19", "<style>#thread { padding-bottom: 76px; }</style>", "<style>#thread { padding-bottom: 104px; }</style>")
sub("20", "</head>", "<style>.tasks { padding-bottom: 96px; }</style>\n</head>", count=1)
sub("21", "</head>", "<style>.bar, .ltabs { background: var(--container); }</style>\n</head>", count=1)

# 22 the shade: one "now", the app's clock, real icons, the question in 80 characters, Roboto Mono
WIFI = '<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M12 20.5 1.2 9.1A15.5 15.5 0 0 1 22.8 9.1z"/></svg>'
CELL = '<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M21 3v18H3z"/></svg>'
BATT = '<svg width="10" height="16" viewBox="0 0 10 18" fill="currentColor" aria-hidden="true"><path d="M3 0h4v2h2a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H1a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1h2z"/></svg>'
TWIFI = '<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2 8.8a15 15 0 0 1 20 0M5 12.5a10 10 0 0 1 14 0M8.5 16a5 5 0 0 1 7 0M12 19.5h.01"/></svg>'
TDND = '<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M8 12h8"/></svg>'
sub("22", '<div class="status"><span>20:41</span><span class="icons"><i></i><i></i><i style="width: 22px"></i></span></div>',
    f'<div class="status"><span>6:54</span><span class="icons">{WIFI}{CELL}{BATT}</span></div>')
sub("22", '<div class="clock"><b>20:41</b><span>Thursday, October 24</span></div>', '<div class="clock"><b>6:54</b><span>Saturday, October 24</span></div>')
sub("22", '<div class="tile on"><span class="dot"></span>', f'<div class="tile on">{TWIFI}')
sub("22", '<div class="tile"><span class="dot"></span>', f'<div class="tile">{TDND}')
sub("22", "<h3>gentoo asks</h3>\n  <p>Which profile for the laptop: desktop or desktop/hardened? Hardened rebuilds about 1,400 packages, roughly 9 hours</p>",
    "<h3>Which profile for the laptop?</h3>\n  <p>Desktop or hardened? Hardened rebuilds about 1,400 packages, roughly 9 hours.</p>")
sub("22", '.n code { font: 13px/19px ui-monospace, "JetBrains Mono", monospace; color: var(--text-2); word-break: break-all; }',
    ".n code { display: block; font: 13px/19px 'Roboto Mono', ui-monospace, monospace; color: var(--text-2); overflow-wrap: anywhere; }")
sub("22", '@font-face { font-family: "Google Sans Flex"; src: url("../../../../web/fonts/google-sans-flex-latin.woff2"); font-weight: 1 1000; }',
    '@font-face { font-family: "Google Sans Flex"; src: url("../../../../web/fonts/google-sans-flex-latin.woff2"); font-weight: 1 1000; }\n'
    '@font-face { font-family: "Roboto Mono"; src: url("../../../../web/fonts/roboto-mono-latin.woff2"); font-weight: 400 500; }')
sub("22", ".status .icons i { display: block; width: 16px; height: 11px; border-radius: 2px; background: var(--text); opacity: .9; }",
    ".status .icons svg { display: block; }")
sub("22", ".tile .dot { width: 20px; height: 20px; border-radius: 6px; border: 2px solid currentColor; flex: none; }", ".tile svg { flex: none; }")
# 12 the shade's actions are 48 to touch (same text position), its reply chips and Clear all get a 48 hit area
sub("22", ".acts { display: flex; gap: 4px; margin: 8px -8px 0; }", ".acts { display: flex; gap: 4px; margin: 4px -8px -4px; }")
sub("22", ".acts button { height: 40px;", ".acts button { height: 48px;")
sub("22", '<div class="choices"><span>desktop</span><span>desktop/hardened</span></div>', '<div class="choices"><span>Desktop</span><span>Hardened</span></div>')
sub("22", ".choices span { height: 32px;", ".choices span { position: relative; height: 32px;")
sub("22", ".choices { display: flex; gap: 8px;", ".choices { display: flex; gap: 16px 8px;")
sub("22", ".clear span { height: 36px;", '.clear span::before { content: ""; position: absolute; inset: -6px 0; }\n.clear span { position: relative; height: 36px;')
sub("22", ".choices span { position: relative;", '.choices span::before { content: ""; position: absolute; inset: -8px 0; }\n.choices span { position: relative;')

# outside review: the snackbar action takes a token that reads on the inverse surface (4.85, was 3.74)
sub("*", "--inverse-primary: #8839ef;", "--inverse-primary: #7028d9;")

CSS = """
/* audit_fixes.py: begin */

/* 12: 48 to touch. The lane tabs, event lines, Reply in chat, the fold, snackbar actions, menu and attach are 48;
   the Now/All tasks group keeps its 40 look with a 48 hit area (its buttons clip nothing) */
.ltab { height: 48px; }
.evline, .reply-btn { min-height: 48px; }
.snack button { min-height: 48px; padding: 0 16px; border-radius: 24px; }
.snack:has(button) { padding: 4px 4px 4px 16px; }
.icon-btn, .field .attach { width: 48px; height: 48px; }
/* chips and the handoff keep their 40 look; their hit areas reach 48 past the clip of .sl */
.sbar .handoff.sl { overflow: visible; }
/* one separator (round 2, N2): the line was a flex row, so a loose " · " lost its spaces and took the 6 gap each side */
.sbar .sub { display: block; overflow: hidden; text-overflow: ellipsis; }
/* tabular figures widen the spaces and the dot; one line of text, nothing to line up */
.urow span { font-variant-numeric: normal; }
/* one chip (round 2, N3): tool chips take the model chip's 48 height, padding, corners and type; no hit area tricks */
.chip { min-height: 48px; padding: 8px 8px 8px 12px; border-radius: var(--shape-md); font: 400 14px/20px var(--sans); color: var(--text-2); }
.chip::before { content: none; }
.chip svg:last-child { width: 20px; height: 20px; color: var(--text-3); }
.activity.running .chip { padding-left: 16px; }
.sbar .handoff::after { inset: 0; }
.sbar .handoff::before { content: ""; position: absolute; inset: -4px 0; }
.field .attach { margin: 2px 0 2px 4px; }
.tview button, .tseg button { position: relative; overflow: visible; }
.tview button::before, .tseg button::before { content: ""; position: absolute; inset: -4px 0; }
.tdots { gap: 0; }
/* the swatches are 48 boxes around their 40 discs: the inline background shorthand would reset the clip, so it is
   forced; the selected ring sits in the box, 2 clear and 2 ring as before */
.tdot { width: 48px; height: 48px; margin: 0; background-clip: content-box !important; padding: 4px; border: 0; box-sizing: border-box; }
.tdot.on { box-shadow: inset 0 0 0 2px var(--text); }
.tseed { width: 48px; height: 48px; }

/* Nav: the active pill on primary (7.3:1 on the bar), its icon on-primary; the label stays as it was */
.nav-item.on .ind { background: var(--primary); color: var(--on-primary); }
.nav-item.on .ring-ico .t { stroke: color-mix(in srgb, var(--on-primary) 30%, transparent); }
.nav-item.on .ring-ico .f, #nav-usage.on[data-tone] .ring-ico .f { stroke: var(--on-primary); }

/* 15: no warning hue of its own (round 2: no new color role). Usage is primary until the limit, error at it; the 5-hour note is
   plain supporting text; warm means needs you */
.note.warn { color: var(--text-2); }
.meter, .meter.ok, .meter.warn { --tone: var(--primary); }
.meter.bad { --tone: var(--error); }
.ring-ico .f, #nav-usage[data-tone="warn"] .ring-ico .f { stroke: var(--primary); }
#nav-usage[data-tone="bad"] .ring-ico .f { stroke: var(--error); }
#nav-usage.on .ring-ico .f { stroke: var(--on-primary); }

/* Wavy progress on a running task's step bar (amplitude 3, wavelength 40, 4 thick, a 4 gap, the flat track); usage
   meters are amounts and stay flat. The wave is a mask over the theme's primary, so accents and themes carry over */
.tr.k-run .prog { height: 10px; align-items: center; }
.tr.k-run .prog i { height: 10px; border-radius: 0; background: var(--primary);
  -webkit-mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='40' height='10' viewBox='0 0 40 10'%3E%3Cpath d='M0 5 C 5 1.05 15 1.05 20 5 S 35 8.95 40 5' fill='none' stroke='%23000' stroke-width='4' stroke-linecap='round'/%3E%3C/svg%3E") left center / 40px 10px repeat-x; mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='40' height='10' viewBox='0 0 40 10'%3E%3Cpath d='M0 5 C 5 1.05 15 1.05 20 5 S 35 8.95 40 5' fill='none' stroke='%23000' stroke-width='4' stroke-linecap='round'/%3E%3C/svg%3E") left center / 40px 10px repeat-x;
  animation: wave 1.2s linear infinite; }
.tr.k-run .prog::after { height: 4px; }
@keyframes wave { to { -webkit-mask-position: 40px center; mask-position: 40px center; } }
@media (prefers-reduced-motion: reduce) { .tr.k-run .prog i { animation: none; } }

/* Contrast: choice descriptions on the warm options */
.qcard .qopt small { color: var(--on-warn-container); opacity: .82; }

/* 8: sending, the contained indicator in the button, widths fixed by the row, not the label */
.btn.busy::before { content: none; }
.wcard.approval .foot .btn.tonal { flex: 1 1 0; }
.wcard.approval .foot .btn.filled { flex: 1.4 1 0; }
.wcard.approval .choice.held:disabled { opacity: .38; }

/* 19, 20: Tasks on the 16 grid; the toggle has room below it */
.tasks { padding-left: 0; padding-right: 0; }
.tview { margin: 4px 0; }
.tview + .tgrp { margin-top: 20px; }

/* 22 */
.archrow .cnt { flex: none; font-size: 14px; color: var(--text-3); font-variant-numeric: tabular-nums; }

/* 24: the choices as one segmented group */
.qcard .qopts { display: flex; flex-direction: column; }

/* 25: a settled command fades only when it overflows (one line here fits); its tile stays neutral */
.wcard.approval.settled .body { -webkit-mask-image: none; mask-image: none; }
.wcard.approval.settled .badge { background: var(--container-high); color: var(--text-2); }

/* 26: one monospace */
.report .rbody code { font: 13px var(--mono); background: var(--container-high); border-radius: 6px; padding: .1em .35em; }

/* 27: the note button round and tonal like Deny */
.wcard.approval .foot .btn.icon { border-radius: var(--shape-full); background: var(--tonal); color: var(--on-tonal); }

/* 14 */
.usage-h { color: var(--text-2); }

/* 17: the buttons under the title */
.card .actions { margin: 12px 0 0; gap: 8px; }

/* 36 */
.tct code { background: none; padding: 0; }
.tcard { border: 0; box-shadow: none; }

/* 37, 38: one card shape in chat; the failure line spans the column */
.card { border-radius: var(--shape-xl); }
.report .evline { border-radius: var(--shape-xl) var(--shape-xl) var(--shape-xs) var(--shape-xs); }
.report .rbody { border-radius: var(--shape-xs) var(--shape-xs) var(--shape-xl) var(--shape-xl); }
.evline.failed { align-self: stretch; border-radius: var(--shape-xl); padding: 10px 12px 10px 20px; }
.evline.failed > span { flex: 1; }

/* 30: even sheet spacing, 14 under the title as between the blocks: no extra room before Today (15) or the version
   line (16), and the same 14 between the line and each button (the block has its own 12 gap) */
.alist > .aday:first-child { margin-top: 0; }
#sheet-body .pblock { padding-top: 0; }
#sheet-body .pblock .usage-note { margin: 0 0 2px; }

/* 40: the fold as a row of the column, left aligned, nothing cut */
.fold { align-self: stretch; display: flex; align-items: center; justify-content: space-between; gap: 12px; min-height: 48px;
  max-width: none; padding: 12px 16px 12px 20px; border-radius: var(--shape-xl); text-align: left; font: 400 14px/20px var(--sans); }
.fold span { white-space: normal; overflow: visible; text-overflow: clip; }
.fold b { flex: none; }
/* audit_fixes.py: end */
"""

css_path = D + "../showcase.css"
css = open(css_path).read()
css = re.sub(r"\n/\* audit_fixes\.py: begin \*/.*?/\* audit_fixes\.py: end \*/\n", "", css, flags=re.S)
open(css_path, "w").write(css.rstrip("\n") + "\n" + CSS)
for n, s in PAGES.items():
    open(D + n + ".html", "w").write(s)
if missed:
    for m in missed:
        print("NOT APPLIED:", m[0], m[1][:90])
    sys.exit(1)
print("audit fixes applied to", len(PAGES), "pages")
