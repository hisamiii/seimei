# Dictation

The mic in the composer of the app and the page turns speech into text with a speech model on OpenRouter. The text lands in the composer for you to edit, and nothing is sent until you send it.

The model is biased toward the words of what you work on: the open chat, the agents and the board. The keys are in [config.md](config.md), under `[asr]`.

## How it works

seimei builds a word list from live context, sends it with the audio, and then fixes misheard words against that list. Only terms that were in the list can come in.

### Input

Two routes take a 16 kHz mono WAV of up to 2 minutes. `POST /api/asr` serves the app and the page with the usual login, and `~/.local/share/seimei/asr.sock` serves a local dictation tool.

The socket has mode 600 and needs no login.

The `X-Seimei-Asr` header carries a hint as JSON, and every field is optional:

- `chat`: `seimei`, `w:<worker id>` or `s:<session pid>`.
- `window`: `{app_id, title, cwd}`, the focused window on the server.
- `screen`: where the phone is.
- `draft`: the text already in the composer.

### The word list

seimei ranks the words for each request and caps the list at `vocab_max`, 100 by default. The sources come in this order:

1. Pins in `asr/words.txt`, spelled as they should come out.
2. Rare words from the open chat's last 20 messages, and its agent's name and project.
3. Live names: shikigami, sessions, projects and repos.
4. Rare words from open board items.
5. The focused window or the draft. For a window that is its title, and the identifiers of the repo a terminal is in.
6. The auto list: rare words from your own recent turns and shell history, rebuilt at night.

A word is rare when it is not English, has 3 to 30 characters, and is not a hash or a file extension. English means hunspell en_US and the 20k most common words, downloaded once into `asr/`. Words merge case-insensitively, and the denylist goes last and always wins.

### Transcription

The audio goes to `microsoft/mai-transcribe-2`, with the list as Azure's `phraseList`. On failure it goes to `mistralai/voxtral-mini-transcribe-3-0`, with the list as `context_bias` and one short description.

The description reads "Dictation to seimei about <project> with <agent names>". A 400 is tried again without the list. "Thank you." and the like on a clip under 2 s are dropped as noise.

### The fixer

The text first goes through `asr/fixes`, with `misheard -> right` lines on whole words. Then a run of up to 4 words that is close to a term of this request's list becomes that term.

- Close means a letter similarity of 0.8, or the same consonant key with a similarity of at least 0.6.
- The bar is 0.9 for a run of English words, and 0.85 when the run holds a known word.
- An English word on its own is never replaced.
- A fixed term at a sentence start gets a capital.

### Reply and logs

The reply is `{text, raw, fixed: [[from, to]], model, ms, cost}`. Each replacement, and each suspect word left as it was, goes to `asr/misses.log` as material for new pins and fixes.

When you edit dictated text before you send it, the app reports the edit. The changed words go to `misses.log` as candidates, and nothing becomes a fix until you add it.

The socket's `/context` returns the list a hint would get, and what the fixer makes of a given text. Use it to check what goes out.

## Privacy

Only terms go out, never chat text. Terms can still name people and clients, so you can hold some back.

- `[asr] deny` holds words and `*` globs that are never sent.
- `[asr.sources]` turns each source off: chat, names, board, window, draft and auto. With all of them off, only your pins go.
- `[asr] zdr = true` asks OpenRouter for providers that keep no data. Both models have such an endpoint.

`asr/sent.jsonl` has one line per request. It holds the time, the client, the model, every term sent with its source, the description, the audio length and the cost. It holds no audio and no transcript, and it rotates at 10 MB.

## In the clients

Tap the mic to record, the square to stop, and x to drop the recording. The text lands at the cursor and is marked for 2 s.

The clients send the speech phrase by phrase, as the laptop's dictate tool does. A pause ends a phrase, and that phrase goes to `/api/asr` while you go on talking. At the square only the last phrase is left to wait for.

- A phrase ends at a 400 ms pause or after 15 seconds. It goes out 250 ms into the pause, and is dropped if you go on talking.
- Voice is the level over 100 ms, 8 dB above the room's noise floor (the 5th percentile of the last 10 s). A phrase keeps 500 ms from before it starts, and one with under 300 ms of voice is dropped.
- Each phrase gets the word list of its own request, with the draft so far as context.
- The texts land in the order you said them, whatever order they come back in. While recording, the draft stays in view above a slim bar, and each text shows in it as it comes. The newest is tinted, and dots show while a phrase is on its way. x drops them all.
- A phrase with no answer in 6 s goes again, and the first answer wins.
- The edit report sees the whole dictated text.

Recording stops by itself after 10 minutes, or after 60 s without a phrase. If a phrase fails, a snackbar offers Retry for it. The app asks for the microphone permission on the first tap.

The cutter is `web/phrases.js` on the page and `Phrases.kt` in the app, with the same numbers.
