# Direct voice mode

In direct voice mode the user talks with the dispatcher itself, and its reply streams back as speech sentence by sentence. A small fast model guesses an opener and the lookups while the user talks.

It replaces the Phonic partner of [voice.md](voice.md). `[voice] engine = "direct"` turns it on; Phonic stays the default until direct mode works on every client.

## The pipeline

Times count from the moment the user stops talking. They were measured through OpenRouter in November 2026.

```
mic --PCM 16 kHz over /api/voice/ws--> server VAD --clause--> MAI-Transcribe-2 --text--> pre-think (opener, lookups)
                                            |                                                   |
                                            +--end of turn--> [voice call] message --> dispatcher --> deltas
                                                                                                      |
client <--PCM-- HeyGen Voice Turbo, one sentence a request <-- sentence splitter <-------------------+
```

| stage | what | measured |
|---|---|---|
| end of turn | 300 ms of silence | 0.3 s |
| reaction | the opener, or a short "Okay." | 0.05 s after the end |
| final text | MAI-Transcribe-2 on the last clause | 0.42 to 0.79 s |
| dispatcher | the first text of its reply | 2 to 4 s, live |
| first sound | HeyGen Voice Turbo on the first sentence | 0.6 to 0.9 s more |

- **The reaction lands on target.** An opener or an acknowledgement plays as the turn ends, so the user hears seimei within half a second.
- **The answer takes about 3 to 5 s.** The dispatcher's own first words set it. The QA server's fake dispatcher answers at once, and there the first reply sound came 0.55 to 1.77 s after the end of speech.
- **The server logs each turn** as `voice: direct turn:` with these stages, the recognition times and the pre-think times.

## Speech to text: pseudo-streaming MAI

The dictation path stays: MAI-Transcribe-2 through `/api/asr`'s code, with the same word list and fixer. It is made pseudo-streaming by cutting at pauses.

- **VAD on the server.** The client streams the microphone nonstop over the voice socket, so one cutter serves every client. It is `web/phrases.js` ported to Rust in `voice/vad.rs`.
- **A pause is a stable point.** A clause ends at about 240 ms of silence, and its audio goes to MAI at once. A cut in silence splits no word, so the partials join end to end.
- **The end of the turn** comes at about 300 ms of silence. The last clause went out at 240 ms, so its text lands about 0.5 s after the user stops.
- **A clause without a pause** goes to MAI as a window at 1.2 s, then every 2 s. A window's text feeds pre-think only.
- **While seimei speaks**, the server keeps only the last 500 ms of the microphone, since it may be seimei's echo. A `barge` from the client turns that pre-roll into the start of a turn.

A minute of speech makes 15 to 30 clause requests, plus the windows. MAI costs $0.11 per hour of audio, so about $0.17 per hour of speech with pre-roll and windows.

## Pre-think and the opener

On each new clause or window, a fast model reads the words so far, at most once every 2 s. It returns JSON with an opener and the lookups to run.

```json
{"opener": "hm, let me look at the board", "lookups": [{"tool": "board", "query": "voice"}]}
```

- **The opener** acknowledges or stalls in at most 10 words. It never states a fact, a number or a promise. It names a topic only when that topic matches something seimei knows, since the words may be misheard.
- **Its audio** is rendered as soon as the guess comes, and plays when the turn ends. A window, pre-think and the voice take about 2.5 s, so only turns of about 3.5 s or more get one.
- **Shorter turns** get "Mm-hm.", "Okay." or "Yeah.", rendered once when the call starts.
- **The lookups** are the read-only ones of `voice/lookups.rs`. Their answers ride with the message, so the dispatcher seldom needs a tool call of its own.
- **The prompt** holds the open board, the shikigami, the sessions and the last 20 chat lines: about 2.4k tokens.

The model is `mistralai/mistral-small-3.2-24b-instruct` on OpenRouter, with the dictation's key. It answers in 0.75 to 1.15 s over a warm connection. The prompt's size changes that by about 0.1 s.

| model | how | first token |
|---|---|---|
| mistral-small-3.2 | OpenRouter | 0.59 s, p90 0.91 s |
| ministral-8b | OpenRouter | 0.58 s |
| haiku | a warm, stripped Claude Code process | 1.36 s, p90 1.69 s |

- **Haiku is too slow for this.** It took 0.77 s longer to the first token, even with no tools, MCP servers or settings loaded.
- **OpenRouter can refuse US models by region** with a 403 for the provider's terms: Anthropic, OpenAI and Google alike.
- **The cost** is about $0.001 a turn.

## How a turn reaches the dispatcher and comes back

The dispatcher runs as one long agent session, and `Turns::delta` already flashes its reply as `delta` events while it is written.

- **The message.** At the end of the turn the call sends `[voice call] <the words>` through `Dispatcher::send_spoken`. Notes follow the words: the opener already said, where the user cut seimei off, and the lookups' answers.
- **The thread.** The user's words are a `user` event with `voice` and `spoken` set. The reply is the usual `assistant` event answering it.
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

## Barge-in

The client keeps its own barge detector, the one in `web/pcm.js` and the app. The echo canceller in the app works under it.

- **The client cuts at once.** It drops its queued audio and sends `barge`.
- **The server stops speaking.** It drops the pieces still to render and opens the microphone, with its last 500 ms as the start of the turn.
- **The dispatcher hears it.** The next message says what the user heard before the cut. The running turn goes on, since it may be doing work.

## Config

Four keys under `[voice]` set direct mode.

| key | default | what |
|---|---|---|
| `engine` | `"phonic"` | `"direct"` for this mode |
| `tts_model` | `"heygen/heygen-voice-1-turbo"` | the speech model on OpenRouter |
| `tts_voice` | `"bram"` | its voice |
| `think_model` | `"mistralai/mistral-small-3.2-24b-instruct"` | the pre-think model |

The key and `zdr` come from `[asr]`, so direct mode needs dictation set up.

## Risks

Most risks are about speed and about services outside seimei.

- **A 300 ms end cuts thinking pauses.** The rest of the speech then becomes the next message. If that happens often, the end can wait longer after a clause with no full stop.
- **The dispatcher may be busy.** A voice message during another turn folds into it, and the answer waits for that turn's next step.
- **The dispatcher's speed is the floor.** Its 2 to 4 s to the first word set the answer's time.
- **OpenRouter's routing.** Pre-think needs Mistral and speech needs HeyGen on OpenRouter. If either goes, voice goes with it.
- **Echo.** The server holds the microphone while seimei speaks, so it relies on the client's barge to hear the user over it.
- **Russian.** MAI runs with `[asr] language`. A call in Russian needs that language per call, or detection.

## What leaves the machine

Each call sends audio and text to three providers through OpenRouter. `[asr] zdr` asks for providers that keep no data, for all three.

- **The audio** of each clause and window goes to Microsoft, with the word list.
- **The words so far**, the board, the shikigami and the last chat lines go to Mistral.
- **Every spoken piece** goes to HeyGen.
