# UI dissection with the user, 2026-10-24 (before the t13 build resumes)

Decisions land here one by one; hand the whole list to dev with the build.

1. Wording: one noun, "task". "to-do" goes everywhere (the user's own board items are tasks owned by them).
   - "your to-do" -> "your task"
   - "2 more to-dos" -> "2 more of yours"
   - project row "1 to-do" -> "1 for you"
   - status: proposed, waiting on the user

2. Language audit of every user-facing string (web, android, pushes, cards): m3-review on board t14, writes docs/m3x-review/GLOSSARY.md. decisions from it land here.

3. Repetition: rule "every fact once per screen; if an element shows it, no text repeats it". proposed cuts:
   - agents header "3 working · 2 need you" (sections + nav badge already say it)
   - "Running ..." prefix on every Working row (just the command)
   - "Idle" line under the Idle header (show the last thing it did instead)
   - "Finished shikigami" -> "Finished"; archived row "2 shikigami and sessions" -> "2"
   - project prefix when it equals the shikigami name ("gentoo · gentoo ...")
   - chat: one "Summoned" chip per summon right under my text that already names it (chip only, or fold chips into one row)
   - chat: my text after a report card retelling the card (seimei prompt side: add only what the card lacks)
   - card header "gentoo asks" + "Needs you" + question tile icon: pick one signal
   status: proposed, waiting on the user

4. M3 behavior gaps (review covered springs, reduced motion, 48dp, window classes; nothing on these): screen transitions (shared axis for tabs, container transform row -> page, fade through for nav) + predictive back; undo snackbar instead of confirm dialogs; android notification actions (approve/deny, inline reply to questions, grouping); writing rules (sentence case, verbs on buttons, no please/successfully); a11y (200% font scale, screen reader live regions, focus order); haptics on approve/deny/swipe; scroll behavior (app bar tint, jump to latest, per-tab scroll memory). queued as board t15 for m3-review after t14.

5. GLOSSARY.md (561f6f5): 23 terms, 60 conflicts, ~80 cuts, rules. my rec: take it whole, except open points for the user:
   - DECIDED (user): state words are one word: "home-tidy · review", "gentoo · question" (not "ready to review", "has a question"). "why use many word when few do trick". applies to every state label: review, question, running, done, failed, paused...
   - DECIDED: no "brief", keep "task" for a shikigami's instructions
   - DECIDED: "Give to me" -> "Take over"; "Hand to seimei" -> "Hand off" (user)
   - the one-word state rule overrides GLOSSARY rows 25/26 ("ready to review" -> "review", "needs you" -> "question" etc.)
   - M3 style guide decides: USE contractions (spelled out only for emphasis on a caution), no period on single sentences, ellipsis only for in-progress, sentence case, serial comma, no em dashes. sent to m3-review to fix GLOSSARY.md rules with t15
   status: rest of GLOSSARY.md = my rec to take whole; user asked "anything else?" and has not objected

6. BEHAVIOR.md (880f752), top 11: per-approval notifications with Allow/Deny (grouped), sending/failed state on answers (silent failure today = bug), 200% text breaks + Android card body clip (bug), one live region / TalkBack announce, RemoteInput reply on pings, haptics, undo on tick + persistent web undo + real unpair dialog, scroll memory/jump to latest/top bar tint, transitions (Lateral lanes, Top level nav, forward/backward pages, reduced motion; legacy easing, no springs for screen transitions), predictive back, notification/dialog writing. my rec: all 11 into the build, bugs (#2, #3) first.
