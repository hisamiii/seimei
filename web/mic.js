// Dictation in the composer (docs/asr.md): tap the mic, talk, tap the square; the text lands at the cursor.
// seimei transcribes it with the open chat's words; nothing is sent on its own.
// Each phrase goes to the server as soon as a pause ends it (phrases.js), so only the last one is left to wait for at the square.
// The texts come in as they will, and go into the composer in the order they were said.
import { $, post, snack, say, toLogin } from './util.js';
import { Phrases } from './phrases.js';

const input = $('input'), field = input.closest('.field'), mic = $('mic'), drop = $('mic-drop');
const level = $('mic-level'), time = $('mic-time');
const BARS = 24;
// A whole dictation at most, and how long it listens without a phrase.
const MAX_MS = 600000, IDLE_MS = 60000;
// A phrase not back by then goes again, and the first answer wins: one call in about 25 takes seconds upstream.
const SLOW_MS = 6000;
let rec = null, busy = false, dictated = '';

for (let i = 0; i < BARS; i++) level.append(document.createElement('i'));

function show(mode) {
  const recording = mode === 'rec';
  field.classList.toggle('rec', recording);
  $('attach').hidden = recording;
  input.hidden = recording;
  drop.hidden = level.hidden = time.hidden = !recording;
  mic.classList.toggle('on', recording);
  mic.classList.toggle('busy', mode === 'busy');
  mic.disabled = mode === 'busy';
  mic.setAttribute('aria-label', recording ? 'Stop and transcribe' : mode === 'busy' ? 'Transcribing' : 'Dictate');
}

async function start() {
  if (busy) return;
  let stream;
  try {
    stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
  } catch {
    snack('Allow the microphone to dictate');
    return;
  }
  const ctx = new AudioContext();
  const src = ctx.createMediaStreamSource(stream);
  const an = ctx.createAnalyser();
  an.fftSize = 1024;
  src.connect(an);
  // The raw samples, in 20 ms frames for the phrase cutter; it runs only while connected, and writes silence.
  const proc = ctx.createScriptProcessor(2048, 1, 1);
  src.connect(proc);
  proc.connect(ctx.destination);
  const s = {
    stream, ctx, an, proc, t0: performance.now(), heard: performance.now(), levels: Array(BARS).fill(0),
    rate: ctx.sampleRate, per: Math.round(ctx.sampleRate / 50), carry: new Float32Array(0),
    recording: true, slots: new Map(), next: 0, failed: [],
    // Where the text goes: the cursor when the mic was tapped, then after each phrase.
    at: input.selectionEnd ?? input.value.length,
  };
  s.cut = new Phrases({ send: (seq, frames) => send(s, seq, frames), cancel: (seq) => cancel(s, seq) });
  proc.onaudioprocess = (e) => frames(s, e.inputBuffer.getChannelData(0));
  rec = s;
  s.cap = setTimeout(() => finish(true), MAX_MS);
  show('rec');
  tick();
}

function frames(s, x) {
  if (rec !== s) return;
  const buf = new Float32Array(s.carry.length + x.length);
  buf.set(s.carry);
  buf.set(x, s.carry.length);
  let i = 0;
  for (; i + s.per <= buf.length; i += s.per) s.cut.push(buf.slice(i, i + s.per));
  s.carry = buf.slice(i);
  const now = performance.now();
  if (s.cut.phrase) s.heard = now;
  else if (now - s.heard > IDLE_MS) finish(true);
}

function tick() {
  if (!rec) return;
  const buf = new Float32Array(rec.an.fftSize);
  rec.an.getFloatTimeDomainData(buf);
  const rms = Math.sqrt(buf.reduce((s, x) => s + x * x, 0) / buf.length);
  // -60 dBFS is silence, -10 is loud speech
  rec.levels = [...rec.levels.slice(1), Math.min(1, Math.max(0, (20 * Math.log10(rms + 1e-9) + 60) / 50))];
  rec.levels.forEach((l, i) => { level.children[i].style.height = `${4 + 26 * l}px`; });
  const sec = Math.floor((performance.now() - rec.t0) / 1000);
  time.textContent = `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, '0')}`;
  setTimeout(tick, 80);
}

// The square (keep) or x: the recording ends; with keep, the last phrase goes and the texts land as they come.
async function finish(keep) {
  const s = rec;
  if (!s) return;
  rec = null;
  clearTimeout(s.cap);
  s.proc.onaudioprocess = null;
  s.proc.disconnect();
  s.stream.getTracks().forEach((t) => t.stop());
  s.ctx.close();
  s.recording = false;
  if (!keep) {
    s.dropped = true;
    return show('idle');
  }
  busy = true;
  show('busy');
  s.cut.end();
  flush(s);
}

function send(s, seq, frames) {
  const slot = { done: false, cancelled: false, shown: false, text: '', wav: null };
  s.slots.set(seq, slot);
  (async () => {
    const pcm = new Float32Array(frames.length * s.per);
    frames.forEach((f, i) => pcm.set(f, i * s.per));
    try {
      slot.wav = await toWav(pcm, s.rate);
      slot.text = await ask(slot.wav, s);
    } catch {
      slot.failed = true;
    }
    slot.done = true;
    flush(s);
  })();
}

// Speech went on after a phrase was sent early: its text is dropped, unless it is in the composer already.
function cancel(s, seq) {
  const slot = s.slots.get(seq);
  if (!slot || slot.shown) return false;
  slot.cancelled = true;
  return true;
}

// The texts in the order they were said: one that came back early waits for those before it.
// While the mic records the composer is hidden, so they wait for the square; x drops them.
function flush(s) {
  if (s.recording || s.dropped) return;
  while (s.slots.has(s.next)) {
    const slot = s.slots.get(s.next);
    if (!slot.cancelled) {
      if (!slot.done) return;
      slot.shown = true;
      if (slot.failed) s.failed.push(slot.wav);
      else if (slot.text) insert(slot.text, s);
    }
    s.next++;
  }
  if (s.next < s.cut.seq) return;
  busy = false;
  show('idle');
  const lost = s.failed.filter(Boolean);
  if (lost.length) snack(lost.length > 1 ? `Couldn't transcribe ${lost.length} phrases` : "Couldn't transcribe", { label: 'Retry', run: () => retry(lost) });
  else if (s.failed.length) snack("Couldn't read the recording");
}

async function retry(wavs) {
  if (busy || rec) return;
  busy = true;
  show('busy');
  const s = { at: input.selectionEnd ?? input.value.length };
  const lost = [];
  for (const w of wavs) {
    try {
      const text = await ask(w, s);
      if (text) insert(text, s);
    } catch {
      lost.push(w);
    }
  }
  busy = false;
  show('idle');
  if (lost.length) snack("Couldn't transcribe", { label: 'Retry', run: () => retry(lost) });
}

// A phrase, resampled to 16 kHz mono, as s16 PCM in a WAV header.
async function toWav(samples, rate) {
  const buf = new AudioBuffer({ length: samples.length, numberOfChannels: 1, sampleRate: rate });
  buf.copyToChannel(samples, 0);
  const n = Math.ceil(buf.duration * 16000);
  const off = new OfflineAudioContext(1, n, 16000);
  const src = off.createBufferSource();
  src.buffer = buf;
  src.connect(off.destination);
  src.start();
  const pcm = (await off.startRendering()).getChannelData(0);
  const v = new DataView(new ArrayBuffer(44 + n * 2));
  const str = (at, t) => [...t].forEach((c, i) => v.setUint8(at + i, c.charCodeAt(0)));
  str(0, 'RIFF'); v.setUint32(4, 36 + n * 2, true); str(8, 'WAVE');
  str(12, 'fmt '); v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true);
  v.setUint32(24, 16000, true); v.setUint32(28, 32000, true); v.setUint16(32, 2, true); v.setUint16(34, 16, true);
  str(36, 'data'); v.setUint32(40, n * 2, true);
  for (let i = 0; i < n; i++) v.setInt16(44 + i * 2, Math.max(-1, Math.min(1, pcm[i])) * 32767, true);
  return v.buffer;
}

// One phrase's text; the draft so far (with what this dictation added) is its context.
async function ask(wav, s) {
  const draft = input.value.slice(0, s.at).slice(-2000);
  const hint = { client: 'web', chat: 'seimei', screen: 'chat', draft };
  // A header is ASCII: anything else in the draft goes as a JSON escape.
  const h = JSON.stringify(hint).replace(/[\u0080-\uffff]/g, (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`);
  const go = async () => {
    const r = await fetch('api/asr', { method: 'POST', headers: { 'Content-Type': 'audio/wav', 'X-Seimei-Asr': h }, body: wav });
    if (r.status === 401) toLogin();
    if (!r.ok) throw new Error(`${r.status}`);
    return ((await r.json()).text || '').trim();
  };
  const first = go();
  const late = new Promise((ok) => { setTimeout(ok, SLOW_MS, null); });
  const r = await Promise.race([first.then((text) => ({ text }), () => ({ failed: true })), late]);
  if (r?.text !== undefined) return r.text;
  // Failed: once more. Slow: once more alongside, and the first text wins.
  return r ? go() : Promise.any([first, go()]);
}

function insert(text, s) {
  const at = Math.min(s.at, input.value.length);
  const before = input.value.slice(0, at), after = input.value.slice(at);
  const lead = before && !/\s$/.test(before) ? ' ' : '', trail = after && !/^\s/.test(after) ? ' ' : '';
  input.value = before + lead + text + trail + after;
  s.at = at + lead.length + text.length;
  input.focus();
  input.setSelectionRange(s.at, s.at);
  input.dispatchEvent(new Event('input'));
  dictated = `${dictated} ${text}`.trim();
  field.classList.add('fresh');
  clearTimeout(insert.fade);
  insert.fade = setTimeout(() => field.classList.remove('fresh'), 2000);
  say(`Dictated: ${text}`);
}

// A message went out: if part of it was dictated, the server compares what the user changed.
export function sentDictated(text) {
  if (!dictated) return;
  const said = dictated;
  dictated = '';
  if (said !== text.trim()) post('api/asr/edit', { dictated: said, sent: text }).catch(() => {});
}

mic.addEventListener('click', () => (rec ? finish(true) : start()));
drop.addEventListener('click', () => finish(false));

// The mic shows when the server has a key and the browser can record (https or localhost).
if (window.isSecureContext && navigator.mediaDevices?.getUserMedia && window.AudioContext) {
  fetch('api/asr').then((r) => (r.ok ? r.json() : {})).then((j) => { mic.hidden = !j.enabled; }).catch(() => {});
}
