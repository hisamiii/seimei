# seimei against Material 3 Expressive: review

Board item #133. A review of the web page's UI and UX against M3 Expressive (M3X), with ranked improvements and static mockups. No app code was changed. The checklist with sources is [REFERENCE.md](REFERENCE.md); the reference apps are in [refs/](refs/README.md).

## How this was done

The page ran on the QA server (`tests/qa/start.sh`, fake claude, nothing live) in headless Firefox through Marionette, at 412x915 (phone), 800x1280 (tablet portrait) and 1280x800 (tablet landscape), with a coarse pointer and the default Catppuccin Mocha theme plus a light-mode pass. A curated set of those screenshots is in [screens/](screens/). Tap targets were measured from the live DOM at 412 wide.

The Android app is native Compose, not a WebView, so nothing done on the web carries over by itself. The Android notes below come from reading the source only: there was no device or emulator, no screenshot test setup, and no gradle cache, so the material3 version that BOM 2026.09.00 resolves to is unverified. The Expressive APIs named below (MaterialExpressiveTheme, MotionScheme.expressive(), LoadingIndicator, ButtonGroup) shipped as `@ExperimentalMaterial3ExpressiveApi` and may still need that opt-in.

The mockups are the real DOM of each state (scripts stripped), the real `web/style.css`, and one small `*-after.css` on top, plus a shared [m3x-tokens.css](mockups/m3x-tokens.css) and [m3x-loading.css](mockups/m3x-loading.css). To open them with the right font, serve the repo root (`python3 -m http.server 8766` in the checkout) and go to `http://127.0.0.1:8766/docs/m3x-review/mockups/approval-after.html`; from `file://` Firefox usually refuses the font under `../../../web/` and shows a fallback. The before/after pairs in [img/](img/) put before on the left (grey strip) and after on the right (purple strip); each side is also there alone at 2x.

## Verdict

seimei is already closer to M3X than most apps that claim it. Color roles are mapped properly onto every theme (including a Material You seed theme), surfaces are tonal rather than shadowed, the nav bar is the new 64dp height, the state layer exists, the composer is the Gemini shape (pill field, round send beside it), assistant text is plain with no bubble, and light mode holds up. What it lacks is the part that makes M3X "expressive": a shape scale (about 30 distinct radii today), emphasized type, spring motion, the new components (loading indicator, button groups, segmented lists), and in a few places 48dp targets. The fixes are mostly CSS.

## Ranked improvements

Ranked by user impact per effort. Effort: S is under an hour or two, M is half a day to a day, L is more. "Android" says what the Compose side needs, with the file it lives in.

| # | Improvement | Web effort | Android | Mockup |
|---|---|---|---|---|
| 1 | Offline on an empty chat: the away card draws over "Ask seimei anything" | S (one line) | Already right: `ChatUi.kt:209` shows the greeting only when online | [m5-offline.png](img/m5-offline.png) |
| 2 | Approval card: 48dp rule rows as a segmented list, the "runs any code" rule in the warning container with an icon, the foot as a full-width button group (note, Deny tonal, Allow filled and widest), no "Waits for you" text on pending cards | S to M | M: `ChatUi.kt:829` ApprovalCard; option rows (`:882`) are about 40dp, Deny is a TextButton | [m1-approval.png](img/m1-approval.png) |
| 3 | 48dp touch pass on the rest (table below): tabs, folds, quote chip, "Give to me", theme segments and seeds | S | Check custom `clickable` rows; Material buttons already get 48dp via minimumInteractiveComponentSize | none |
| 4 | Working state: a contained loading indicator (morphing shape) where the reply will appear instead of the 6px pulsing dot; Stop shows as a rounded square, not a circle | S (CSS file is generated, plus one element in `index.html`) | S if the resolved material3 has it: `ChatUi.kt:385` Working() becomes ContainedLoadingIndicator() | [m2-working.png](img/m2-working.png), [m2-loading-frames.png](img/m2-loading-frames.png) |
| 5 | Usage: the two percentages emphasized and bigger, M3X determinate progress (4dp gap, no stop dot), lists as segmented groups | S | S: `Usage.kt:98` turns the M3 gap and stop dot off on purpose (`gapSize = 0.dp, drawStopIndicator = {}`); deleting only `gapSize` gives the M3X gap and keeps the dot off | [m4-usage.png](img/m4-usage.png) |
| 6 | Agents: each group a segmented list, "Needs you" rows in the warm container with a filled tile, the working tile a small loading indicator instead of a spinning arc that reads as a "C", done rows quieter | M (CSS plus the tile markup in `agents.js`) | M: `Agents.kt:387` AgentTile, rows at `:412` | [m3-agents.png](img/m3-agents.png) |
| 7 | Tokens: adopt the shape scale, emphasized weights and spring easings from [m3x-tokens.css](mockups/m3x-tokens.css) and map the ~30 radii onto 9 steps | M (mechanical, touches most of `style.css`) | S to M: `Ui.kt:144` MaterialTheme becomes MaterialExpressiveTheme with MotionScheme.expressive(), which gives every M3 component the springs; a `Shapes` set replaces ad-hoc RoundedCornerShape values | none |
| 8 | Nav rail from 600dp (M3 medium) instead of 840: at 800x1280 the page is one stretched column with the phone bar | M (the media query plus checking 600 to 839 layouts) | M: `Ui.kt:602` NavigationBar only; NavigationSuiteScaffold switches by window class | none |
| 9 | Fingerprint card offers "Set up" while the server is away (it can't work offline); hide it with the away card | S | not checked | visible in [m5-offline.png](img/m5-offline.png) |

Items 1 to 5 are the "do these first" set: all small, all visible on every use, and none needs a new dependency.

## This is how app X does it, here's seimei

Clock's stopwatch ([refs/08](refs/08-google-clock-timers-stopwatch.png)) puts the decision as big stacked buttons in thumb reach, with the urgent one filled in a warm container. seimei's approval card is the place where a decision matters most, and today it ends in a text "Waits for you", a 40dp pen, a 40dp text Deny and a filled Allow, all squeezed into one line that wraps ("Waits for / you" on the plan card, [screens](screens/412x915-09-cards-pending.png)). M1 turns the foot into a full-width group like MDC's button groups ([refs/02](refs/02-mdc-button-groups.png)) and the rule choices into a segmented list ([refs/03](refs/03-mdc-segmented-list.png)).

Google Home ([refs/09](refs/09-google-home-tiles-and-lists.png)) groups rows into containers with large outer and small inner corners and tells state by tile color and shape. seimei's agents list already groups by state with tinted tiles; M3 makes the groups visible as segmented lists and lets the shape carry state too.

The M3X loading indicator ([refs/05](refs/05-mdc-loading-indicator-frames.png)) exists for short, indeterminate waits like "the model is thinking". seimei uses a 6px pulsing dot in chat and a spinning arc on agent tiles; M2 and M3 use one morphing indicator for both, built with `clip-path` polygons of equal point counts (generated by [tools/gen_loading.py](tools/gen_loading.py)), no library.

Gemini ([refs/10](refs/10-gemini-chat-composer.png)) uses plain assistant text and a pill composer with send beside it, the same as seimei, and Google's own research mockup ([refs/01](refs/01-research-compose-standard-vs-expressive.png)) makes send the one hero action. seimei already matches both; nothing to change here. Gemini hides the nav bar inside a chat; seimei keeps chat chrome at about 108px top and 152px bottom of 915 (28%). That is a real cost, but changing the information architecture is outside this review; noted, not proposed.

The M3X nav bar and rail ([refs/07](refs/07-mdc-nav-bar-and-rail.png)): seimei's bar is already the shorter 64dp with a pill indicator, a match. The rail arrives at 840 rather than 600 (item 8).

Wavy progress ([refs/06](refs/06-mdc-wavy-progress-frames.png)) is for things in motion. The usage meters are static amounts, so M4 uses the flat M3X track with the gap instead (without the stop dot, per the no-dots rule); wavy would fit an upload or a running shikigami's context ring.

## Per view

Login ([screens](screens/412x915-01-login.png)): big filled fingerprint button, filled password field, tonal "Log in", centered mark. Matches M3X well; no change.

Chat ([screens](screens/412x915-03-chat-top.png)): plain assistant text, tonal user bubbles, tonal cards for summons and questions, the state layer on taps. Off: tabs are 40px tall (48 in M3), the quote chip is 25px, folds are 32px, the working indicator is weak (item 4), cards use 20px corners while the log sheet uses 28 and the bottom 24.

Approval cards ([screens](screens/412x915-09-cards-pending.png)): see item 2. The 1.5px primary outline for pending cards is good and stays. The "runs any code" warning is the end of a line in orange text; it should be visible before reading the whole row.

Trajectory ([screens](screens/412x915-06-trajectory.png)) and Tasks ([screens](screens/412x915-07-tasks.png)): calm, readable, good tonal grouping. Off: task step folds are 28px and "n done" folds 34px.

Agents ([screens](screens/412x915-22-agents.png), [800 wide](screens/800x1280-22-agents.png), [1280 wide](screens/1280x800-22-agents.png)): clear grouping by what needs you, tinted tiles, 64px rows. Off: the working arc reads as a "C" when still, groups are not visually contained, the "Needs you" tint is faint. On the session page ([screens](screens/412x915-26-session-running.png)), "Give to me" is a 32px chip.

Usage ([screens](screens/412x915-29-usage.png)): numbers first, tones by threshold, good. Off: plain flat bars, rows separated by hairlines rather than grouped (item 5).

Menu and theme sheet ([menu](screens/412x915-30-menu.png), [sheet](screens/412x915-31-theme-sheet.png)): the theme cards with live previews are a strength. Off: System/Light/Dark is a 32px outlined segmented control (an M3X connected button group at 48 would fit), Material You seed swatches are 28x22.

Light mode ([chat](screens/412x915-32-light-chat.png), [agents](screens/412x915-33-light-agents.png)): roles hold, contrast fine. The after CSS uses only role variables, so it should theme the same way, but the mockups were only shot in Catppuccin dark.

Empty and offline ([empty](screens/412x915-40-empty-chat.png), [offline](screens/412x915-46-offline-away.png)): the empty states are friendly. The offline-over-empty overlap is the one real bug found (item 1).

## Touch targets under 48dp

Measured at 412 wide on the live page. Two earlier rows were dropped: the task tick (`button.g`, 18px) has a `::after` that extends it to about 44px, and the theme sheet audit had picked up page controls behind the sheet.

| Element | Size (px) | Where |
|---|---|---|
| Lane tabs `.ltab` | 40 tall | Chat, Trajectory, Tasks |
| Rule choice `.choice` | 37 tall | approval card |
| Deny / Keep planning `.btn.text` | 40 tall | approval card |
| Note pen `.btn.icon` | 40 wide | approval card |
| Activity fold `.fold` | 32 tall | chat |
| Quote chip `.re` | 25 tall | chat |
| Step fold `.sf` | 28 tall | tasks |
| Done fold `.fold3` | 34 tall | tasks |
| Hand-off `.handoff` ("Give to me") | 32 tall | session page |
| Light/dark segments | 32 tall | theme sheet |
| Seed swatch `.tseed` | 28x22 | theme sheet |

Icon buttons (menu, attach) are 44, which is close; the nav bar, composer and agents rows are fine. Most of the small ones can keep their look and grow their hit area with padding or the `::after` trick the task tick already uses.

## Where not to be expressive

Chat messages stay plain: no shape play on bubbles, no motion on streaming text. The composer and nav bar already match. Springs go on things the finger touches (buttons, rows, Stop) and the one wait indicator, not on scrolling content. All mockup motion has a `prefers-reduced-motion` fallback (springs collapse, the indicator only turns).

## Notes for implementation

M1: the tonal Deny is a deliberate change from text: on a decision card both options deserve a real target, and Allow stays the only filled one. Settled cards keep their result line in the foot (`.settled .foot .left`).

M2: `chat.js` shows Send and Stop as two buttons toggled with `hidden`, so a `border-radius` transition never runs. The mockup uses a keyframe that plays when Stop appears, which works with the current DOM; a true morph would need one button that changes state. The indicator's CSS is generated: [tools/gen_loading.py](tools/gen_loading.py) writes [m3x-loading.css](mockups/m3x-loading.css) (seven shapes, 48 points each, so `clip-path` can interpolate between them).

M3: tile shape transitions only run if rows are updated in place; if `agents.js` re-renders the list, they jump (still correct, just no motion).

M4: the track after the active part is a flex `::after` with a 4px gap; at 100% the gap still takes 4px, and at 0% the gap shows with no active part. Both are acceptable, or hide the `::after` at the ends.

M5: the CSS rule `body.offline .hello { display: none; }` is enough; the cleaner fix is `|| !S.online` in `hello()` (`chat.js:825`) plus a `hello()` call from `render()`, which also matches Android.

Two rules from design feedback, for all views on both shells. No bare status dots: no colored dot before text anywhere (header status, session header, agent rows, trajectory triggers, list trails, the working line, menu items). State is said in words, by the tone of the text or its container, or by the loading indicator where something works. The only dots left are M3 small badges on icons (Chat nav unread, the menu button's update), and those are a choice to confirm. Title on its icon: when a header or row has no supporting line, the title centers vertically on the icon tile; it top-aligns only when a supporting line follows (CSS `.head:has(.sub)`, Compose `verticalAlignment = if (sub == null) CenterVertically else Top`). This covers approval cards, agent rows and task rows. The [Slop audit](#slop-audit) lists every place.

The springs are `linear()` fits of the Compose expressive springs, made by [tools/springs.py](tools/springs.py); values and the table are in [REFERENCE.md](REFERENCE.md).

## The Tasks tab (#138)

The user's verdict: "imo the tasks tab is the worst ux out of everything simply due to how much things it has". This is an information-architecture problem first and an M3X one second, so this section starts with what the tab is for.

### Who uses it and for what

The user glances from their phone to see three things: (a) what needs them, (b) what is running and how far along it is, (c) what comes next. History and paused projects are rarely wanted. Today's tab answers none of the three directly: it lists every project with every live item, the steps of everything running or asking, a done fold per project and the paused projects, sorted by project. The three answers exist in the data (`state`, `owner`, `steps`, `pos`, and `nexts()` already picks each owner's next item) but are spread over the whole list.

### The fixture

The real board lives in the live instance's state, which this review does not read, so [tools/tasks_fixture.py](tools/tasks_fixture.py) writes a board of the same shape (`api/tasks`: tasks with id, title, project, owner, state, pos, worker, steps, done_at; projects with name, color, paused) and the size the user described: 23 open or paused items and 13 done across 8 projects (seimei, gentoo, home, efi-agent, personal, player, and the paused minecraft harness and yah cleanup), owners you, seimei, dev, gentoo, home-tidy, efi-agent, player and m3x-review, states open, running, asks, review, paused and done, steps on the running ones. The mix is a normal day, not a pile-up: one question, one job to review, three of the user's own to-dos, two tasks running, the rest queued. It is in [mockups/tasks-fixture.json](mockups/tasks-fixture.json). The "before" is the real `tasks.js` rendering that fixture on the QA page (fed through its own `setTasks`, no server state touched); the two directions are generated from the same fixture by [tools/tasks_mock.py](tools/tasks_mock.py) inside the real page chrome.

### Before

[t0-tasks-before-all.png](img/t0-tasks-before-all.png): 1736px of content, three phone screens. On the first screen are gentoo, because asking projects sort first, and half of seimei, but the review item and the user's own to-dos are on screens two and three, mixed with open items. 9 step rows show by default. The asking item looks like the open ones (a dotted circle versus a plain one), and review uses a check-in-circle that is close to done's check. Rows are 42px, step folds 28px, done folds 34px.

### Direction A: "Now"

[t1-tasks-a-now.png](img/t1-tasks-a-now.png) (first screen against before), [all screens](img/t1-tasks-a-now-all.png), page [tasks-a-now.html](mockups/tasks-a-now.html). A Now / All tasks switch (a connected button group, 48px) at the top. Now has three groups. **Needs you**, in the warm container: the questions, then what is ready to review, then the user's own to-dos; it shows one to-do with "2 more to-dos" under it, so the group stays short however many to-dos pile up. The tiles tell the three apart at a glance: a question is a filled warm squircle, a job to look over a filled primary circle with a clipboard check (Material's assignment_turned_in: the worker turned its work in; the check sits on a sheet, never in a circle, so it can't pass for a ticked to-do; five options in the real group: [review-icons.png](img/review-icons.png), [page](mockups/review-icons.html), hanko stamp the runner-up), a to-do an empty warm circle that ticks it (48px target). Question and review rows open their shikigami's page. Own to-dos also swipe right to complete; the list shows every row at rest, and the swipe has its own still: [t1-tasks-swipe.png](img/t1-tasks-swipe.png) ([page](mockups/tasks-swipe.html)). **Running**: a row per task with the current step as the second line and an M3X step bar in primary with the count ("1 of 3"). No loading indicator here: a frozen morph shape next to "Running" reads as an icon, and the bar already says how far along each task is. **Up next**: one item per owner (the existing `nexts()`), the user excluded since their to-dos are already above. Only needs-you rows have a leading tile, so the eye lands there first. The supporting line is the owner and the state only, never the project or the `#id`: "gentoo has a question", "home-tidy · ready to review", "kernel · Install and update the EFI entry" (the current step), and just the owner under Up next; a to-do says "your to-do". The project and id are one tap away, in All tasks. What needs the user wraps to two lines instead of an ellipsis. Then one tonal "All tasks" row: "18 open in 6 projects · 2 paused · done". 1168px in all, and the first phone screen holds all three groups: Needs you, both running tasks with their bars, and the first of Up next.

### Direction B: "Quiet board"

[t2-tasks-b-quiet.png](img/t2-tasks-b-quiet.png), [all screens](img/t2-tasks-b-quiet-all.png), page [tasks-b-quiet.html](mockups/tasks-b-quiet.html). It is the All tasks side of A's switch, so the same Now / All tasks group sits at the top with All tasks on. The full board stays, but every project is one 64px row of one segmented list, with a one-line summary in plain words: "1 question · 1 running · 2 queued", "1 to review · 1 to-do · 1 queued", "1 to-do · 1 running · 3 queued". Project rows stay neutral: only the words that need the user are warm and only "running" is primary, so four projects with something for the user don't turn into four warm blocks. Inside an open project the rows keep the `#id` at the end of the supporting line, since this is the view for finding a specific item. Projects sort by question, then review, then running. There is no leading tile, since an initial in a circle says nothing. Opening a project (gentoo here) adds its items as rows of the same list, with the same edges and gaps, set in only by their text; the one row that asks is the only warm one. Done is one "2 done" row, and both paused projects are one row at the end. 996px with one project open, about 650 with all shut.

Fixed after design feedback on the first version: the review tile (dark green on the warm container read as disabled), a frozen mid-swipe row that looked like a rendering bug, the frozen loading shape next to "Running", B's inner list inset with different edges from the project rows, four fully warm project rows in a row, the progress bar's stop dot, "1 yours", and the ragged `#id` column before titles. Then, in later rounds: the review tile's eye became a clipboard check, Now's supporting lines lost the project, and the fixture was made a realistic day instead of a needy one, with to-dos capped at one plus "n more" so all three groups fit on the first screen.

### Recommendation

Do both, as one design: **A is the default view, B is "All tasks".** A answers the glance in one screen; B is a cleaner version of today's list for the rarer "show me everything" moment and replaces it, so nothing is lost. If only one gets built, build A: it fixes the complaint, while B alone still makes the user open projects to find what needs them.

One thing to decide while building: the Needs-you group repeats the Agents tab's "Needs you" for asking shikigami (fine as two views of the same items, but the Agents badge and this group should count the same things). The question itself is not on the row (the Now line is owner and state only); tapping the row opens the shikigami's page, where it is.

| Piece | Web | Android |
|---|---|---|
| A: the Now view (groups, rows, "All tasks") | M: a second render path in `tasks.js` (about 120 lines) over the same board; `nexts()` and `summary()` already exist | M: a `NowList` composable beside `TasksTab` (`Tasks.kt:111`) over the same parsed board (`sections()`, `:80`) |
| A: Now / All switch | S | S: SingleChoiceSegmentedButtonRow (stable), or ButtonGroup if the Expressive API resolves |
| B: projects shut by default with a count summary | S: `renderTasks()` already builds a header per project and a `summary()`; it shows one word, this makes it counts, and default-closed for all | S: `Project()` (`Tasks.kt:140`) and its header |
| Segmented groups, 48dp+ rows, warm needs-you | S with the tokens from item 7 | S: shapes per index on the rows of `TaskLine` (`:180`), `heightIn(min = 56.dp)` |
| Step bar on running rows, no loading indicator in Tasks | S | S: LinearProgressIndicator with its default gap and `drawStopIndicator = {}` |
| Swipe to complete the user's own items | M: pointer events with a spring settle, no library; the tick circle stays as the accessible path | S: SwipeToDismissBox (stable material3) calling the existing `vm.tick` |

All of it is client-side: no API or board change.

## Slop audit

The bar: nothing on screen that looks like it came out of a UI generator. I went through the current screenshots (phone, tablet and light), `web/style.css`, the JS views and the Compose shell, then turned the same eye on my own mockups. Ranked by how often the user sees it. "Mockups" says whether the after pages and PNGs now show the fix.

| # | Where | Why it reads as slop | Fix | Mockups |
|---|---|---|---|---|
| 1 | Status dots. Header subtitle "• waiting for your OK" (`style.css:118-122`, `:504`; `Ui.kt:544`), session header "• wallpaper · Needs you" (`Agents.kt:504`), the fresh dot before agent times (`style.css:799`, `Agents.kt:433`) and list trails (`:707`), the Trajectory "• after app-tester finished" (`style.css:150`, `ChatUi.kt:325`), the pulsing "working" dot (`style.css:287`), the dot in the Android "Update to" menu item (`Ui.kt:562`), and the dead `.wrow .dot` rules (`style.css:449-456`, nothing renders `.wrow` any more). | The single most common tell: a colored dot that restates the words next to it. | Delete them. State is words, the tone of the text or its container, or the loading indicator where something works. A "fresh" item gets primary-colored, weight 500 time text. One exception I would keep: the Chat nav's unread dot (`.ndot`, `style.css:684`) and the update dot on the Android menu button (`Ui.kt:553`) are M3's small badge on an icon with no text to carry the state. If the rule should be absolute, show a count there instead. | Yes, all pages (`m3x-tokens.css`, last block) |
| 2 | Agents nav icon, a four-point sparkle with a small sparkle (`index.html:88`, `Ui.kt:162`). | The 2024 "AI feature" glyph. It says "AI" rather than what the tab holds, which is workers. | The grid, four rounded squares, the user's pick: it says "a set of workers" with no theme to decode, same 24px grid and 1.8 stroke as Chat. Five options in the real nav bar, selected and unselected: [agents-icons.png](img/agents-icons.png) ([page](mockups/agents-icons.html)). | Yes, every after page |
| 3 | Every chat card has the same template: 32px tinted tile, bold title, body (summoned, asks, finished, failed, plan, command). | Six kinds of event that look identical make the decisions look like the log lines. | Only decisions (approval, asks) keep tile plus card. Summoned and finished become one quiet line with a chevron, like the tool chip; a failure keeps the error tone on the line, not a card. | M1 shows the decision cards; the event lines are a note |
| 4 | Filler copy. "Waits for you" in every approval foot (`chat.js:428`, `ChatUi.kt:917`), "Ask seimei anything" under a second copy of the logo (`index.html:49`, `ChatUi.kt:212`), "Answer below." (`agents.js:488`, `Agents.kt:621`), the cost paragraph's first clause "What this server's Claude Code use would cost on pay-as-you-go API prices" (`usage.js:201`, `Usage.kt:186`). | Phrases that would fit any app. "Waits for you" restates that a card has buttons, and wraps to two lines next to them. | Drop "Waits for you" (show only a real expiry). Empty chat: nothing, or the last thing seimei did. "Answer below." goes; the field's placeholder already says it. Usage: "At API prices, checked {prices_checked}". | Approval: gone. The rest are a note |
| 5 | Pill badges: "shikigami" on every agent row (`style.css:797`), "next" in Tasks (`tasks.js:129`, `Tasks.kt:213`), the "Today" day pill, status lines as centered pills ("Session is back."). | A pill around a word adds a shape that means nothing and costs width. "shikigami" repeats the group title, and "next" pushes titles into an ellipsis. | Plain words in the supporting line ("next" in primary, as in B), the group title for "shikigami", and status notes as plain centered text in `text-3`. | Agents: pill removed. Tasks: "next" as a word |
| 6 | Color means four things. Peach is "needs you" in Agents, a high 5-hour use in Usage, the owner "you" in Tasks, the "runs any code" warning and the inactive Usage nav ring. Project tints come from the same accent set, so "seimei" is yellow next to peach and "yah" is teal like done. `Agents.kt:503` hard-codes MAUVE and PEACH. | When a color means "project" in one row and "state" in the next, it stops meaning anything. | Reserve warm for "needs you" and primary for "working", across all views. Draw project names in `text-2` at weight 500. If tints stay, draw them only from hues no state uses. The nav ring is neutral until the limit is close. | Tasks and Agents: neutral projects, primary step bars |
| 7 | Truncation where the text is the content: the asking question in Agents ("Should the glass plane stay at 1.6x li…"), the Tasks title squeezed by the pill and the owner column, the trajectory fold "app-tester finished, asr-bench failed and 3 m…". | An ellipsis that hides the half you need. | Allow 2 lines for questions and for anything that needs the user; a fold says the count ("5 events") rather than a cut sentence. | Agents and Tasks: two lines |
| 8 | Spinner and pulse overuse: `pulse` runs on the status dot, the tool chip, `.call.run b` (pulsing text), the working dot and the sheet badge (`style.css`, 5 places). The working agent tile is a spinning arc (`ARC`, `agents.js:160`) that reads as the letter "C" whenever it is still or caught in a screenshot. My own first Tasks mockups had a loading indicator on every running row (4 at once) and on project tiles. | Several things breathing at once reads as an app that can't settle. | One indicator per area: the working line in chat, one in the Running header, one per working agent tile (the morphing indicator, as in M3, not the arc). Nothing else pulses. | Tasks: one indicator in the header |
| 9 | Optical alignment: a single-line title sat at the top of its 40px tile (my approval mockup). The friend's rule applies everywhere: approval cards, agent rows, task rows. | A title that does not center on its icon looks unfinished. | Center the title on the tile when it is alone; top-align only when a supporting line follows. | Approval: fixed with `:has(.sub)`. Agents and Tasks rows already center |
| 10 | Icon tiles that say nothing. A second and third star (login tile, empty chat tile) under the header's star avatar, and in my first B mockup a letter or spinner tile on every project row. | Tiles as decoration fill a column without adding meaning. | Tiles only where shape or color tells state or invites a tap (ask, review, the user's tick). One star per screen. | Tasks: tiles only on needs-you rows |
| 11 | One font size per element: 18 sizes in `style.css` from 10.5 to 34px, six of them half pixels (10.5, 11.5, 12.5, 13.5, 14.5, 15.5), and the same in Compose (`12.5.sp`, `13.5.sp`). | No type scale shows as a near-miss everywhere: 13 next to 13.5 next to 14. | The M3 roles: 12 label, 14 body/supporting, 16 title/body large, 20 and 22 headers, 28 and 34 display. Line heights 16/20/24. | Tasks: on the scale |
| 12 | Radii and spacing: 22 radius values in `style.css` (1 to 999px) and 14 in Compose; 6, 10 and 14px make up 128 of the padding, margin and gap values. | No shape scale means no rhythm: everything is "a bit rounded". | The shape tokens in `m3x-tokens.css` (xs 4, sm 8, md 12, lg 16, lg-inc 20, xl 28, xl-inc 32, xxl 48, full) and a 4/8 grid, keeping 2px only for segmented gaps. | Tasks: 4/8 and tokens |
| 13 | Hairline outlines: rule choices, "Reply in chat", the login "or" lines, theme tiles (10 1px borders and insets). | An outline around everything flattens the hierarchy that M3 builds from tonal surfaces. | Tonal containers (M1's segmented rule list), outline only on the one secondary action that needs it. | M1 |

Checked and fine: secondary text contrast (`text-3` is 6.6:1 on `container`, 5.7:1 on `container-high`), no gradient text or glows, no emoji as decoration (the theme sheet's check is a symbol in a filled circle and is fine), shadows only on things that float (menu, toast), and labels are already in sentence case.

## Showcase (#142)

The whole app as it would look after the review, before anything gets built: every screen the web app has, one PNG each at 412 wide and full page height, in [img/showcase/](img/showcase/), and all of them on one contact sheet, [all.png](img/showcase/all.png) (three rows: chat, then tasks and agents, then usage, the account menu and offline; 2656 wide, labels in the app's font). The pages are in [mockups/showcase/](mockups/showcase/), the sheet page is [showcase-sheet.html](mockups/showcase-sheet.html).

| # | Screen | What it shows |
|---|---|---|
| 01 | [login](img/showcase/01-login.png) | Fingerprint first, the password below it, no tile around the mark and no "or" rules |
| 02 | [chat](img/showcase/02-chat.png) | A real afternoon: home-tidy summoned and its report, kernel and gentoo started, a "looked in" chip, gentoo's question card, and seimei asking to push (the approval card, M1) |
| 03 | [chat working](img/showcase/03-chat-working.png) | The same thread with the push allowed once and seimei working: the M3X loading indicator and "Working", the stop button in the field |
| 04 | [new chat](img/showcase/04-new-chat.png) | The empty thread: only the fingerprint offer, no second logo and no "Ask seimei anything" |
| 05 | [trajectory](img/showcase/05-trajectory.png) | dev's report, a failure line in the error tone, seimei's replies with "Reply in chat", and the limit note in warm |
| 06 | [tasks Now](img/showcase/06-tasks-now.png) | Direction A: Needs you (question, review with the clipboard check tile, a to-do), Running with step bars, Up next |
| 07 | [tasks All](img/showcase/07-tasks-all.png) | Direction B as the All tasks side of the switch, gentoo open |
| 08 | [agents](img/showcase/08-agents.png) | Needs you, Working (one loading indicator per working tile), Idle, Finished shikigami, Archived |
| 09 | [shikigami asking](img/showcase/09-shikigami-asking.png) | gentoo's page with its question at the end and the answer field |
| 10 | [shikigami working](img/showcase/10-shikigami-working.png) | m3x-review's page while a command runs |
| 11 | [session asking](img/showcase/11-session-asking.png) | A session's question with choices as one segmented list |
| 12 | [usage](img/showcase/12-usage.png) | Both windows, today's turns, cost at API prices, a row per shikigami with its time and state |
| 13 | [menu](img/showcase/13-menu.png) | The account menu |
| 14 | [theme](img/showcase/14-theme.png) | Light or dark as a connected group, the theme tiles, Material You's seed, the accents |
| 15 | [activity](img/showcase/15-activity.png) | The activity log as one segmented list, the blocked call in the error container |
| 16 | [phone app](img/showcase/16-phone-app.png) | Download and pair, and the paired phones |
| 17 | [offline](img/showcase/17-offline.png) | The server out of reach under a conversation, with "Try now" |
| 18 | [answer sending](img/showcase/18-answer-sending.png) | BEHAVIOR.md: Allow tapped, an indicator in the button, the others held |
| 19 | [answer failed](img/showcase/19-answer-failed.png) | BEHAVIOR.md: the buttons back, "Couldn't send your answer" with Retry |
| 20 | [task undo](img/showcase/20-task-undo.png) | BEHAVIOR.md: a task ticked off, "Task done" with Undo until used |
| 21 | [jump to latest](img/showcase/21-jump-to-latest.png) | BEHAVIOR.md: scrolled up, "Latest" with the count of new rows |
| 22 | [notification shade](img/showcase/22-notification-shade.png) | BEHAVIOR.md: two approvals grouped, Deny and Allow on each; a question with its options and an inline reply |

Since t16 every screen also carries DECISIONS.md and GLOSSARY.md (one-word state labels, task, Take over and Hand off, contractions, every fact once) and the spec button group (40px, 2px gap, 8px inner corners, the selected one a full pill, the rest tinted, no check), applied by [tools/decisions.py](tools/decisions.py) after `showcase.py`.

How they were made: the QA server (`tests/qa/start.sh`, fake claude) in a seeded world (projects, shikigami, a board, usage and an activity log), its pages saved as DOM dumps, then `tools/showcase.py` applies what the current web code doesn't do yet and links the after CSS (`m3x-tokens.css` and the other after sheets, then `showcase.css` for what only the showcase needs). The two working shikigami really run: the fake claude's `!run` turn hangs on a Bash call, so their tiles, pages and chips are the app's own working state. The Tasks pages are the A and B mockups from above.

All decisions so far are in force on every screen: no status dots, centered single-line titles, the grid icon for Agents, the clipboard check review tile, owner and state as the supporting line, the type scale (12, 14, 16, 20, 22, 28, 34; 11 only for the nav badge's count), the shape tokens, no hairlines, warm only for what needs the user and primary only for what works, one star per screen, and none of the filler from the slop audit.

Two cards needed a design for this, beyond item 3 of the slop audit. A shikigami report is a quiet line ("home-tidy finished", chevron to its page) on top of its body as one segmented unit, with no tile: it is news, not a decision. A question is the warm card with the agents' ask tile and two buttons, "Open gentoo" (tonal) and "Answer" (filled), so answering is one tap from the chat. Summoned is a quiet line with the model and effort, a failure is a line in the error container with its first sentence.

The slop pass on the result: every rule of the audit checked on the sheet, and a script over the rendered pages for off-scale font sizes and borders. Left on purpose: the nav badge's 11px count (M3's badge label), the inset row rules of a markdown table, the screen-reader-only label on login and the pending approval's primary outline (the one outline the review keeps). The pass found two things, both fixed: the question card in chat still had the app's bare "?" glyph instead of the ask tile's, and the trajectory's limit note said "resets 19:50" next to a Usage tab saying 08:23 PM (it now uses the app's own wording and the same window).

## Files

[REFERENCE.md](REFERENCE.md) is the checklist. [GLOSSARY.md](GLOSSARY.md) is the word audit: one term per concept, every naming conflict and every string to cut. [BEHAVIOR.md](BEHAVIOR.md) is the non-visual half: transitions, back, undo, notifications, accessibility at 200% text, haptics, scrolling and wait feedback. [refs/](refs/README.md) has the reference images with sources: the MDC ones are Apache 2.0, the Play Store and Google design ones belong to Google and are here only as review material. [mockups/](mockups/) has the before/after pages (plus the Agents icon sheet `agents-icons.html`, the review tile sheet `review-icons.html` and the swipe still `tasks-swipe.html`; `showcase/` and `showcase.css` for the showcase), [img/](img/) the PNGs, [screens/](screens/) the current UI, [tools/](tools/) the generators (springs, loading indicator, Tasks fixture, Tasks mockups, the review tile sheet, `showcase.py` and `showcase_sheet.py`); the Agents nav icon swap lives in `tasks_mock.py` and the other after pages.
