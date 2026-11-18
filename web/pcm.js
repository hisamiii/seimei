// Voice call audio without the DOM (docs/voice.md): the microphone down to 16 kHz PCM16, the partner's PCM16 back to samples, and the local barge-in.
// voice.js feeds it; node can run it on plain arrays.
import { Vad, CUT, frameDb } from './phrases.js';

export const RATE = 16000;
// 20 ms at 16 kHz: one frame up the socket.
export const FRAME = 320;

/** Takes samples at the microphone's rate and returns them at 16 kHz, each the mean of the input it spans. */
export class Down {
  constructor(from) {
    this.r = from / RATE;
    this.carry = new Float32Array(0);
    this.t = 0;
  }

  push(x) {
    const buf = new Float32Array(this.carry.length + x.length);
    buf.set(this.carry);
    buf.set(x, this.carry.length);
    const out = new Float32Array(Math.max(0, Math.floor((buf.length - this.t) / this.r)));
    let t = this.t, k = 0;
    for (; k < out.length; k++, t += this.r) {
      const a = Math.floor(t), b = Math.max(a + 1, Math.floor(t + this.r));
      let s = 0;
      for (let j = a; j < b; j++) s += buf[j];
      out[k] = s / (b - a);
    }
    const keep = Math.floor(t);
    this.carry = buf.slice(keep);
    this.t = t - keep;
    return out;
  }
}

export function toPcm16(f) {
  const out = new Int16Array(f.length);
  for (let i = 0; i < f.length; i++) out[i] = Math.max(-1, Math.min(1, f[i])) * 32767;
  return out;
}

export function fromPcm16(buf) {
  const s = new Int16Array(buf, 0, buf.byteLength >> 1);
  const out = new Float32Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s[i] / 32768;
  return out;
}

/** The mean square level of samples in -1..1, as 0..1 for the call's level bars: -60 dBFS is 0, -10 is 1. */
export function level(f) {
  let s = 0;
  for (let i = 0; i < f.length; i++) s += f[i] * f[i];
  const db = 10 * Math.log10(s / Math.max(1, f.length) + 1e-12);
  return Math.min(1, Math.max(0, (db + 60) / 50));
}

// The user talks over the partner: 120 ms of voice, above the room's noise by more than dictation needs and at least -42 dBFS.
export const BARGE = { ...CUT, marginDb: 10, minDb: -42, onsetMs: 120, echoDb: 6, echoStart: 10, echoFallDb: 15 };

/**
 * Feed it 16 kHz frames with the level of what plays now (dBFS, or null when nothing plays).
 * push() returns true once per stretch of the user's voice over the partner; talking() says whether the user talks now.
 * The partner's own sound comes back through the microphone when the echo canceller misses it, as it can for page audio.
 * So over the partner, voice must also beat the echo: the microphone's level against what plays, learned as the call goes.
 */
export class Barge {
  constructor(c = BARGE, echo = c.echoStart) {
    this.c = c;
    this.vad = new Vad(c);
    this.onset = 0;
    this.quiet = 0;
    this.fired = false;
    // The loudest microphone level over what played, in dB, held and let fall slowly.
    this.echo = echo;
  }

  push(f, playDb) {
    const c = this.c;
    let v = this.vad.voice(f, this.onset >= c.onsetMs / c.frameMs);
    if (playDb != null) {
      const over = frameDb(f) - playDb;
      if (over > this.echo + c.echoDb) {
        // Louder than the partner's echo can be: the user.
      } else {
        this.echo = Math.max(over, this.echo - c.echoFallDb * c.frameMs / 1000);
        v = false;
      }
    }
    this.onset = v ? this.onset + 1 : 0;
    this.quiet = v ? 0 : this.quiet + 1;
    // A pause re-arms it: the next stretch of voice can barge again.
    if (this.quiet * c.frameMs >= c.silenceMs) this.fired = false;
    if (playDb == null || this.fired || this.onset * c.frameMs < c.onsetMs) return false;
    this.fired = true;
    return true;
  }

  talking() {
    return this.onset * this.c.frameMs >= this.c.onsetMs;
  }
}
