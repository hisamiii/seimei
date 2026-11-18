# Direct voice mode

In direct voice mode the user talks with the dispatcher itself, and its reply streams back as speech sentence by sentence. The first sound the user hears is that reply: no filler plays before it.

It replaces the Phonic partner of [voice.md](voice.md). `[voice] engine = "direct"` turns it on; Phonic stays the default until direct mode works on every client.

## The pipeline

Times count from the moment the user stops talking. They were measured through OpenRouter in November 2026.

```
mic --PCM 16 kHz over /api/voice/ws--> server VAD --clause--> MAI-Transcribe-2 --text--> pre-think (lookups)
                                            |                                                   |
                                            +--end of turn--> [voice call] message --> dispatcher --> deltas
                                                                                                      |
client <--PCM-- HeyGen Voice Turbo, one sentence a request <-- sentence splitter <-------------------+
                (HeyGen's own API, or OpenRouter)
```

| stage | what | measured |
|---|---|---|
| end of turn | 400 ms of silence, 2.5 s when words trail off | 0.4 s |
| final text | MAI-Transcribe-2 on the last clause | 0.42 to 0.82 s |
| dispatcher | the first text of its reply | 2 to 4 s, live |
| first sound | HeyGen Voice Turbo on the first sentence | 1.3 to 1.7 s more |

- **The first sound is the answer.** It comes at about 4 to 6 s with the live dispatcher, which sets most of it.
- **On the QA server** the fake dispatcher answers at once. There the first reply sound came 1.76 to 2.32 s after the end of speech, in 4 turns.
- **Speech is the next biggest stage.** The first piece is a sentence, or a clause of 6 words, rendered whole before it plays.
- **The server logs each turn** as `voice: direct turn:` with these stages, the recognition times and the pre-think times.

## Speech to text: pseudo-streaming MAI

The dictation path stays: MAI-Transcribe-2 through `/api/asr`'s code, with the same word list and fixer. It is made pseudo-streaming by cutting at pauses.

- **VAD on the server.** The client streams the microphone nonstop over the voice socket, so one cutter serves every client. It is `web/phrases.js` ported to Rust in `voice/vad.rs`.
- **A pause is a stable point.** A clause ends at about 240 ms of silence, and its audio goes to MAI at once. A cut in silence splits no word, so the partials join end to end.
- **The end of the turn** waits for the words. At 300 ms of silence the cutter marks a pause. Once the last clause's text is in, about 0.5 s after the user stops, the call decides how long the pause must be:
  - **finished words** end the turn after `end_finished_ms`, 400 ms. They end in `.`, `?` or `!`, and their last word does not lead on;
  - **words that trail off** wait up to `end_trailing_ms`, 2.5 s. They end on a comma, an ellipsis or no stop. Or their last word is one such as "and", "but", "so", "like" or "um", or one of their Russian peers;
  - **speech in the pause** goes on in the same turn. A filler such as "um" is speech, so it starts the wait again, and its words trail off.
- **Far voices** open no turn. The call learns the user's level from the turns it sends. A turn's level is the 80th percentile of its voice frames, and the user's is the quietest of their last 5 turns. So a watch at rest, 10 to 15 dB quieter than the phone, still counts once it spoke.
- **The gate.** Voice must reach the user's level less `near_db`, 15 dB, and never less than `floor_db`, -60 dBFS. A clause quieter than that is dropped unheard, and the log says so.
- **Fillers alone** make no turn: "Mm. Um." goes nowhere.
- **A clause without a pause** goes to MAI as a window at 1.2 s, then every 2 s. A window's text feeds pre-think only, so its lookups start before the user stops.
- **While seimei speaks**, the server keeps only the last 500 ms of the microphone, since it may be seimei's echo. A `barge` from the client turns that pre-roll into the start of a turn.

A minute of speech makes 15 to 30 clause requests, plus the windows. MAI costs $0.11 per hour of audio, so about $0.17 per hour of speech with pre-roll and windows.

## Pre-think

On each new clause or window, a fast model reads the words so far, at most once every 2 s. It returns the lookups to run as JSON.

```json
{"lookups": [{"tool": "board", "query": "voice"}]}
```

- **The lookups** are the read-only ones of `voice/lookups.rs`. Their answers ride with the message, so the dispatcher seldom needs a tool call of its own.
- **The message never waits for them.** Lookups that finish after the turn ends are dropped.
- **The prompt** holds the open board, the shikigami, the sessions and the last 20 chat lines: about 2.4k tokens.

The model is `mistralai/mistral-small-3.2-24b-instruct` on OpenRouter, with the dictation's key. One provider serves it, and its time varies by the hour.

| when | median | range |
|---|---|---|
| morning, warm connection | 0.93 s | 0.75 to 1.15 s |
| evening, warm connection | 3.45 s | 2.58 to 3.87 s |

- **Why not haiku.** Through a warm, stripped Claude Code process it took 1.36 s to the first token, against 0.59 s for Mistral then.
- **OpenRouter can refuse US models by region** with a 403 for the provider's terms: Anthropic, OpenAI and Google alike.
- **The cost** is about $0.001 a turn.

### No opener

An earlier build spoke a guessed opener or a canned acknowledgement as the turn ended. It was dropped: the first sound is the real reply.

## How a turn reaches the dispatcher and comes back

The dispatcher runs as one long agent session, and `Turns::delta` already flashes its reply as `delta` events while it is written.

- **The message.** At the end of the turn the call sends `[voice call] <the words>` through `Dispatcher::send_spoken`. Notes follow the words: where the user cut seimei off, and the lookups' answers.
- **The thread.** The user's words are a `user` event with `voice` and `spoken` set. The reply is the usual `assistant` event answering it. The chat shows both as plain messages with a small wave mark, and the call's end writes no summary.
- **The reply back.** Delta events carry `re`, the id of the message they answer. The call speaks the chat-lane deltas whose `re` is one of its own messages.
- **Sentence by sentence.** The splitter in `voice/speech.rs` cuts after a full stop, or after a clause of 6 words for the first piece. Two pieces render at a time, and they play in order.
- **News while the call is on.** A reply to a shikigami's or a timer's event is spoken whole, once the user is quiet.
- **The reply style.** The `[voice call]` line of `dispatcher.md` asks for the answer first, plain short sentences and no markdown. For a slow action it says "on it" and reports when done.
- **Effort stays as it is.** Changing effort per turn can cost the cached context, and low effort answered no faster.

### Why the turn starts only at the end

Starting the dispatcher's turn at the first pause, with later speech folded in, would save little.

- **A message sent mid-turn folds in at the next model step only**, after a tool call. A turn that answers the partial with text alone ends first.
- **Pre-think already does the lookups** that an early turn would run.

## Text to speech: HeyGen Voice Turbo on OpenRouter

HeyGen Voice Turbo through OpenRouter's `/audio/speech` is the pick: casual voices, clean Russian, and the dictation's key. It returns its PCM whole at 44.1 kHz, so each request is one piece, resampled to 16 kHz on the server.

| model | 2 words | 16 words | per hour of speech |
|---|---|---|---|
| HeyGen Voice Turbo | 0.69 to 0.89 s | 0.53 to 0.66 s | about $0.22 |
| HeyGen Voice | 1.6 to 2.1 s | 0.85 s | about $0.81 |
| ElevenLabs Flash v2.5 | 0.39 s | 0.46 s | about $2.20 |
| Fish Drama 3 | 0.4 to 0.8 s | n/a | about $0.55, later $2.70 |
| MAI-Voice-2.1 Flash | 1.18 s | n/a | about $0.80 |

- **How it was measured.** Each figure is a median over a warm connection. Russian was checked by running MAI-Transcribe-2 on the audio.
- **The voice** is `[voice] tts_voice`, `bram` by default. Any HeyGen library voice id works, and `tts_model` takes any OpenRouter speech model.
- **HeyGen's free variant** allows 50 requests a day, and a 10 minute call needs about 100.
- An hour of speech is about 54,000 characters.

### A voice from the user's HeyGen account

A voice designed or cloned in the user's HeyGen account speaks on HeyGen's own API: OpenRouter answers "Voice not found" for it. `[voice] heygen_voice` turns this on, with the key in `heygen_key_file`.

- **The request.** Each piece goes to `/v3/models/audio/tts/stream` as PCM at 16 kHz, read whole. The whole piece comes 0.1 to 0.4 s after its first audio, which came at 0.3 to 0.8 s.
- **The first sound.** On the QA server it came 1.48 s after the end of speech, against 1.76 to 2.32 s with bram on OpenRouter.
- **The fallback.** When HeyGen fails, `tts_voice` on OpenRouter speaks the piece, unless `tts_fallback = false`. HeyGen then rests for a while:
  - 10 minutes after a refusal, such as 402 for no credit;
  - the `Retry-After` time after a 429;
  - 30 s after a timeout, a 5xx or a dropped link. No first audio in 2 s counts as a timeout.
- **The log.** Each fallback logs `voice: heygen:` with HeyGen's answer, and each turn's `direct turn:` line says who spoke the first sound.
- **The models.** `heygen-voice-1-turbo` needs credit in the account's wallet. `heygen-voice-1-turbo-free` costs nothing until 2026-11-30, with 30 requests a minute and capacity shared by all its users.

## Languages

A call takes English and the `additional_languages` of `[voice]`, such as Russian.

- **Speech recognition.** MAI takes one language hint, and transcribes the other language under it too. The call's hint is `[asr] language` at first, then the language of the user's last turn. Detection without a hint did worse on short Russian: "okay, do it" came back half in Latin letters. The word list does not hurt Russian.
- **The message.** A turn mostly in Cyrillic is Russian, or the Cyrillic language of `additional_languages`. The `[voice call]` message then says which language the user spoke, so the dispatcher answers in it.
- **The voice.** HeyGen detects the language of each piece, and voice a speaks Russian. The splitter cuts Cyrillic sentences as it cuts English.

## Speed

The call bar's speed button sets how fast seimei talks, from 0.9 to 1.3. It is 1 by default, kept on the device and sent with the call as `?speed=`.

- **HeyGen ignores speed** for voices designed or cloned in an account. So the server stretches each piece in time at the same pitch, before it goes down. It overlaps 20 ms frames, each placed where it best goes on from the last.
- **A new speed counts from the next call.**

## Barge-in

The client keeps its own barge detector, the one in `web/pcm.js` and the app. The echo canceller in the app works under it.

- **The client cuts at once.** It drops its queued audio and sends `barge`.
- **The server stops speaking.** It drops the pieces still to render and opens the microphone, with its last 500 ms as the start of the turn.
- **The dispatcher hears it.** The next message says what the user heard before the cut. The running turn goes on, since it may be doing work.

### Speech that goes on from the last message

The user may go on after the turn went out: after a long pause, or over seimei's first words. Then the user and seimei get one answer, not two.

- **When it counts.** The reply to the last message has not come yet, and the user says real words: not fillers, not a far voice. A barge during that reply counts at once.
- **The reply stops.** What is still to say is dropped, the client flushes its audio, and the rest of that reply is not spoken.
- **One message completes the other.** The new words go out with a note: they go on from the last message, and the user heard so much of the reply.
- **The dispatcher answers once.** It folds the message into its running turn at the next step. Its reply from then on answers the new message, so that is what is spoken.

## Config

These keys under `[voice]` set direct mode.

| key | default | what |
|---|---|---|
| `engine` | `"phonic"` | `"direct"` for this mode |
| `tts_model` | `"heygen/heygen-voice-1-turbo"` | the speech model on OpenRouter |
| `tts_voice` | `"bram"` | its voice |
| `think_model` | `"mistralai/mistral-small-3.2-24b-instruct"` | the pre-think model, for the lookups |
| `heygen_voice` | `""` | a voice in the user's HeyGen account |
| `heygen_model` | `"heygen-voice-1-turbo"` | HeyGen's speech model |
| `heygen_key_file` | `"~/.config/heygen/api_key"` | the HeyGen API key |
| `tts_fallback` | `true` | `tts_voice` on OpenRouter speaks when HeyGen fails |
| `end_finished_ms` | `400` | the silence that ends a turn whose words sound finished |
| `end_trailing_ms` | `2500` | the silence that ends a turn whose words trail off |
| `near_db` | `15` | how far under the user's level speech is someone else's |
| `floor_db` | `-60` | the least level of the user's voice, in dBFS |

The key and `zdr` come from `[asr]`, so direct mode needs dictation set up.

## Risks

Most risks are about speed and about services outside seimei.

- **Speech recognition sets the stops.** MAI may end a fragment with a full stop, and the turn then ends at 400 ms. Speech after it goes on from that message, so the answer is still one.
- **The dispatcher may be busy.** A voice message during another turn folds into it, and the answer waits for that turn's next step.
- **The dispatcher's speed is the floor.** Its 2 to 4 s to the first word set the answer's time.
- **OpenRouter's routing.** Pre-think needs Mistral and speech needs HeyGen on OpenRouter, or HeyGen's own API. If either goes, voice goes with it.
- **Echo.** The server holds the microphone while seimei speaks, so it relies on the client's barge to hear the user over it.

## What leaves the machine

Each call sends audio and text to three providers through OpenRouter. `[asr] zdr` asks for providers that keep no data, for all three.

- **The audio** of each clause and window goes to Microsoft, with the word list.
- **The words so far**, the board, the shikigami and the last chat lines go to Mistral.
- **Every spoken piece** goes to HeyGen. With `heygen_voice` set it goes straight to HeyGen's API, where `zdr` does not apply.
