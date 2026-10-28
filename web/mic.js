// Dictation in the composer (docs/asr.md): tap the mic, talk, tap the square; the text lands at the cursor.
// seimei transcribes it with the open chat's words; nothing is sent on its own.
// The browser records in its own format (MediaRecorder); the clip is resampled to the 16 kHz mono WAV the server takes.
import { $, post, snack, say, toLogin } from './util.js';

const input = $('input'), field = input.closest('.field'), mic = $('mic'), drop = $('mic-drop');
const level = $('mic-level'), time = $('mic-time');
const BARS = 24, MAX_MS = 120000;
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
  let stream;
  try {
    stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
  } catch {
    snack('Allow the microphone to dictate');
    return;
  }
  const ctx = new AudioContext();
  const an = ctx.createAnalyser();
  an.fftSize = 1024;
  ctx.createMediaStreamSource(stream).connect(an);
  const mr = new MediaRecorder(stream);
  const chunks = [];
  mr.ondataavailable = (e) => chunks.push(e.data);
  mr.start(250);
  rec = { stream, ctx, an, mr, chunks, t0: performance.now(), levels: Array(BARS).fill(0) };
  rec.cap = setTimeout(() => finish(true), MAX_MS);
  show('rec');
  tick();
}

function tick() {
  if (!rec) return;
  const buf = new Float32Array(rec.an.fftSize);
  rec.an.getFloatTimeDomainData(buf);
  const rms = Math.sqrt(buf.reduce((s, x) => s + x * x, 0) / buf.length);
  // -60 dBFS is silence, -10 is loud speech
  rec.levels = [...rec.levels.slice(1), Math.min(1, Math.max(0, (20 * Math.log10(rms + 1e-9) + 60) / 50))];
  rec.levels.forEach((l, i) => { level.children[i].style.height = `${4 + 26 * l}px`; });
  const s = Math.floor((performance.now() - rec.t0) / 1000);
  time.textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  setTimeout(tick, 80);
}

async function finish(keep) {
  const r = rec;
  if (!r) return;
  rec = null;
  clearTimeout(r.cap);
  const stopped = new Promise((ok) => { r.mr.onstop = ok; });
  r.mr.stop();
  await stopped;
  r.stream.getTracks().forEach((t) => t.stop());
  r.ctx.close();
  if (!keep) return show('idle');
  show('busy');
  let wav;
  try {
    wav = await toWav(new Blob(r.chunks, { type: r.mr.mimeType }));
  } catch {
    show('idle');
    return snack("Couldn't read the recording");
  }
  transcribe(wav);
}

// The clip, decoded and resampled to 16 kHz mono, as s16 PCM in a WAV header.
async function toWav(blob) {
  const dec = new AudioContext();
  const buf = await dec.decodeAudioData(await blob.arrayBuffer()).finally(() => dec.close());
  const n = Math.ceil(buf.duration * 16000);
  const off = new OfflineAudioContext(1, n, 16000);
  const src = off.createBufferSource();
  src.buffer = buf;
  src.connect(off.destination);
  src.start();
  const pcm = (await off.startRendering()).getChannelData(0);
  const v = new DataView(new ArrayBuffer(44 + n * 2));
  const str = (at, s) => [...s].forEach((c, i) => v.setUint8(at + i, c.charCodeAt(0)));
  str(0, 'RIFF'); v.setUint32(4, 36 + n * 2, true); str(8, 'WAVE');
  str(12, 'fmt '); v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true);
  v.setUint32(24, 16000, true); v.setUint32(28, 32000, true); v.setUint16(32, 2, true); v.setUint16(34, 16, true);
  str(36, 'data'); v.setUint32(40, n * 2, true);
  for (let i = 0; i < n; i++) v.setInt16(44 + i * 2, Math.max(-1, Math.min(1, pcm[i])) * 32767, true);
  return v.buffer;
}

async function transcribe(wav) {
  if (busy) return;
  busy = true;
  show('busy');
  const hint = { client: 'web', chat: 'seimei', screen: 'chat', draft: input.value.slice(-2000) };
  // A header is ASCII: anything else in the draft goes as a JSON escape.
  const h = JSON.stringify(hint).replace(/[\u0080-￿]/g, (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`);
  try {
    const r = await fetch('api/asr', { method: 'POST', headers: { 'Content-Type': 'audio/wav', 'X-Seimei-Asr': h }, body: wav });
    if (r.status === 401) toLogin();
    if (!r.ok) throw new Error(`${r.status}`);
    const text = ((await r.json()).text || '').trim();
    if (text) insert(text);
  } catch {
    snack("Couldn't transcribe", { label: 'Retry', run: () => transcribe(wav) });
  } finally {
    busy = false;
    show('idle');
  }
}

function insert(text) {
  const a = input.selectionStart ?? input.value.length, b = input.selectionEnd ?? a;
  const before = input.value.slice(0, a), after = input.value.slice(b);
  const lead = before && !/\s$/.test(before) ? ' ' : '', trail = after && !/^\s/.test(after) ? ' ' : '';
  input.value = before + lead + text + trail + after;
  const end = a + lead.length + text.length;
  input.focus();
  input.setSelectionRange(end, end);
  input.dispatchEvent(new Event('input'));
  dictated = `${dictated} ${text}`.trim();
  field.classList.add('fresh');
  setTimeout(() => field.classList.remove('fresh'), 2000);
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
if (window.isSecureContext && navigator.mediaDevices?.getUserMedia && window.MediaRecorder) {
  fetch('api/asr').then((r) => (r.ok ? r.json() : {})).then((j) => { mic.hidden = !j.enabled; }).catch(() => {});
}
