# Voice mode

Voice mode lets the user talk with seimei hands-free, in a long flowing conversation. A speech-to-speech model, Phonic's `phonic-1`, is the voice partner: it answers most turns in its own words and hands every action to the dispatcher.

Voice mode is off by default; `[voice] on = true` in [config.md](config.md) turns it on.

## How a call works

The client streams the microphone to seimei over `GET /api/voice/ws`, and seimei holds Phonic's conversation socket on the other side. The bridge is `crates/core/src/voice`; nothing about a call goes from the phone to Phonic directly.

```
client --PCM over wss--> /api/voice/ws --> voice bridge --wss--> Phonic
                                             |  ^
            lookups (board, notes, files, web)  |  tool_call_output, add_system_message, generate_reply
            ask_seimei --> dispatcher, as a [voice] message in the real thread
```

- **The partner talks.** It answers from the context pack and from read-only lookups that run on seimei's server.
- **The dispatcher acts.** For an action, or a question the partner can't settle, the partner calls `ask_seimei`. The request goes into the thread as the user's message, marked voice. It reaches the dispatcher as `[voice] <request>`, and the dispatcher's prompt says how to answer one: short, fact first, heard rather than read.
- **The answer comes back.** The dispatcher's reply goes to Phonic as the tool call's output. Then, once both sides are quiet, a `generate_reply` asks the partner to say it in its own words. A reply that takes over 150 s gets a "still working" output first, and is spoken when it lands.
- **Both sides land in the thread.** Each finished turn is a `voice` event, `who` being `user` or `partner`, with `cut` on a reply the user barged into or hung up on. A cut reply holds only the words that reached the client, so none the bridge held back. The call so far rides to the dispatcher as a keyed "Voice call so far" note, so its next turn sees what was said.
- **News reaches the partner.** During a call, seimei's other chat replies, the user's typed messages and shikigami reports go to Phonic as stamped `add_system_message` lines.

A conversation that drops, or reaches Phonic's two-hour limit, goes on in a fresh one with a rebuilt pack. The pack's chat section holds the voice turns so far, so the new conversation knows them. When Phonic ends the call after `idle` seconds of silence, the call ends.

## The context pack

The pack is the partner's system prompt, built when a conversation opens. Phonic's first audio does not get slower between 10k and 150k characters of prompt, so the pack is generous, capped at 120k.

| section | from | budget |
|---|---|---|
| voice rules | `voice/partner.md`, always | about 3k |
| `style` | `style.md` in the state folder | 1k |
| `chat` | the newest 60 thread lines, 400 characters each | 16k |
| `board` | the open board items | 8k |
| `work` | the shikigami and the user's sessions, one line each | 6k |
| `notes` | `notes.md` from the top, then the later headings | 30k |
| `memory` | `MEMORY.md` in the `[voice] memory` folder | 24k |

`[voice] sections` lists the sections to send. A section left out also takes its lookups away. Each section is stamped with the time it was built, and the rules say a lookup beats the pack.

## The partner's tools

Every lookup runs on seimei's server and answers through the socket seimei opened, so nothing is exposed to the internet. Answers are cut to 12,000 characters, under Phonic's 16,000.

| tool | does | mode |
|---|---|---|
| `board(query)` | open board items, filtered | sync |
| `shikigami(name, count)` | the list, or one's latest 12 messages, up to 40 | sync |
| `sessions(name, count)` | the list, or one's latest 4 messages, up to 20 | sync |
| `search_notes(query)` | the best matching sections of `notes.md` | sync |
| `search_memory(query)`, `read_memory(name)` | the memory index, or one file | sync |
| `recent_chat(query)` | the whole thread's matching lines, numbered | sync |
| `read_chat(line)` | six lines on each side of one, in full | sync |
| `read_file(path)`, `search_files(query)` | files under the allowed roots | sync |
| `search_web(query)`, `fetch_page(url)` | the web, through OpenRouter and a plain fetch | sync, speech first |
| `ask_seimei(request, topic)` | the dispatcher | async |

- **Fast lookups** are sync with `require_speech_before_tool_call: false`: a 50 ms lookup adds about 0.4 s to an answer.
- **Web tools** take 1 to 3 s. They stay sync, so their answer starts the partner's reply by itself. They ask for speech first: the partner says it is looking, so the wait is not silent.
- **ask_seimei** is plain async. Phonic's `wait_for_response` would block every lookup during the wait and speak the result twice.
- **Names are matched loosely.** Speech recognition hears "fern anvil" as "for-an-anvil". So a name goes to the closest real one: the same letters, one holding the other, or an edit distance within a third.
- **Late answers** name their topic. When the user has spoken since the ask, the partner opens with its own few words for it, like "About the voice doc:". So the answer doesn't mix with the talk of the moment.
- **Approval cards.** When seimei waits on the user's ok for an open ask, the partner says so once per card. The card is in the chat, with Allow and Deny.
- **Read-only.** The board, shikigami and sessions are read through `Tools::lookup`, which runs only the actions it names. A tool call for anything else, such as `tasks` or `summon`, gets "not one of your tools".

`search_web` sends the query to OpenRouter's web plugin with the Exa engine and five results. It returns the model's short answer and each result's title, link and excerpt.

## Speech recognition and the voice

`boosted_keywords` holds up to 200 words, surest first, each at most 50 characters.

- seimei, shikigami and `[voice] keywords`.
- The names of the shikigami, sessions and board projects, also split at dashes.
- The active project folders under the roots, and their docs such as `voice.md`. A folder is active when it or its `.git` changed in the last 14 days, or a known name matches it.

`[voice] pronounce` becomes Phonic's pronunciation dictionary. Turn taking is `eager`, which detects the end of speech about 0.4 s sooner than `standard`.

## The page

The page offers a call from an empty composer. When voice mode is on and there is nothing to send, the send button becomes the voice button.

- **During a call** the composer becomes the call bar: mute, a pill with the state, the level and the time, and hang up. The call goes on while the user moves to other pages.
- **In the chat** the user's spoken turns are their bubbles with a wave mark. The partner's turns are plain lines with a bar on the left, so they never look like seimei's replies. The partner's current line grows as it speaks. A request to seimei shows as an "Asked seimei" line, which pulses until seimei's reply lands under it.
- **Barge-in** is cut on the page. `web/pcm.js` hears 120 ms of the user's voice over the partner, at least -42 dBFS and 10 dB over the room. The page then drops what it has queued to play and sends `barge`. That takes about 0.1 to 0.2 s, against about 1 s for Phonic's own detection.
- **Echo** must not count as the user. A browser's echo canceller may miss audio the page plays itself, so the partner comes back through the microphone. Over the partner, voice must also be 6 dB louder than the loudest echo heard so far, relative to what plays. That estimate starts high, falls 15 dB a second, and the phone keeps it between calls.
- **Mute** sends silence: Phonic expects a steady stream.
- **Speed**, the button next to hang up, steps through 0.9x to 1.3x. The phone keeps it and sends it as `?speed=` when a call starts. Phonic takes `audio_speed` only in a conversation's config, so a change counts from the next call, and the button says so.
- **The debug panel** opens with a tap on the pill, or before any call with `#voicedebug` in the address. It shows the browser, whether its audio runs, and the chunks received, played and cut.
- **Its beeps** play half a second through each way out. The ways are the page's audio context, an audio element, a new context and `beep.wav` as a plain file. Then an oscillator in a context with no options, and the stream (below). Each one reports what the browser did, such as `NotAllowedError` for an autoplay block. In a call, the partner's audio then follows the last way tried.
- The microphone runs with the browser's echo cancellation, noise suppression and gain control. It goes up as 16 kHz PCM16 in 20 ms frames, and the screen stays on during a call.

A call needs https or localhost, since the browser gives the microphone to a secure page only.

### The stream way out

Firefox on Android can play nothing through Web Audio, on tone generator sites too, while an audio element plays a file fine. There the partner's audio goes through the stream, `web/stream.js`.

- **The format.** Each 20 ms of PCM16 becomes a verbatim FLAC frame in fragmented MP4 (`web/fmp4.js`). The frames go to a MediaSource on an audio element. Verbatim frames need no encoder, and MediaSource takes FLAC but no raw PCM.
- **The choice** goes by browser: Firefox on Android gets the stream when it takes FLAC, every other browser Web Audio. No sound test can choose, since a page cannot hear its own output and the echo canceller removes a test sound.
- **The self-test.** When the stream's `play()` is refused, or its clock stands still for 2 s, the call falls back to Web Audio. The panel and the stats say why.
- **Silence ahead.** The stream keeps 0.25 s of silence queued past the playhead. With half of that, Firefox's element starves and its clock slows to about 70 percent. A reply's first chunk replaces that silence from 40 ms past the playhead.
- **The cost** is small. In headless Firefox a reply's first sample played 50 to 77 ms after it arrived, with no stalls. Web Audio schedules 60 ms ahead. The panel and the stats show the median wait and the stalls.
- **The mic gate.** Firefox's echo canceller only removes what plays through its own audio graph, so it misses the stream. Phonic then hears the partner, takes it for the user and cuts its own replies. So while the stream plays, and 0.3 s after, the microphone goes up as silence. When the page hears the user over the partner, it cuts the partner as usual. The microphone's last 300 ms then go up too, so Phonic hears the words that started the barge.
- The page's policy allows `blob:` media, since the MediaSource is reached by a blob URL.

## The phone app

The Android app makes the same call as the page over the same socket, with the same talk button, call bar and chat lines. Its call is `VoiceCall` and `Voice.kt` in `android/shared`, for the watch app to reuse.

- **Echo.** The microphone records as a voice call, with the phone's echo canceller and noise suppressor where it has them. The partner plays through an AudioTrack in the microphone's session, on the loudspeaker unless a headset takes the call.
- **The mic gate** stays as a backup for an echo canceller that misses. While the partner plays, and 0.3 s after, the microphone goes up as silence. A barge then sends its last 300 ms, as on the page.
- **Barge-in** is the page's detector, with the echo learned on the phone and kept between calls. A barge drops the AudioTrack's queue at once and sends `barge`.
- **Screen off.** The call runs in a foreground service of the microphone type. Its ongoing notification shows the state and the time, with Mute and Hang up. The call bar stays on every tab while the call runs.
- **Speed** works as on the page and is kept on the phone.
- **Stats** go up as on the page, with `path: "audiotrack"` and whether the echo canceller and noise suppressor run.

## The client socket

The socket carries PCM16 mono at 16 kHz both ways as binary frames, and JSON as text frames. It needs a login, and a cookie login needs one of the page's own origins.

The client sends the microphone without pause, silence included, since Phonic expects a steady stream. Text frames up:

- `{"type": "barge"}`: the client's own voice detection heard the user over the partner, and the client has cut its playback. The bridge then holds back the partner's audio until Phonic's own detection agrees, or for 1.5 s on a false alarm.
- `{"type": "stats", ...}`: the page's numbers for the server log, every 10 s and at hang-up. They hold the audio context's state and rate, the way out and any fallback, the seconds played and cut, barges, flushes and the echo estimate. With the stream they also hold its wait and stalls.
- `{"type": "end"}`: hang up.

Text frames down:

- `{"type": "state", "state": ...}`: `connecting`, `listening`, `speaking`, `reconnecting`, `ended`.
- `{"type": "flush"}`: drop the audio queued to play. It follows Phonic's own barge-in, about 1 s after the user starts speaking.
- `{"type": "saying", "text": ...}`: the partner's words that go with the audio just sent, for a live line.
- `{"type": "said", "who": "user" | "partner", "text": ...}`: a finished turn.
- `{"type": "asking", "request": ...}` and `{"type": "answered", "request": ...}`: an ask_seimei call and its answer.
- `{"type": "error", "text": ...}`.

Down audio is only what is heard. Phonic streams silent chunks nonstop, so the bridge forwards a chunk only while the partner speaks, or when it holds sound or words.

Each turn leaves a line in the server log. It holds the user's words and the time from their last speech to the partner's first sound. It also says when Phonic heard the end, and how long each lookup took. The line also names the `intelligence_level`. A reply that speaks seimei's answer gets a line too, and so do the page's barges and stats.

## What leaves the machine

Every call sends the pack, lookup answers, the audio and both transcripts to Phonic, and Phonic's upstream providers process them. Web searches go to OpenRouter and its search engine, and `fetch_page` reaches the page's site.

All of it goes as it is: nothing is scrubbed.

Phonic keeps conversations under its default retention; zero retention is for its Enterprise plan only.

The guards, all in the bridge:

- **File reads.** `read_file` and `search_files` reach only `[voice] roots`, the memory folder and `notes.md`. They never read seimei's state folders, so no login, upload or audit log. Nor do they read a denied path, a key file, a hidden file or folder, or a file whose name says it holds a secret. Symlinks are resolved before the check.
- **No local pages.** `fetch_page` takes any http or https address, except ones on this machine or a private network, checked on every redirect.
- **The audit log.** Each pack, with its section sizes, and each tool call of the partner's, with its query, URL or path, goes to `audit.jsonl` as `who: "voice"`.

## Tests

`tests/fakes/phonic.py` is a fake conversation socket that plays a script and logs what the bridge sends. `crates/seimei/tests/it/voice.rs` runs a whole call against it, with the fake agent CLI as the dispatcher and a fake search endpoint.

- A script is a JSON list of conversations, each a list of steps.
- `expect` waits for a matching message, `send` sends an event, and `sleep`, `close` and `drop` do what they say.
- An `audio_chunk` to send with `"pcm": "loud"` or `"silent"` gets 20 ms of that audio.
- `tests/cases/voice.json` holds the pack, name-matching and guard cases.
