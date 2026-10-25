# Harness functionality audit

The spec is what seimei tells its agents and users: the dispatcher prompt (`crates/core/src/dispatcher.md`), the shikigami prompt (`crates/core/src/workers.rs:42-52`), the tool descriptions (`crates/core/data/tools.json`), docs/ and the README, and what the web page and the app show. Each finding checks the spec against the code at cfbdc9a and, where it could be done safely, against what the live and dev instances recorded (read-only: workers.json, tasks.json, events.jsonl, claude-queue's event log). Nothing was restarted or written on either instance. Live checks of new behavior ran only in a scratch install with its own HOME (see PUBLISH.md, Install test) and in scratch unit tests in a worktree.

## Ranked findings

| # | finding | severity | verified |
|---|---|---|---|
| 1 | Background shells: a shikigami that ends its turn while one runs is reported done, and the result never reaches anyone | high | code; plausible in live use |
| 2 | Summoning "#NN ..." for an archived board item fails ("#NN is on the board already"), and leaves a worker record behind | high | scratch test |
| 3 | AskUserQuestion is offered to the dispatcher and the shikigami, then always denied; the dispatcher's denial points to an ask tool it does not have | high | code + live command line |
| 4 | Peek, the shikigami page, the agents search and the post-compaction recall headline the task a worker was first summoned with, not the board item it works on | medium | live state |
| 5 | claude-queue "command ended but pid X still holds" is a false positive for some agents, and seimei relays every one: 116 in 3.5 h | medium | cqd log + events |
| 6 | SendMessage to one of the user's sessions in another permission mode is held in that terminal; the prompt says SendMessage "needs no approval" | medium | live events |
| 7 | A summon "#NN ..." whose item is done but not yet archived files a new number, so the board id and the title's #NN disagree | medium | scratch test |
| 8 | `board_task` on a worker's record is set once at summon and never follows `board start` | low | code |
| 9 | Event turns with no seimei input show "on its own" in the chat with no reason; mostly Claude Code's own cross-session notices | low | live events |
| 10 | Web vs Android: Android hides "Open in a terminal" without a `started` field, its password login skips push registration, the queue routes have no client, the page has no notifications | medium | code |

What works as the prompt says (checked, no finding): chat vs trajectory routing and the three markers; the 30-minute hold of info notes (`dispatcher.rs:42`); card timeouts (guard cards 10 min, `approvals.rs:23`; the dispatcher's permission cards 30 min and a shikigami's reminder at 30 min, `permissions.rs:14-15`); the resume cap after a restart (3 per hour, `workers.rs:93-95`); summon's tools/harness options (added at runtime, `tools.rs:237-250`); the 5-worker confirmation (`tools.rs:366`); board drift and the paused-and-untouched note at the start of a turn (`main.rs:452-468`); the done-item archive and its search (`tasks.rs:235-273`). `cargo test --workspace` passes at cfbdc9a (99 passed, 7 ignored).

## 1. Background shells end in a report that never comes

- **Spec:** the shikigami prompt: "Your final message is shown to the user on their phone as your report"; "Before reporting done, run the checks that prove it". The dispatcher hears "when one finishes".
- **What happens:** at a turn's result, `finish()` holds the report only for background *subagents* (`workers.rs:1247-1252`, "Waiting for N subagents"). Background *shells* are tracked (`workers.rs:756-757`, `1100-1116`) only to keep the process up; the turn is reported `done` at once (`workers.rs:1263-1266`). When the shell ends, Claude Code sends `task_notification`, the shell set empties, and the process closes after 5 s idle (`workers.rs:1113-1114`, `886-893`). Nothing starts a turn to read the result. If Claude Code does start one on its own, a second "done" report follows the first. Either way the dispatcher acted on the first one. Shikigami use background shells a lot (one live worker made 226 background Bash calls). The dispatcher now adds "Wait for background jobs in the foreground before ending a turn" to its task texts by hand (this audit's own task has it).
- **Evidence:** the code path above. A transcript scan of the last 80 workers could not tell a waited-for job from a forgotten one, so this is from the code, not a reproduced case.
- **Fix:** treat running shells like subagents: no report while any runs ("Waiting for N background commands"); when the last ends and no turn follows within the grace time, send the worker "your background command finished; check it and report". Also add one line to the shikigami prompt: "Before your final message, wait for background commands you started (Monitor or their output), or stop them."

## 2. Summon on an archived #NN fails

- **Spec:** summon's task_id description: "without it the board gets a new task (with the task's #NN when it starts with one)"; the dispatcher prompt says to start a summon task with its number, like "#59 Fix the flaky deploy test.". The archive (cfbdc9a) says "an update on an archived id brings it back to the board".
- **What happens:** `Board::attach` (`tasks.rs:488-531`) looks the number up with `find`, which sees only the live board. For an archived #NN it falls through to `add` with that id, and `add` refuses an id that is in the archive (`tasks.rs:321-324`) with "#NN is on the board already". `Pool::summon` adds the worker record first (`workers.rs:2203`) and files it second (`workers.rs:2204`), so the summon fails and a worker that never started stays in workers.json.
- **Evidence:** a scratch test in the worktree: an item done, swept to the archive a day later, then `attach("w1", "dev", "#1 Follow up on old work.", ...)` returns `Err(Invalid("#1 is on the board already"))`.
- **Fix:** in `attach`, bring an archived #NN back with `live()` (as `update` does) and link it, or file a fresh number with the task's own words; file before `add_worker`, or drop the record when filing fails. Say "archived" in the error when it is.

## 3. AskUserQuestion is offered, then always refused

- **Spec:** dispatcher: "Answer questions with mcp__seimei__message when you can; ask the user when only they can decide" (it asks in the chat). Shikigami: "call mcp__shikigami__ask with the question and end your turn."
- **What happens:** AskUserQuestion is not in either session's `--disallowedTools` (`dispatcher.rs:48-63`, `workers.rs:74-88`; the live dispatcher's command line lists 14 tools without it). So both models see the tool and use it, and the permission callback always denies it with "Ask the user through seimei instead (your ask tool, or the chat)." (`permissions.rs:255-257`). The dispatcher has no ask tool (`tools.json` "tools": summon ... tasks, no ask), so its denial points it at nothing.
- **Fix:** add AskUserQuestion to both OFF_TOOLS lists, so neither model sees it. Better for the dispatcher, if the user wants it: turn its AskUserQuestion into a chat card with the options as buttons (the card UI exists), since a multiple-choice question is a good phone interaction. Either way, make the denial text per session ("ask in the chat" for seimei, "use mcp__shikigami__ask" for a shikigami).

## 4. The headline is the first summon task, not the current item

- **Spec:** shikigami move along their own queue (`board start`), and "a linked task follows its shikigami" (tasks tool). The page and peek should say what the worker does now.
- **What happens:** the summon text is stored once as `rec["task"]` and shown as the headline everywhere:
  - peek: "X (id) is S. Task: <rec.task>" (`tools.rs:447-453`);
  - the web page: `taskRow(r.task)` (`web/agents.js:778`) and the agents search (`web/agents.js:124`);
  - the app: `TaskRow(r.str("task"))` (`Agents.kt:1024`), search (`Agents.kt:223`);
  - the recall after a compaction: "task <board_task>: <first line of rec.task>" (`crates/seimei/src/main.rs:476-479`).
- **Evidence:** live workers.json: `dev` (22383d02) has task "#110 Drop the Python server, its rollback and the Python tests...", and tasks.json links it to t13 (#143 M3X redesign), #155, #165, #168 and #111 since then.
- **Fix:** headline with `board.current(worker)` (its title and id) when there is one, and show the summon text as "first task" under it. Keep `board_task` current (finding 8) so the recall reads right.

## 5. claude-queue "still holds" flood

- **Spec:** the queue prompt: [seimei] lines tell the dispatcher "when a holder idles or outlives its command". Each line should be news.
- **What happens:** cqd raises `detached` when a scope's command looks ended while a process in it still holds a device (`~/.local/lib/claude-queue/cqd.py:730-736`, `scope_ended` at 530-554). For agent q6488 it fired on every lease: the cqd log shows "grant L238 to q6488 pid ..." followed at once by "detached: q6488's command ended but pid ... still holds compute, npu, render", for L238 through L246 and on. cqd keys it per lease (`detached:L246`), and seimei's relay keys the note `cqd:<key>` (`queue.rs:801-837`), so nothing collapses. Count in cqd's `events.jsonl`: 116 `detached` of 134 events, all for q6488, 31 to 50 an hour, a median of 19 s apart. These are short runs (7 to 16 s each, `history.jsonl`), so "still holds" is wrong: the leader check in `scope_ended` decides "ended" for this agent's scopes as soon as they start. Each one is a line in the dispatcher's next turn.
- **Fix:** in cqd, report `detached` only after the job has stayed detached for a while (seimei already has `STUCK = 600 s`, `queue.rs:30`) and find why `scope_ended` is true at grant time for this agent's scopes (CQ_LEADER of a wrapper that exits at once, likely). In seimei, collapse `detached` per agent (key `cqd:detached:<agent>`) and drop the ones whose job ended before the note was read.

## 6. SendMessage held because of a different permission mode

- **Spec:** dispatcher prompt: "Messaging them (SendMessage) needs no approval."
- **What happens:** Claude Code's own cross-session delivery holds a message from a session in another permission mode until the user OKs it in the receiving terminal. Live events 11699-11702: SendMessage to `cleanup` returned success with "a [Cross-session delivery notice] follows if that session holds it (usually a different permission mode)"; the dispatcher first told the user it was delivered (11701), then corrected itself (11702: "held for your approval in that terminal, because cleanup runs in a different permission mode than me"). The live dispatcher runs in bypassPermissions; the user's sessions run in their own modes.
- **Fix:** the prompt says when a message is held ("a session in another permission mode holds it until the user OKs it in its terminal: say so, and do not report it delivered"); `sessions` and `session_peek` show each session's permission mode (it is in Claude Code's session registry) so the dispatcher knows before it sends. When the session runs in kitty, the page's own "send" path (typing into the window) is the one that always arrives.

## 7. A done #NN gets a new number

- **What happens:** for a #NN that is done but still on the board, `attach` skips the done item and `tid` is dropped because the id exists, so `add` files the next free number (`tasks.rs:516-530`). The scratch test: `#2` done, a summon "#2 Follow up on recent." files `#3` titled "Follow up on recent", while the dispatcher thinks of it as #2.
- **Fix:** reopen the done #NN and link it (as task_id does for a done item, `tasks.rs:507-509`), or tell the dispatcher in the summon result that it became #3.

## 8. board_task stays at the first item

`board_task` is written only in `Pool::file` at summon (`workers.rs:2132`). `board start` (`tasks.rs:562-578`) and a task_id link through `tasks update worker=...` do not touch it. It is read by the summon result, the adopt result (`adopt.rs:611`) and the recall (`main.rs:477`). **Fix:** derive it from `board.current(worker)` when read, and drop the stored field.

## 9. "On its own" event turns

Of the last 4000 live events, 78 event-turn replies went to the chat and 61 stayed in the trajectory, so the default works. 9 of the 78 had an empty `why`, so the chat shows "on its own" (`web/chat.js:712`). They come from turns that started on Claude Code's own input, like the cross-session delivery notice (11702), not a seimei line. **Fix:** name those inputs (a background task finishing, a peer's message, a delivery notice) in `step()` (`dispatcher.rs:1085-1110`) so the mention says what started the turn.

## 10. Web vs Android

The app is mostly a close port of the page (same strings, often the same comments). Every `/api` route has a caller except the queue's. Gaps and differences, worst first:

- **Bug, Android: "Open in a terminal" is hidden when the record has no `started` field.** Web shows it unless `started === false` (`web/agents.js:767`); the app needs `optBoolean("started")`, false when missing (`Agents.kt:981`), while its own Take over button defaults to true (`Agents.kt:640`). Fix: `optBoolean("started", true)`.
- **Bug, Android: a password login on the lock screen does not register push.** The key unlock calls `registerPush()` (`Seimei.kt:383`), `passwordLogin` does not (`Seimei.kt:400-411`), so that session gets no notifications until the next key unlock.
- **The queue view has no client.** `/api/queue` and `/api/queue/action` (`crates/server/src/lib.rs:335-336`, "The user steers the queue from the app") are called by neither the page nor the app. Either build the view or drop the routes.
- **The web page has no notifications and no presence.** Push setup, push test, notification settings and `api/presence` are app-only. The server already speaks Web Push (RFC 8291) for UnifiedPush, so the page could subscribe with the same VAPID key. Without it, someone on the web page (or an iPhone, where there is no app) never hears about a card.
- **Approving from a notification** offers Deny / Allow / Always allow only (`Push.kt:121-122`): no deny with a note, no "Keep planning" for plan cards, and its failure text differs ("Answer didn't go through", `Push.kt:189`, against "Couldn't send your answer" in both apps).
- **Approval countdown:** the page shows "Auto-denies in N min" (`web/chat.js:516-526`); the app hides it on purpose (`ChatUi.kt:1070`). Pick one.
- **Archive on Android** changes the list at once and does not undo it when the server refuses (`Seimei.kt:1150-1166`); the page waits for the server and shows its error (`web/agents.js:253-254`).
- **Errors shown raw:** the app's tune error shows the HTTP body, likely `{"error": ...}` JSON (`Seimei.kt:1186-1188`); the page shows `r.error` (`web/agents.js:832`).
- **Same action, different words** (pick one string each): password errors ("Wrong password" / "That password is wrong.", `web/login.js:39`, `Seimei.kt:314`); file too large ("is over 50 MB" / "is larger than 50 MB"); the laptop error; "Answer on the server" with and without a period; the session details footer; the menu ("Account" with Add fingerprint login, Phone app / "Menu" with About, Notifications); the Archived rows (badge on the page, subtitle in the app).
- **Only one side:** session gone ("The session has ended.") and stale page notes are app-only (`Seimei.kt:982,1027`); "Try now" reloads the page on the web but reconnects in place on Android; passkeys, making a pairing code and unpairing are web-only (by design: the app has its device key); share-into-app is Android-only.
- **Neither:** reordering the board from the UI (the dispatcher's `tasks order` only).
- **Both, same stale headline:** see finding 4.
