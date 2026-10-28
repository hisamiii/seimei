# Dictation

The phone app and the web page have a mic in the composer. seimei transcribes what you say with a speech model on OpenRouter, biased toward the words of what you are working on (the open chat, the agents, the board), and puts the text in the composer for you to edit. Nothing is sent until you send it.

## How it works

- `POST /api/asr` (the app and the page, with the usual login) or the unix socket `~/.local/share/seimei/asr.sock` (mode 600, for a dictation tool on the same machine) take a 16 kHz mono WAV, up to 2 minutes. The `X-Seimei-Asr` header carries a hint as JSON, every field optional: `chat` (`seimei`, `w:<worker id>` or `s:<session pid>`), `window` (`{app_id, title, cwd}`, the focused window on the server), `screen` (where the phone is) and `draft` (the text already in the composer).
- seimei builds a word list for the request, ranked and capped at `vocab_max` (100):
  1. pins: `asr/words.txt`, spelled as they should come out;
  2. rare words from the open chat's last 20 messages, and its agent's name and project;
  3. live names: shikigami, sessions, projects, repos;
  4. rare words from open board items;
  5. the focused window (its title, and the identifiers of the repo a terminal is in) or the draft;
  6. the auto list: rare words from your own recent turns and shell history, rebuilt at night.
  A word is rare when it is not English (hunspell en_US and the 20k most common words, downloaded once into `asr/`), 3 to 30 characters, and not a hash or a file extension. Words are merged case-insensitively; the denylist goes last and always wins.
- The audio goes to `microsoft/mai-transcribe-2` with the list as Azure's `phraseList`; on failure, `mistralai/voxtral-mini-transcribe-3-0` with the list as `context_bias` and one short description ("Dictation to seimei about <project> with <agent names>"). A 400 is tried again without the list. "Thank you." and the like on a clip under 2 s are dropped as noise.
- The text then goes through a fixer: `asr/fixes` (`misheard -> right` lines, whole words) first, then a run of up to 4 words that is close to a term of this request's list becomes that term. Close means a letter similarity of 0.8 (0.9 for a run of English words, 0.85 when the run holds a known word), or the same consonant key with a similarity of at least 0.6. An English word on its own is never replaced, and only terms that were sent can come in. A fixed term at a sentence start gets a capital.
- The reply is `{text, raw, fixed: [[from, to]], model, ms, cost}`. Each replacement, and each suspect word left as it was, is logged to `asr/misses.log` as material for new pins and fixes. When you edit dictated text before sending it, the app reports the edit and the changed words go to misses.log as candidates; nothing becomes a fix until you add it.
- The socket's `/context` returns the list a hint would get, and what the fixer makes of a given text, for checking what goes out.

## Privacy

- Only terms go out, never chat text. They can still name people and clients: `[asr] deny` holds words and `*` globs that are never sent, and `[asr.sources]` turns each source (chat, names, board, window, draft, auto) off. With all of them off, only your pins go.
- `asr/sent.jsonl` has one line per request: time, client, model, every term sent with its source, the description, the audio length and the cost. No audio and no transcript. It rotates at 10 MB.
- `[asr] zdr = true` asks OpenRouter for providers that keep no data; both models have such an endpoint.

## Config

`[asr]` in config.toml: `key_file` (default `~/.config/seimei/openrouter-key`; without a key the mic is not shown), `model`, `fallback`, `language` (`en`), `vocab_max`, `zdr`, `deny`, and `[asr.sources]`. See [config.md](config.md).

## In the clients

Tap the mic to record, the square to stop, × to drop it; it stops by itself after 2 minutes. The text lands at the cursor and is marked for 2 s. If it fails, a snackbar offers Retry and the recording is kept until then. The app needs the microphone permission, asked on the first tap.
