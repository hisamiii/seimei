// Cuts dictation into phrases at pauses (docs/asr.md), the way the laptop's dictate tool does, so each phrase is transcribed while the next is spoken.
// An energy VAD: the level, smoothed over 100 ms, against the room's noise floor (the 5th percentile of the last 10 s).
// No DOM here: mic.js feeds it 20 ms frames, and node runs it on a WAV in the tests.

export const CUT = {
  frameMs: 20,
  // A pause this long ends a phrase; at specMs into it the phrase is sent already.
  silenceMs: 400,
  specMs: 250,
  maxPhraseMs: 15000,
  // Phrases with less voice are dropped: a cough comes back as "Thank you."
  minVoiceMs: 300,
  prerollMs: 500,
  // Voice starts this far above the floor and holds down to holdDb less.
  marginDb: 8,
  holdDb: 6,
  minDb: -62,
  // The mic pops when it opens.
  warmupMs: 400,
};

/** A frame's level in dBFS, from samples in -1..1. */
export function frameDb(f) {
  let s = 0;
  for (let i = 0; i < f.length; i++) s += f[i] * f[i];
  const rms = f.length ? Math.sqrt(s / f.length) : 0;
  return 20 * Math.log10(Math.max(rms, 1 / 32768));
}

class Vad {
  constructor(c) {
    this.c = c;
    this.n = 0;
    this.powers = [];
    this.history = [];
  }

  voice(f, inPhrase) {
    this.n++;
    const db = frameDb(f);
    if (this.n * this.c.frameMs <= this.c.warmupMs) return false;
    this.powers.push(10 ** (db / 10));
    if (this.powers.length > 5) this.powers.shift();
    const level = 10 * Math.log10(this.powers.reduce((a, b) => a + b, 0) / this.powers.length);
    // Digital silence (a muted mic) says nothing about the room.
    if (db > -85) {
      this.history.push(level);
      if (this.history.length > 10000 / this.c.frameMs) this.history.shift();
    }
    const noise = this.history.length < 10 ? -60 : [...this.history].sort((a, b) => a - b)[Math.floor(this.history.length / 20)];
    const margin = this.c.marginDb - (inPhrase ? this.c.holdDb : 0);
    return level > Math.max(noise + margin, this.c.minDb);
  }
}

/**
 * Feed it frames with push(); it calls send(seq, frames) for each phrase and cancel(seq) when speech goes on after an early send.
 * cancel returns false when that phrase's text is in the draft already: what follows is then a phrase of its own.
 */
export class Phrases {
  constructor({ send, cancel }, c = CUT) {
    this.send = send;
    this.cancel = cancel;
    this.c = c;
    this.vad = new Vad(c);
    this.pre = [];
    this.phrase = null;
    this.voiced = 0;
    this.quiet = 0;
    this.onset = 0;
    this.early = null;
    this.seq = 0;
  }

  frames(ms) {
    return Math.round(ms / this.c.frameMs);
  }

  push(f) {
    const c = this.c;
    const v = this.vad.voice(f, !!this.phrase);
    if (!this.phrase) {
      this.pre.push(f);
      if (this.pre.length > this.frames(c.prerollMs)) this.pre.shift();
      this.onset = v ? this.onset + 1 : 0;
      // 60 ms of voice starts a phrase, with the audio from just before it.
      if (this.onset >= this.frames(60)) {
        this.phrase = this.pre;
        this.pre = [];
        this.voiced = this.onset;
        this.quiet = 0;
      }
      return;
    }
    this.phrase.push(f);
    if (v) {
      this.voiced++;
      if (this.quiet && this.early !== null) {
        // Speech went on after the early send: drop that text, or start anew from the pause when it is shown already.
        if (!this.cancel(this.early)) {
          this.phrase = this.phrase.slice(-(this.quiet + 1));
          this.voiced = 1;
        }
        this.early = null;
      }
      this.quiet = 0;
    } else {
      this.quiet++;
    }
    const enough = this.voiced * c.frameMs >= c.minVoiceMs;
    if (c.specMs && this.quiet === this.frames(c.specMs) && this.early === null && enough) {
      this.early = this.seq;
      this.send(this.seq++, this.phrase.slice());
    }
    if (this.quiet >= this.frames(c.silenceMs) || this.phrase.length >= this.frames(c.maxPhraseMs)) {
      if (this.early === null && enough) {
        // 200 ms of the pause stays with it.
        this.send(this.seq++, this.phrase.slice(0, this.phrase.length - Math.max(0, this.quiet - this.frames(200))));
      }
      this.phrase = null;
      this.voiced = this.quiet = this.onset = 0;
      this.early = null;
    }
  }

  /** Stopped: what is left of a phrase goes too. */
  end() {
    if (this.phrase && this.early === null && this.voiced * this.c.frameMs >= this.c.minVoiceMs) this.send(this.seq++, this.phrase);
    this.phrase = null;
  }
}
