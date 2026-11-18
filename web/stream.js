// The partner's audio through an audio element (docs/voice.md, the stream way out): PCM16 as FLAC in MP4, appended to a MediaSource as it comes.
// Some browsers play nothing through Web Audio on some phones while a media element plays fine; this is the way out for them.
// It keeps a little silence queued so the element never stalls, and new speech goes in right after it.
import { init, segment, BLOCK } from './fmp4.js';

const TYPE = 'audio/mp4; codecs="flac"';
// Silence kept queued ahead of the playhead. With less, Firefox's element starves and its clock runs slow (0.7x at 0.12 s).
// A reply does not wait behind it: push(..., true) drops the silence from just past the playhead.
const LEAD = 0.25;
// After a cut, new audio starts this far past the playhead: what is decoded already plays out.
const CUT_GAP = 0.04;
// Played audio is dropped from the buffer once this much has piled up behind the playhead: an hour would be over 100 MB.
const KEEP_BEHIND = 30;

export const streamable = () => !!window.MediaSource?.isTypeSupported?.(TYPE);

export class Stream {
  constructor(rate) {
    this.rate = rate;
    this.el = new Audio();
    this.ms = new MediaSource();
    this.url = URL.createObjectURL(this.ms);
    this.el.src = this.url;
    this.sb = null;
    this.queue = [];
    // Samples in the SourceBuffer, and those plus the queued ones.
    this.sent = 0;
    this.at = 0;
    this.seq = 1;
    this.removeFrom = null;
    // The time from a reply's arrival to the playhead reaching it, for the debug panel and the log.
    this.waits = [];
    this.marks = [];
    this.stalls = 0;
    this.trimmed = 0;
    // The first wait is for the first data; only later ones are stalls.
    this.el.addEventListener('waiting', () => { if (this.el.currentTime > 0) this.stalls++; });
    this.ms.addEventListener('sourceopen', () => {
      this.sb = this.ms.addSourceBuffer(TYPE);
      this.sb.addEventListener('updateend', () => this.pump());
      this.sb.appendBuffer(init(rate));
    }, { once: true });
    this.timer = setInterval(() => this.keep(), 20);
  }

  /** Call in the tap: the browser lets the element play only then. */
  start() {
    return this.el.play();
  }

  now() {
    return this.el.currentTime;
  }

  /**
   * Queues PCM16 to play after what is queued; returns its [start, end] on the element's clock.
   * `fresh` marks a reply's first chunk: only silence is queued then, so it goes in just past the playhead.
   */
  push(pcm, fresh = false) {
    if (fresh) this.cut();
    const n = Math.ceil(pcm.length / BLOCK) * BLOCK;
    const whole = n === pcm.length ? pcm : (() => { const p = new Int16Array(n); p.set(pcm); return p; })();
    const start = this.at / this.rate;
    this.queue.push(whole);
    this.at += n;
    if (fresh) this.marks.push([start, performance.now()]);
    this.pump();
    return [start, this.at / this.rate];
  }

  // Silence up to LEAD ahead of the playhead, and the waits of replies the playhead has reached.
  keep() {
    const now = this.now();
    while (this.marks.length && this.marks[0][0] <= now && !this.el.paused) {
      this.waits.push(performance.now() - this.marks.shift()[1]);
      if (this.waits.length > 50) this.waits.shift();
    }
    const ahead = this.at / this.rate - now;
    if (ahead < LEAD) this.push(new Int16Array(Math.ceil(((LEAD - ahead) * this.rate) / BLOCK) * BLOCK));
  }

  pump() {
    const sb = this.sb;
    if (!sb || sb.updating) return;
    if (this.removeFrom !== null) {
      const from = this.removeFrom / this.rate;
      this.removeFrom = null;
      if (this.sent / this.rate > from) {
        this.sent = Math.round(from * this.rate);
        sb.remove(from, Infinity);
        return;
      }
    }
    const behind = this.now() - 5;
    if (behind - this.trimmed > KEEP_BEHIND) {
      sb.remove(this.trimmed, behind);
      this.trimmed = behind;
      return;
    }
    if (!this.queue.length) return;
    const n = this.queue.reduce((s, p) => s + p.length, 0);
    const pcm = new Int16Array(n);
    let k = 0;
    for (const p of this.queue) {
      pcm.set(p, k);
      k += p.length;
    }
    this.queue = [];
    try {
      sb.appendBuffer(segment(this.seq++, this.sent, pcm, this.rate));
      this.sent += n;
    } catch {
      // The buffer is full or the source closed: what was queued is dropped, the clock goes on.
      this.at = this.sent;
    }
  }

  /** Drops what is queued past the playhead; new audio goes in just after it. */
  cut() {
    const from = Math.ceil(((this.now() + CUT_GAP) * this.rate) / BLOCK) * BLOCK;
    this.queue = [];
    this.marks = [];
    if (this.sent > from) this.removeFrom = from;
    this.at = Math.min(this.sent, from);
    this.pump();
  }

  /** The median wait from a reply's arrival to its first sample playing, in ms. */
  wait() {
    if (!this.waits.length) return null;
    const s = [...this.waits].sort((a, b) => a - b);
    return Math.round(s[Math.floor(s.length / 2)]);
  }

  close() {
    clearInterval(this.timer);
    this.el.pause();
    this.el.removeAttribute('src');
    this.el.load();
    URL.revokeObjectURL(this.url);
  }
}
