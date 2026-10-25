# seimei's words: glossary, conflicts and cuts

Every string a user can see was collected: the web page (`web/`, all JS and HTML, including toasts, empty states, errors, aria labels and confirm sheets), the Android app (`android/`, including notification channels and settings), what the server writes into the chat, its notifications and approval cards (`crates/`), and the showcase mockups (`docs/m3x-review/mockups/showcase/`). That is about 1,400 strings. This page has three parts: one word per concept (the glossary), every place where the words disagree (conflicts), and every string that repeats the screen or says more than it needs to (cuts). Report only: no app code was changed.

Paths are shortened: `A/` is `android/app/src/main/java/com/hisamiowo/seimei/`, `core/` is `crates/core/src/`, `server/` is `crates/server/src/`. Android usually mirrors the web string, so a row lists both places. The showcase mockups are generated: their rows point to the generator line (`tools/tasks_mock.py`, `tools/showcase.py`), since the generated HTML has the whole page on a few lines. Agents (the tab) and shikigami (the workers on it) are two names on purpose and are not listed. Where REVIEW.md quotes copy for the Tasks directions ("your to-do", "2 more to-dos", "gentoo has a question"), this page wins.

The user's decisions of 2026-10-24 are folded in: state labels are one word ("home-tidy · review", "gentoo · question"), "task" stays (no "brief"), the two directions of a handover are "Take over" and "Hand off", and the grammar follows the M3 style guide ([grammar and punctuation](https://m3.material.io/foundations/content-design/style-guide/grammar-and-punctuation), [word choice](https://m3.material.io/foundations/content-design/style-guide/word-choice)): contractions, no period on a single sentence, a serial comma, "…" only for an action in progress. The rows below already follow them; [BEHAVIOR.md](BEHAVIOR.md) has the notification, dialog and snackbar writing rules.

## Glossary

| Term | What it is | Replaces |
|---|---|---|
| task | An item on the board, and what seimei gave a shikigami to do (the "Task" block on its page). | to-do, todo, item ("only your own items can be ticked") |
| step | One line of the plan a shikigami keeps for its task (its TaskCreate list); "3 of 5 steps". | "tasks" in "updated its tasks" |
| session | A Claude Code session of the user's own, on the server. | "Session" for a shikigami's details |
| subagent | A helper a session or shikigami starts with the Agent tool. | "agent" in "started 2 agents" |
| server | The machine seimei runs on. | computer, laptop, "seimei" for the machine ("Could not reach seimei") |
| seimei | The dispatcher you chat with, and the app's name. | |
| approval | A card where a session asks to do something, answered with **Allow** or **Deny**; it ends **Allowed** or **Denied**. | "your OK", "card" (in text), "permission prompt", "Approve" (as a button) |
| question | What a shikigami or session asks you; the verb is "asks". | "ask" as a noun ("1 ask"), "asks" as a label, "Asked you a question" |
| needs you | The state of anything with a question or an approval open, as a header or in a sentence; as a label it is "question" or "review". | "waiting for you", "Waiting for your answer", "Waits for you", "need you" |
| working | A shikigami or session in the middle of a turn, as a header or in a sentence ("3 working"); as a label it is "running". | |
| finished, failed, stopped, interrupted | How a shikigami's run ended, as a header or in a sentence. Stopped: you stopped it. Interrupted: a restart cut it off. | "ended" for a shikigami |
| state labels | The one word after "·" on a row: **review** (finished, waits for your look), **question**, **running**, **paused**, **done**, **failed**; for tasks also **open** and **yours** (assigned to you; this page's pick, the decision has no word for it). "home-tidy · review", "gentoo · question". | "ready to review", "asks", "asking", "has a question", "for you", raw state ids |
| reopened | A finished shikigami that got more work. | "follow-ups", "Rounds", "got more work after" |
| archive | Hide a shikigami or session from the Agents list until it does something. | "hide" in the explanations; "Archive" as a file kind |
| take over, hand off | Move a shikigami to your own session, and give it back to seimei. | "Give to me", "Hand to seimei", "Adopted from", "Handed to you" |
| offline | The page or app cannot reach the server. | away, went quiet, not answering, not connected |
| fingerprint login | Logging in with a passkey. The menu ("Add fingerprint login") and the login button ("Use fingerprint") already say fingerprint, so the rest of the UI follows them. | passkey |
| phone | A paired copy of the app. "Device" stays only for "this device" in the browser. | |
| update | A new version of the Android app, and nothing else. | "Update from seimei" as a notification title |
| ping | A notification seimei chooses to send. | |
| 5-hour window, weekly window | The two Claude usage limits. | |

## Conflicts

One row per disagreement. "Fix" is the proposed text; the recommendation says why when it is not obvious.

| # | Concept | Where | Current | Fix |
|---|---|---|---|---|
| 1 | task | `tools/tasks_mock.py:88`, `:151`, `:165-167` (showcase 06, 07) | "your to-do" | "for you" |
| 2 | task | `tools/tasks_mock.py:90` (showcase 06) | "2 more to-dos" | "2 more for you" |
| 3 | task | `tools/tasks_mock.py:111` (showcase 07) | "1 to-do" (project summary) | "1 for you" |
| 4 | task | `tools/tasks_mock.py:164` (swipe still) | "Swipe a to-do right to tick it off" | "Swipe one of your tasks right to tick it off" |
| 5 | task | `core/tasks.rs:333` (error toast on tick) | "only your own items can be ticked here" | "Only your own tasks tick off here" |
| 6 | step | `web/chat.js:52`, `A/Chat.kt:362` | "updated its tasks" | "updated its steps" |
| 7 | task | `web/agents.js:309`, `A/Agents.kt:361` | "Search names, tasks and reports" | "Search" (see cuts; it also lacks the serial comma) |
| 8 | task | `web/agents.js:842`, `A/Agents.kt:766` | "without restarting the task" | cut, see cuts |
| 9 | subagent | `web/chat.js:51`, `A/Chat.kt:361` | "started an agent", "started 3 agents" | "started a subagent", "started 3 subagents" (the card already says "{who}'s subagent", `web/chat.js:322`) |
| 10 | session | `web/agents.js:387`, `:392`, `A/Agents.kt:462` (on a shikigami's page too) | "Session details", "Context 53% used. Session details" | "Details", "Context 53% used" |
| 11 | approval | `web/chat.js:336`, `:362`, `A/ChatUi.kt:921`, `A/Push.kt:113` | "Approve" (cards without options, and the notification action) | "Allow"; the server's own options already say Allow (`core/permissions.rs:143-156`) and the outcome says "Allowed" |
| 12 | approval | `web/chat.js:767`, `A/Ui.kt:455` (header) | "waiting for your OK" | cut, see cuts |
| 13 | approval | `web/agents.js:729`, `A/Agents.kt:826` | "Waits for your OK: run a command" | "Needs you: run a command" |
| 14 | approval | `web/agents.js:786`, `A/Agents.kt:733` | "It plans, you OK the plan, then it works." | cut, see cuts |
| 15 | approval | `A/Push.kt:38` (channel description) | "A card waits for your OK: a session command, a file edit, a desktop action." | "What a session wants to run, edit, or do" |
| 16 | approval | `web/agents.js:453`, `A/Seimei.kt:1122` | "Approve the card in Chat to hand it over." | "Allow it in Chat to hand it off" |
| 17 | approval | `web/agents.js:786`, `A/Agents.kt:733` | "comes to you as a card" | cut, see cuts |
| 18 | needs you | `web/app.js:108` (Agents nav aria) vs `web/app.js:88`, `A/Ui.kt:521` (header) | "2 waiting for you" vs "2 need you" | "2 need you" in both |
| 19 | needs you | `web/agents.js:17`, `A/Agents.kt:117` | "Waiting for your answer" | "Needs you" (it is the page header word, `web/agents.js:413`) |
| 20 | needs you | `core/sessions.rs:323` | "Needs your input on the server" | "Input on the server" (it shows under Needs you) |
| 21 | question | `web/usage.js:88`, `A/Usage.kt:203` | "1 ask", "3 asks" | "1 question", "3 questions" |
| 22 | question | `web/tasks.js:64`, `A/Tasks.kt:97`, `:163` (project label) | "asks" | "question" |
| 23 | question | `tools/tasks_mock.py:84`, `:144` vs `web/agents.js:121` | "gentoo has a question" vs "Asks a question" | "gentoo · question" on Tasks; "Asks a question" goes (see cuts) |
| 24 | state labels | `web/tasks.js:63-69`, `A/Tasks.kt:95-101` | raw state ids as text: "asks", "running", "review", "you", "open", "paused", "done" | one word each: "question", "running", "review", "yours", "open", "paused", "done" (or the B summary counts) |
| 25 | working | `web/usage.js:123`, `A/Usage.kt:248` (shikigami rows) | raw state: "running", "asking", "done" | "running", "question", "done": "asking" is the only raw id left, the other two already are labels |
| 26 | working | `web/agents.js:358`, `A/Agents.kt:248` | "Nothing running" (empty Agents tab) | cut, see cuts |
| 27 | finished | `web/agents.js:413` (page header, a shikigami with no end state) | "Done" | keep: "done" is a state label |
| 28 | interrupted | `web/chat.js:225`, `A/Chat.kt:294` vs `web/chat.js:546` vs `core/workers.rs:2097`, `:2370` | "was interrupted by a restart" vs "was interrupted" vs "was interrupted by a seimei restart" vs "Interrupted by a seimei restart." | "was interrupted by a restart" everywhere |
| 29 | interrupted | `web/chat.js:275`, `A/ChatUi.kt:770` | "Cancelled by a seimei restart" | "Cancelled by a restart" |
| 30 | reopened | `web/usage.js:89`, `:96`, `:116`, `:123`, `A/Usage.kt:204`, `:205`, `:213`, `:237` | "follow-ups", "Rounds: one go", "got more work after", "reopened" | "reopened" everywhere: "Reopened 2 times (1 by you)", row "0 of 2 reopened"; drop "Rounds" |
| 31 | take over, hand off | `web/agents.js:426`, `:428`, `A/Agents.kt:522`, `:525`, `core/adopt.rs:461` | "Give to me", "Give gentoo to me: it goes on as my own session on the server", "Hand to seimei", "tap Hand to seimei on its session page" | "Take over" and "Hand off", aria "Take over gentoo" and "Hand gentoo off to seimei"; "tap Hand off on its session page" ("me" is the only first-person word in the app, and M3 says never mix "my" with "your") |
| 32 | take over | `core/workers.rs:580`, `:583`, `:2259`, `:2342` | "Handed to you as a session.", "Handed to you: it goes on as your session on the server.", "Adopted from your session", "Back from your session" | "You took it over", "You handed it off" |
| 33 | server | `A/Push.kt:156`, `:158` | "seimei said no (403).", "Could not reach seimei." | "The server said no (403)", "Couldn't reach the server" (as `web/agents.js:522`, `A/Seimei.kt:384`); the title is in BEHAVIOR.md topic 5 |
| 34 | server | `A/Ui.kt:319` | "Connect to your seimei", "The address of the computer that runs seimei." | "Server address" (the field label already is); see cuts |
| 35 | server | `A/Seimei.kt:306` vs `A/Ui.kt:349` | "Make a new one on the server." vs "On the seimei page, open the menu..." | "Make a new one on the seimei page" (the code comes from the page, not the server's shell) |
| 36 | server | `web/agents.js:863`, `:871`, `A/Agents.kt:871` vs `web/agents.js:19` | "Open on server", "Stop it and open on server" vs "Open on the server since" | "Open in a terminal", "Stop it and open in a terminal", "In a terminal on the server since" (see #58) |
| 37 | server | `A/Seimei.kt:364` vs `web/tasks.js:143`, `A/Seimei.kt:384`, `:893` | "Could not reach the server. Is it on?" vs "Could not reach the server." | "Couldn't reach the server" |
| 38 | offline | `web/index.html:62` vs `A/ChatUi.kt:398` | "The server went quiet" vs "The server is not answering" | "Can't reach the server" on both (the page cannot tell a sleeping server from a phone without network) |
| 39 | offline | `web/chat.js:766`, `A/Ui.kt:452` vs `web/app.js:93`, `A/Ui.kt:526` | "away since 06:55" (Chat) vs "offline since 06:55" (other tabs) | "Offline since 06:55" |
| 40 | offline | `web/chat.js:965`, `A/Seimei.kt:1159` | "Not connected to the server yet. Your message stays here." | "Offline. Your message stays here." |
| 41 | offline | `A/Seimei.kt:894` vs `web/agents.js:1020`, `A/Seimei.kt:896` | "The server no longer has this; it shows the last copy." vs "Offline: this is the last copy." | keep both meanings, same form: "Gone from the server: this is the last copy" |
| 42 | archive | `web/chat.js:460`, `A/ChatUi.kt:473` (file chip kind) | "Archive" | "Zip" (the same word is the Agents action) |
| 43 | archive | `web/agents.js:269-271`, `:435-436`, `A/Agents.kt:536-537` | "hides it from this list", "hidden from the list" | cut, see cuts |
| 44 | fingerprint login | `web/index.html:56`, `web/account.js:37`, `server/lib.rs:541`, `:573` | "Save a passkey on this device", "already has a passkey", "adding a passkey", "A passkey was added" | "fingerprint login" in each: "Fingerprint login is already on for this device", "Fingerprint login was added" |
| 45 | update | `A/Push.kt:137` (lock-screen title of pings) | "Update from seimei" | "seimei" (an "update" is an app version, `A/Update.kt:140`) |
| 46 | update | `core/push.rs:750`, `:770` (bundled pings) | "(3 updates)", "4 updates" | "(3 pings)", "4 from seimei" |
| 47 | 5-hour window | `core/usage.rs:140` (chat note) vs `web/chat.js:521-526`, `A/Chat.kt:320-331` (limit events) | "5-hour limit at 80%, resets 19:50" vs "5-hour 80% used, ahead of pace, resets 16:00" | one wording, the client's: the server sends the numbers and the page words them |
| 48 | the user | `core/workers.rs:1087` (a shikigami's log) | "Permissions default → plan (by the user)." | "Permissions: default → plan, by you" |
| 49 | model names | `web/chat.js:237` (Summoned line) vs `web/usage.js:123`, `:155` | "opus · high" vs "Opus · high" | "Opus · high" (a product name; effort stays lowercase) |
| 50 | time of day | `web/util.js:47` (locale: "08:23 PM") vs `A/Chat.kt:343`, `A/Usage.kt:76`, `:150`, `:217` ("20:23", "Tue 20:23") vs `core/usage.rs:139`, `core/wake.rs:42` ("%H:%M") | three clocks | the device's own setting: the web already follows the locale; Android uses `DateFormat.is24HourFormat`; the server sends timestamps, not formatted times |
| 51 | durations | `web/util.js:62`, `web/chat.js:97` ("5m", "1m 5s") vs `web/usage.js:13`, `:81`, `core/wake.rs:21-23` ("52 min", "1 h 29 min", "30 s") | two styles | "5m" only in a row's trailing time slot; "5 min", "1 h 29 min" everywhere in text |
| 52 | ellipsis | `web/agents.js:547`, `A/Agents.kt:667` (placeholder) vs `index.html:78` | "Your answer…" vs "Message seimei" | no ellipsis in placeholders: "Your answer" |
| 53 | ellipsis | `A/Tasks.kt:122` vs `web/usage.js:106`, `A/Update.kt:136`, `:147`, `A/ChatUi.kt:511`; buttons `A/Ui.kt:336`, `:387`, `A/Notifications.kt:126`, `A/Seimei.kt:376` | status lines "Loading the board" vs "Adding up…", "Checking for updates…", "Downloading…", "Saving to Downloads…"; buttons that turn into "Checking", "Pairing", "Sending" | status lines in flight end in "…": "Loading the board…"; buttons never do: they keep their verb ("Continue", "Pair") and show an indicator inside (BEHAVIOR.md topic 9) |
| 54 | case | `web/app.js:88`, `:93`, `web/chat.js:766-767`, `A/Ui.kt:452-526` (header subtitles) | "3 working · 2 need you", "offline since 06:55 · last known state", "nothing running" | sentence case like every other line: "Offline since 06:55" (most of these go, see cuts) |
| 55 | contractions | 48 strings, e.g. `web/tasks.js:143`, `web/login.js:39`, `A/Seimei.kt:364`, `A/Device.kt:166`, `A/Address.kt:25`, `web/agents.js:522` vs `web/agents.js:547`, `A/Update.kt:134` | "Could not reach the server.", "The fingerprint did not unlock the key.", "That is not a server address.", "The server did not answer." vs "Can't reply from here right now" | contractions, as M3 asks: "Couldn't reach the server", "The fingerprint didn't unlock the key", "That isn't a server address", "The server didn't answer"; spelled out only for a caution that needs the weight |
| 56 | spelling | `web/chat.js:275`, `A/ChatUi.kt:770` | "Cancelled" (British) next to "color", "license" (American) | "Canceled" |
| 57 | ask | `web/agents.js:785`, `A/Agents.kt:731` (permission mode) vs "asks" for questions | "Ask" (the mode where Claude Code asks for approval) | "Ask first" |
| 58 | open | `web/tasks.js:68`, `tools/tasks_mock.py:100` ("18 open", a task state) vs `web/agents.js:19`, `:863` ("Open on the server", a terminal session) vs `web/agents.js:274` ("Open", a button) | three meanings | "open" stays the task state and "Open" the button; the session state says where it is: "In a terminal on the server since 18:55", buttons "Open in a terminal", "Stop it and open in a terminal" (#36 is the same fix) |
| 59 | end of a fragment | `web/chat.js:1033`, `A/ChatUi.kt:467` vs `web/chat.js:965` | "Not sent. Tap to retry" vs "Your message stays here." | a period only when there are several sentences: "Not sent. Tap to retry." and "Offline. Your message stays here." keep theirs; a single sentence, label or snackbar has none |
| 60 | period | 119 single sentences, e.g. `web/account.js:151`, `web/login.js:39`, `web/account.js:103`, `A/Seimei.kt:380` | "Copied.", "Wrong password.", "Nothing yet.", "Sent. It should arrive in a few seconds." | "Copied", "Wrong password", "Nothing yet" (M3: no period on a single sentence, a label, a snackbar or a dialog body) |
| 61 | serial comma | `web/index.html:32`, `web/agents.js:309`, `A/Agents.kt:361`, `web/index.html:44`, `A/ChatUi.kt:214` | "Chat, trajectory or tasks", "Search names, tasks and reports", "Shikigami reports, restarts and what seimei did about them" | "Chat, trajectory, or tasks"; the other two go (see cuts) |
| 62 | parentheses | `web/account.js:176`, `web/agents.js:749`, `web/chat.js:185` | " (this phone)", " (default)", "(no output)" | M3 keeps parentheses for defining a term: a "This phone" supporting line, a trailing "Default", "No output" |

## Cuts

The bar: a user who has the screen in front of them never reads a word they did not need. "Delete" means the string goes and nothing replaces it. Otherwise the proposal is the shortest text that keeps the one new fact.

### Repeats what the screen already says

| Where | Current | Proposed | Already said by |
|---|---|---|---|
| `web/app.js:88`, `A/Ui.kt:521` (Agents header) | "3 working · 2 need you" | delete | the Needs you and Working sections and the nav badge |
| `web/app.js:93`, `A/Ui.kt:526` | "offline since 06:55 · last known state" | "Offline since 06:55" | "offline" implies the last known state |
| `web/chat.js:767`, `A/Ui.kt:455` (Chat header) | "waiting for your OK" | delete | the approval card in the thread and the chat nav dot |
| `web/chat.js:766`, `A/Ui.kt:452` (Chat header while offline) | "away since 06:55" | delete on Chat | the offline card says it, with the time |
| `web/agents.js:132`, `A/Agents.kt:210` (row line) | "Idle" | delete (the row keeps its project, or its last line) | the Idle section header |
| `web/agents.js:28`, `:35`, `A/Agents.kt:132`, `:140` (row line) | "Working" | delete | the Working section header and the loading tile |
| `web/agents.js:36`, `A/Agents.kt:141`; showcase 08 via `tools/showcase.py:120` | "Running python3 tools/showcase.py" | "python3 tools/showcase.py" | the Working section; the command is the news |
| `web/agents.js:121`, `:131`, `A/Agents.kt:191`, `:207` (row line, no question text) | "Asks a question" | delete | the Needs you section and the question tile |
| `tools/tasks_mock.py:84`, `:144` (showcase 06, 07) | "gentoo has a question" | "gentoo · question" | the question tile and the Needs you group; the state label stays, one word |
| `tools/tasks_mock.py:86` (showcase 06) | "home-tidy · ready to review" | "home-tidy · review" | the review tile (clipboard check) and the group; the state label stays, one word |
| `web/agents.js:184-187`, `:408-411`, `A/Agents.kt:440`, `:506` (project before the line) | "gentoo · gentoo · Which profile..." when the project is the name | drop the project when it equals the name | the row's own name |
| `web/agents.js:346`, `A/Agents.kt:265` | "Finished shikigami" | "Finished" | the tab and every row are shikigami |
| `web/agents.js:287`, `A/Agents.kt:333` (Archived row) | "2 shikigami and sessions" | "2" as the row's trailing count | the tab |
| `web/agents.js:268` (archive sheet head) | "gentoo · shikigami", "kv-gate · your session in yah" | "gentoo", "kv-gate" | the row the sheet was opened from |
| `web/agents.js:322` (archive search) | "Newest first" | delete | the times on the rows |
| `web/chat.js:215`, `A/ChatUi.kt:741`; showcase via `tools/showcase.py:58-61` | "Summoned home-tidy  sonnet · medium" right under "Summoning home-tidy for it" | "home-tidy  sonnet · medium" (name, model, effort, chevron) | seimei's own sentence above it |
| `web/chat.js:237` (summoned meta) | "opus · high · 400k · ~/src/seimei" | "Opus · high" | the folder and context size are on its page |
| `web/chat.js:223`, `A/Chat.kt:288` | "You messaged gentoo: ..." | delete | you wrote it yourself, on gentoo's page |
| `web/chat.js:681`, `:687` (trajectory) | "after dev finished" directly under dev's report | delete when it follows its trigger | the report right above |
| `web/chat.js:54`, `A/Chat.kt:364`; showcase 11 | chip "Asked you a question" right above the question card | delete the chip for AskUserQuestion | the question card |
| `web/agents.js:470`, `A/Agents.kt:589`, `web/agents.js:935` | "A question", "Question" (heading on the card) | delete | the card's tile and its text ends in "?" |
| `web/agents.js:486`, `A/Agents.kt:617` | "Something else? Answer in your own words below." | delete | the answer field and its placeholder |
| `web/agents.js:488`, `A/Agents.kt:621` | "Answer below." | delete | the field |
| `web/agents.js:548`, `A/Agents.kt:667` | "Message gentoo to wake it" | "Message gentoo" | sending wakes it; Archived says so |
| `web/usage.js:92`, `A/Usage.kt:208` (per shikigami cost) | "$0.98 at API prices" | "$0.98" | the section is "At API prices" and the sum line says it |
| `web/usage.js:115`, `A/Usage.kt:236` | "8 shikigami, $4.83 at API prices." | "$4.83 in all" | the 8 rows below |
| `core/workers.rs:455` (Usage "Today in seimei" row label) | "shikigami home-tidy" | "home-tidy" | the names are unique, and "seimei" is the only other row |
| `web/themepick.js:79`, `A/ThemeSheet.kt:63` | "Light or dark" (label before System / Light / Dark) | delete | the three buttons |
| `tools/tasks_mock.py:96` (showcase 06) | "Up next  one per owner" | "Up next" | the owner on every row |
| `tools/tasks_mock.py:100` (showcase 06) | "18 open in 6 projects · 2 paused · done" | "18 open · 6 projects" | paused and done are one tap away in All tasks |
| `index.html:49`, `A/ChatUi.kt:212` | "Ask seimei anything" | delete (slop audit item 4) | the field says "Message seimei" |
| `login.html:20` | "Unlock to reach your server." | delete | the password field and "Log in" |
| `web/chat.js:428`, `A/ChatUi.kt:917` | "Waits for you" | delete (slop audit item 4) | the card's buttons |

### Explains the obvious, or says it long

| Where | Current | Proposed |
|---|---|---|
| `index.html:62-63` | "The server went quiet" + "It is probably asleep. Everything is paused where it was, and this page reconnects on its own when it wakes." | "Can't reach the server" + "Reconnects on its own" |
| `A/ChatUi.kt:398-399` | "The server is not answering" + "Since 18:55. It may be asleep or offline; seimei keeps trying." | "Can't reach the server" + "Since 18:55. Reconnects on its own." |
| `index.html:55-56` | "Use your fingerprint next time?" + "Save a passkey on this device and skip the password." | "Log in with your fingerprint next time?" |
| `web/account.js:105` | "Every tool call by seimei and the shikigami, newest first." | delete |
| `web/account.js:128`; showcase 16 via `tools/showcase.py:173` | "seimei for Android 0.9.4, 14.2 MB. Open the file on the phone to install or update it." | "Android 0.9.4 · 14.2 MB" |
| `web/account.js:130` | "The Android app is not built yet." | "Not built yet" |
| `web/account.js:28` | "For safety, this needs a fresh login: log out, log in again, then try once more." | "Log out and in again first" |
| `server/lib.rs:541` | "Log in again first: adding a passkey or a phone needs a fresh login." | "Log out and in again first" |
| `web/account.js:35` | "Fingerprint login is set up on this device." | "Fingerprint login is on" |
| `web/account.js:181` | "Unpair Pixel 9? It logs out at once and needs a new code to come back." | an M3 dialog: headline "Unpair Pixel 9?", body "It needs a new pairing code to come back", buttons Cancel and Unpair (BEHAVIOR.md topic 3) |
| `web/account.js:189` | "None yet. The app opens with your fingerprint; unpairing a phone here locks it out." | "None yet" |
| `web/usage.js:42`, `A/Usage.kt:107` | "No usage reported yet. Claude Code reports it when a session takes a turn." | "No usage yet" |
| `web/usage.js:43`, `A/Usage.kt:108` | "Updated 06:51 PM. Claude Code reports usage when a session takes a turn." | "Updated 06:51 PM" |
| `web/usage.js:201`, `A/Usage.kt:186-187` | "What this server's Claude Code use would cost on pay-as-you-go API prices (checked 2026-10-20). Other machines on the account are not included." | "Prices of 2026-10-20. This server only." |
| `web/usage.js:116`, `A/Usage.kt:237` | "0 of 2 that finished got more work after." | "0 of 2 reopened" |
| `web/agents.js:358`, `A/Agents.kt:248-250` | "Nothing running" + "Your Claude sessions on the server and the shikigami seimei summons show up here." | "No shikigami or sessions yet" |
| `web/agents.js:323`, `A/Agents.kt:371` | "Nothing archived. Shikigami and idle sessions move here 12 hours after their last activity." | "Nothing archived. Idle ones move here after 12 hours." |
| `web/agents.js:323`, `A/Agents.kt:371` | "Nothing archived matches." | "No matches" |
| `web/agents.js:309`, `A/Agents.kt:361` | "Search names, tasks and reports" | "Search" |
| `index.html:44`, `A/ChatUi.kt:214` | "Shikigami reports, restarts and what seimei did about them show here." | "Nothing yet" |
| `web/tasks.js:197`, `A/Tasks.kt:122` | "Nothing on the board. seimei adds what it queues here." | "No tasks" |
| `web/agents.js:269` | "Back into the list." | delete (the option says Unarchive) |
| `web/agents.js:270` | "Only hides it from this list. The session keeps running, and it comes back here when it does something." | "Keeps running. Back when it does something." |
| `web/agents.js:271` | "Hides it from this list. A message wakes it and brings it back." | "A message brings it back" |
| `web/agents.js:435`, `A/Agents.kt:536` | "Archived. A message wakes it with its full context and puts it back in the list." | "Archived. A message brings it back." |
| `web/agents.js:436`, `A/Agents.kt:537` | "Archived: hidden from the list, still running. It comes back when it does something." | "Archived, still running" |
| `web/agents.js:451`, `A/Seimei.kt:1121` | "gentoo is your session now, open on the server." | "gentoo is yours, on the server" |
| `web/agents.js:485`, `A/Agents.kt:616` | "Answer these on the server: several questions at once works there only." | "Answer on the server" |
| `web/agents.js:776`, `A/Agents.kt:896` | "Messages you send here are typed into this session's window on the server." | "Typed into its terminal on the server" |
| `web/agents.js:786-788`, `A/Agents.kt:733-735` (mode notes) | "It plans, you OK the plan, then it works." / "What Claude Code would ask comes to you as a card." / "File edits go through; commands still ask." / "Claude Code decides what is safe; the rest asks." / "Only what your rules allow; nothing asks." / "Nothing asks. seimei's guard still applies." | Plan first, Ask: delete / Allow edits: "Commands still ask" / Auto: "Claude Code decides" / Only what's allowed: "Never asks" / Bypass: "The guard still applies" |
| `web/agents.js:842`, `A/Agents.kt:766` | "Takes effect at its next turn, without restarting the task. A new effort keeps the cache; a new model rebuilds it once." | "From its next turn. A new model rebuilds the cache once." |
| `web/login.js:27` | "No fingerprint set up yet. Log in with the password, then add it from the menu." | "No fingerprint login here yet. Use the password." |
| `web/login.js:28` | "That did not work. Try again or use the password." | "That didn't work" |
| `A/Ui.kt:319` | "Connect to your seimei" + "The address of the computer that runs seimei. Its page shows it under Phone app." | "The seimei page shows it under Phone app" under the "Server address" field |
| `A/Ui.kt:348` | "Type seimei's password. The phone pairs itself and keeps only its own key; the password is not stored." | "The password isn't stored" |
| `A/Ui.kt:349` | "On the seimei page, open the menu, then Phone app, then Pair a phone. Type the code it shows." | "On the seimei page: menu, Phone app, Pair a phone" |
| `A/Address.kt:15` | "The app only connects over https. Put seimei behind a tunnel or a proxy with a certificate." | "Needs an https address" |
| `A/Seimei.kt:380` | "Sent. It should arrive in a few seconds." | "Sent" |
| `A/Seimei.kt:306` | "That code is wrong or has expired. Make a new one on the server." | "Wrong or expired code" |
| `A/Update.kt:141` | "14.2 MB, signed with the same key. Android asks you to confirm the install." | "14.2 MB" |
| `A/Update.kt:155` | "First allow seimei to install updates. Android opens the setting; switch it on and come back." | "Allow seimei to install updates first" |
| `A/Notifications.kt:103` | "Android shows it as a silent notification that stays (top)." | delete |
| `A/Notifications.kt:122` | "Sounds and channels" + "Approvals, pings, quiet, urgent" | "Sounds and channels" |
| `fcm/Delivery.kt:24` | "Google's push service carries them to this phone." | delete (the title says "Through Firebase") |
| `foss/Delivery.kt:58` | "UnifiedPush. Each notification is encrypted for this phone; ntfy only carries it." | "End-to-end encrypted" |
| `foss/Delivery.kt:60` | "seimei keeps its own connection to the server instead. Install ntfy or another UnifiedPush app to save battery." | "seimei keeps its own connection instead. A push app saves battery." |
| `foss/Delivery.kt:62` | "No push app, and the background connection is off. Install ntfy or another UnifiedPush app, or ..." | "Install a push app, or turn on the background connection" |
| `A/Push.kt:40` (channel description) | "Things seimei tells you about: an APK is up, a run finished, a reply is ready." | "What seimei tells you" |
| `A/Push.kt:41` | "Rare emergencies seimei flags itself." | "Emergencies" |
| `A/Push.kt:42` | "Shown while seimei keeps its own connection open, on phones without a push app." | "While seimei keeps its own connection open" |
| `core/workers.rs:1453-1456` (a failed shikigami's report line) | "The agent ran out of tokens.", "The agent refused." | "Ran out of tokens", "Refused" (its name starts the line) |
| `core/workers.rs:583` (chat note) | "Handed to you: it goes on as your session on the server." | "You took it over" |
| `core/wake.rs:73` | "The server slept for 2 h (18:00 to 20:00)." | "The server slept from 18:00 to 20:00" (the span says the length; parentheses only define terms) |

## Rules that fall out of this

- One word per concept, from the glossary. Raw state ids (`asks`, `review`, `running`, `done`) never reach the screen as themselves; each maps to its word.
- State labels are one word after "·": review, question, running, paused, done, failed (and open, yours for tasks). Headers and sentences keep "Needs you", "Working", "Finished".
- A row's supporting line carries what its section, tile and header do not: no state word under a state header, no project equal to the name, no "shikigami" on the Agents tab.
- Helper text under a control states only a consequence the label can't ("Commands still ask"), never what the label already says.
- Empty states are one short line; no "X shows up here" tours.
- Grammar, from the M3 style guide: contractions ("can't", "didn't"), spelled out only for a caution that needs the weight; a period only when there are several sentences, none on a single sentence, label, snackbar or dialog body; a serial comma; sentence case everywhere, header subtitles included; no em dashes; parentheses only to define a term.
- "…" only for an action in progress on a status line ("Downloading…"), never on a button, a menu item or a placeholder.
- Avoid abbreviations where there's room ("12 hours" in a sentence); "5 min" in text and "5m" in a row's trailing time slot are fine. American spelling, the device's own 12 or 24 hour clock.
- "you" and "your" throughout, never "me" or "my": buttons are verbs from the user's side, "Take over", "Hand off", "Allow", "Archive".
