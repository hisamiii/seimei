// Voice calls (docs/voice.md): the voice button in an empty composer starts a hands-free call with the voice partner over /api/voice/ws.
// The microphone goes up as 16 kHz PCM16 without pause, silence included; the partner's audio comes down and plays at once.
// The page cuts the partner off itself when the user talks over it, well before the partner's own detection does.
import { $, S, el, snack, say, toLogin } from './util.js';
import { render, toBottom, nearBottom } from './chat.js';
import { Down, Barge, BARGE, RATE, FRAME, toPcm16, fromPcm16, level } from './pcm.js';
import { frameDb } from './phrases.js';

const bar = $('callbar'), pill = $('call-pill'), who = $('call-who'), lvl = $('call-level'), time = $('call-time');
const mute = $('call-mute'), end = $('call-end'), thread = $('thread'), debug = $('call-debug'), readout = $('call-readout');
const BARS = 26;
// What the speaker has queued starts this far ahead, so a late chunk does not leave a gap.
const LEAD = 0.06;
// What played this long ago can still come back through the microphone: the output's latency plus the room.
const ECHO_S = 0.3;
// The echo learned on this phone, kept between calls (pcm.js Barge).
const ECHO_KEY = 'seimei.voiceEcho';
// The page's own numbers go to the server log this often, and at hang-up.
const STATS_MS = 10000;
// The debug panel stays open between calls once opened (a tap on the pill).
const DEBUG_KEY = 'seimei.voiceDebug';
// A phone may play nothing from one way out and fine from another: the panel's beeps try each, and the partner follows the last one tried.
const PATHS = { page: 'the page audio', element: 'an audio element', fresh: 'a new audio context' };
// What the last beeps did, newest last: the panel shows them, with the browser's error when it would not play.
const tried = [];
const LABEL = { connecting: 'Connecting…', listening: 'Listening', speaking: 'Speaking', reconnecting: 'Reconnecting…' };
let call = null;

for (let i = 0; i < BARS; i++) lvl.append(document.createElement('i'));

function show(on) {
  $('composer').hidden = on;
  bar.hidden = !on;
  S.onCall = on;
  render();
}

async function start() {
  if (call) return;
  // The context is made and resumed in the tap: a phone lets it play only then.
  const ctx = new AudioContext();
  ctx.resume().catch(() => {});
  let stream;
  try {
    stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true } });
  } catch {
    ctx.close();
    snack('Allow the microphone to talk');
    return;
  }
  const url = new URL('api/voice/ws', location.href);
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  const ws = new WebSocket(url);
  ws.binaryType = 'arraybuffer';
  const src = ctx.createMediaStreamSource(stream);
  const proc = ctx.createScriptProcessor(1024, 1, 1);
  const out = ctx.createGain();
  out.connect(ctx.destination);
  const c = {
    // Where the partner's audio goes: a context and the node to connect to (see PATHS).
    sink: { ctx, out, path: 'page' }, got: 0, peak: -Infinity, beeps: 0,
    ctx, stream, ws, src, proc, out, t0: performance.now(), down: new Down(ctx.sampleRate), barge: new Barge(BARGE, learned()),
    pending: new Float32Array(0), sources: new Set(), until: 0, muted: false, state: 'connecting',
    levels: Array(BARS).fill(0), mic: 0, heard: 0, live: null, lock: null,
    // What was scheduled to play, as [start, end, dBFS], for the echo check.
    played: [], stats: { played: 0, chunks: 0, barges: 0, cut: 0, flushes: 0 },
  };
  // A phone may suspend the context, at a call or a route change: the next tap brings it back.
  ctx.onstatechange = () => { if (call === c && ctx.state !== 'running') document.addEventListener('pointerdown', () => ctx.resume().catch(() => {}), { once: true }); };
  call = c;
  src.connect(proc);
  // A ScriptProcessor runs only while connected; it writes silence.
  proc.connect(ctx.destination);
  proc.onaudioprocess = (e) => up(c, e.inputBuffer.getChannelData(0));
  ws.onmessage = (m) => {
    if (typeof m.data === 'string') return message(c, JSON.parse(m.data));
    c.got++;
    play(c, m.data);
  };
  ws.onclose = (e) => {
    if (call !== c) return;
    if (e.code === 1006 && c.state === 'connecting') {
      // A refused upgrade: the login may be gone.
      fetch('api/voice').then((r) => { if (r.status === 401) toLogin(); }).catch(() => {});
    }
    hangUp(c.state === 'connecting' ? "Couldn't start the call" : '');
  };
  navigator.wakeLock?.request('screen').then((l) => { c.lock = l; }).catch(() => {});
  mute.classList.remove('on');
  show(true);
  label(c);
  tick();
}

// The microphone: 16 kHz frames up the socket, each one also through the barge-in detection.
function up(c, x) {
  if (call !== c) return;
  const s = c.down.push(x);
  const buf = new Float32Array(c.pending.length + s.length);
  buf.set(c.pending);
  buf.set(s, c.pending.length);
  let i = 0;
  for (; i + FRAME <= buf.length; i += FRAME) {
    const f = buf.subarray(i, i + FRAME);
    if (c.muted) f.fill(0);
    else if (c.barge.push(f, playLevel(c))) {
      c.stats.barges++;
      cut(c);
      send(c, { type: 'barge' });
    }
    c.mic = c.muted ? 0 : level(f);
    if (c.ws.readyState === WebSocket.OPEN) c.ws.send(toPcm16(f).buffer);
  }
  c.pending = buf.slice(i);
}

function send(c, msg) {
  if (c.ws.readyState === WebSocket.OPEN) c.ws.send(JSON.stringify(msg));
}

// The partner's audio, played back to back as it comes.
function play(c, data) {
  const f = fromPcm16(data);
  if (!f.length) return;
  const { ctx, out } = c.sink;
  const buf = ctx.createBuffer(1, f.length, RATE);
  buf.copyToChannel(f, 0);
  const node = ctx.createBufferSource();
  node.buffer = buf;
  node.connect(out);
  c.peak = 20 * Math.log10(Math.max(1 / 32768, f.reduce((m, x) => Math.max(m, Math.abs(x)), 0)));
  const at = Math.max(ctx.currentTime + (c.until > ctx.currentTime ? 0 : LEAD), c.until);
  node.start(at);
  c.until = at + buf.duration;
  c.heard = level(f);
  c.played.push([at, c.until, frameDb(f)]);
  c.stats.played += buf.duration;
  c.stats.chunks++;
  c.sources.add(node);
  node.onended = () => c.sources.delete(node);
}

// The loudest sound that played lately, in dBFS, or null when nothing did.
function playLevel(c) {
  const now = c.sink.ctx.currentTime;
  while (c.played.length && c.played[0][1] < now - ECHO_S) c.played.shift();
  let db = null;
  for (const [a, , d] of c.played) if (a <= now) db = Math.max(db ?? -Infinity, d);
  return db;
}

function learned() {
  try {
    const v = localStorage.getItem(ECHO_KEY);
    // A little above what this phone showed last time, never above the cautious start.
    if (v !== null && Number.isFinite(Number(v))) return Math.min(BARGE.echoStart, Number(v) + 3);
  } catch {}
  return BARGE.echoStart;
}

// The page's side of the call, for the server log: whether the audio could play and what cut it.
function stats(c) {
  const t = c.stream.getAudioTracks()[0]?.getSettings() || {};
  send(c, { type: 'stats', context: c.ctx.state, rate: c.ctx.sampleRate, base_latency: c.ctx.baseLatency, output_latency: c.ctx.outputLatency,
    ...c.stats, echo: Math.round(c.barge.echo * 10) / 10, muted: c.muted, got: c.got, path: c.sink.path, out_context: c.sink.ctx.state,
    out_time: Math.round(c.sink.ctx.currentTime * 10) / 10, beeps: c.beeps, agent: navigator.userAgent,
    mic: { echo_cancellation: t.echoCancellation, noise_suppression: t.noiseSuppression, auto_gain: t.autoGainControl, rate: t.sampleRate } });
}

// Everything queued to play stops now.
function cut(c) {
  c.stats.cut += Math.max(0, c.until - c.sink.ctx.currentTime);
  for (const n of c.sources) {
    try { n.stop(); } catch {}
  }
  c.sources.clear();
  c.until = 0;
}

function message(c, m) {
  if (m.type === 'state') {
    c.state = m.state;
    label(c);
    if (m.state === 'ended') hangUp('');
  } else if (m.type === 'flush') {
    c.stats.flushes++;
    cut(c);
  }
  else if (m.type === 'saying') saying(c, m.text);
  else if (m.type === 'said' && m.who === 'partner') endLive(c);
  else if (m.type === 'error') snack(m.text);
}

// The partner's words as they play, in a line that its finished turn replaces.
function saying(c, text) {
  const stick = nearBottom();
  if (!c.live?.isConnected) {
    c.live = el('div', 'vline live');
    c.live.append(el('span'), el('i', 'caret'));
    thread.append(c.live);
  }
  c.live.firstChild.textContent += text;
  if (stick) toBottom();
}

function endLive(c) {
  c.live?.remove();
  c.live = null;
}

function label(c) {
  const talking = !c.muted && c.barge.talking();
  const playing = c.sink.ctx.currentTime < c.until;
  who.textContent = c.muted ? 'Muted' : LABEL[c.state] || '';
  pill.classList.toggle('me', talking);
  pill.classList.toggle('them', playing && !talking);
}

function tick() {
  const c = call;
  if (!c) return;
  const playing = c.sink.ctx.currentTime < c.until;
  if (!debug.hidden) shown(c);
  if (performance.now() - (c.statsAt || c.t0) > STATS_MS) {
    c.statsAt = performance.now();
    stats(c);
  }
  c.levels = [...c.levels.slice(1), c.muted ? 0 : playing && !c.barge.talking() ? c.heard : c.mic];
  c.levels.forEach((l, i) => { lvl.children[i].style.height = `${4 + 24 * l}px`; });
  const sec = Math.floor((performance.now() - c.t0) / 1000);
  time.textContent = `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, '0')}`;
  label(c);
  setTimeout(tick, 80);
}

function hangUp(why) {
  const c = call;
  if (!c) return;
  call = null;
  stats(c);
  try { localStorage.setItem(ECHO_KEY, String(c.barge.echo)); } catch {}
  send(c, { type: 'end' });
  c.ws.close();
  c.proc.onaudioprocess = null;
  c.proc.disconnect();
  c.src.disconnect();
  c.stream.getTracks().forEach((t) => t.stop());
  cut(c);
  c.ctx.close();
  if (c.sink.ctx !== c.ctx) c.sink.ctx.close();
  c.audio?.pause();
  c.lock?.release().catch(() => {});
  endLive(c);
  show(false);
  if (!debug.hidden) shown();
  if (why) snack(why);
  say('Call ended');
}

// The panel: what the browser is, whether its audio runs, what reached it, and what the beeps did.
function shown(c = call) {
  const ua = navigator.userAgent;
  const b = ua.match(/(Firefox|SamsungBrowser|EdgA|Chrome)\/(\d+)/);
  const lines = [`${b ? `${b[1]} ${b[2]}` : 'browser?'}${/Android/.test(ua) ? ' on Android' : ''}${c ? ` · out: ${PATHS[c.sink.path]}` : ' · no call, mic closed'}`];
  if (c) {
    const s = c.sink.ctx;
    lines.push(`audio ${s.state} at ${s.sampleRate} Hz, clock ${s.currentTime.toFixed(1)} s · mic ${c.ctx.state}`,
      `got ${c.got} chunks, played ${c.stats.chunks} (${c.stats.played.toFixed(1)} s), last peak ${c.peak.toFixed(0)} dBFS`,
      `barge cuts ${c.stats.barges} · flushes ${c.stats.flushes}`);
  }
  readout.textContent = [...lines, ...tried.slice(-5)].join('\n');
}

// A beep's outcome, as the browser reports it.
function note(text) {
  tried.push(text);
  shown();
  if (call) send(call, { type: 'stats', beep: text });
}

// An error's name and message, as short as the browser lets it be.
const why = (e) => `${e?.name || 'Error'}${e?.message ? `: ${e.message.slice(0, 80)}` : ''}`;

// Watches an audio context for a moment after a beep: a context the browser holds back stays suspended.
function watch(label, ctx, played) {
  const t0 = ctx.currentTime;
  setTimeout(() => note(`${label}: context ${ctx.state}, clock moved ${(ctx.currentTime - t0).toFixed(2)} s${played ? `, element ${typeof played === 'function' ? played() : played}` : ''}`), 400);
}

// An element's play(), whose refusal says why: NotAllowedError is the browser's autoplay block.
function playing(el, label, then) {
  let res = 'pending';
  el.play().then(() => { res = 'playing'; }, (e) => { res = why(e); note(`${label}: play() refused, ${res}`); });
  then(() => res);
}

// Moves the partner's audio to another way out, made in this tap.
function route(c, path) {
  if (path === c.sink.path) return;
  cut(c);
  if (c.sink.ctx !== c.ctx) c.sink.ctx.close();
  c.audio?.pause();
  c.audio = null;
  c.out.disconnect();
  if (path === 'fresh') {
    const ctx = new AudioContext();
    const out = ctx.createGain();
    out.connect(ctx.destination);
    c.sink = { ctx, out, path };
  } else {
    if (path === 'element') {
      const dest = c.ctx.createMediaStreamDestination();
      c.out.connect(dest);
      c.audio = new Audio();
      c.audio.srcObject = dest.stream;
      c.audio.play().catch((e) => snack(`The audio element won't play: ${e.name}`));
    } else c.out.connect(c.ctx.destination);
    c.sink = { ctx: c.ctx, out: c.out, path };
  }
  c.sink.ctx.resume().catch(() => {});
}

// Half a second of 660 Hz at -10 dBFS, as PCM16 at 16 kHz.
function tone() {
  const n = RATE / 2, pcm = new Int16Array(n);
  for (let i = 0; i < n; i++) pcm[i] = 0.3 * 32767 * Math.sin(2 * Math.PI * 660 * i / RATE) * Math.min(1, i / 160, (n - i) / 160);
  return pcm;
}

// A beep through one way out. In a call it goes the partner's way and moves the partner there; before a call it uses its own audio, with the mic closed.
// "file" is a plain WAV in an audio element, with no Web Audio at all; "osc" is an oscillator in a context of its own.
function beep(path) {
  const c = call;
  const label = `${c ? 'in call' : 'no call'}, ${path}`;
  try {
    if (path === 'osc') {
      // The plainest Web Audio there is: a new context with no options, at the device's own rate, and an oscillator.
      const ctx = new AudioContext();
      ctx.resume().catch((e) => note(`${label}: resume() refused, ${why(e)}`));
      const osc = ctx.createOscillator(), gain = ctx.createGain();
      osc.frequency.value = 660;
      gain.gain.value = 0.3;
      osc.connect(gain).connect(ctx.destination);
      osc.start();
      osc.stop(ctx.currentTime + 0.5);
      const t0 = ctx.currentTime;
      setTimeout(() => note(`${label}: context ${ctx.state} at ${ctx.sampleRate} Hz, clock moved ${(ctx.currentTime - t0).toFixed(2)} s, output latency ${((ctx.outputLatency || 0) * 1000).toFixed(0)} ms`), 400);
      setTimeout(() => ctx.close(), 1500);
      return;
    }
    if (path === 'file') {
      // The page's policy plays media from its own address only: no blob URL.
      const el = new Audio(new URL('beep.wav', import.meta.url));
      playing(el, label, (res) => setTimeout(() => note(`${label}: element ${res()}, at ${el.currentTime.toFixed(2)} s, paused ${el.paused}`), 400));
      return;
    }
    if (c) {
      route(c, path);
      c.sink.ctx.resume().catch(() => {});
      play(c, tone().buffer);
      watch(label, c.sink.ctx, c.audio ? (c.audio.paused ? 'paused' : 'playing') : '');
      return;
    }
    const ctx = new AudioContext();
    ctx.resume().catch((e) => note(`${label}: resume() refused, ${why(e)}`));
    const buf = ctx.createBuffer(1, RATE / 2, RATE);
    buf.copyToChannel(fromPcm16(tone().buffer), 0);
    const node = ctx.createBufferSource();
    node.buffer = buf;
    let el = null;
    if (path === 'element') {
      const dest = ctx.createMediaStreamDestination();
      node.connect(dest);
      el = new Audio();
      el.srcObject = dest.stream;
      playing(el, label, () => {});
    } else node.connect(ctx.destination);
    node.start();
    watch(label, ctx, el && (() => (el.paused ? 'paused' : 'playing')));
    setTimeout(() => { el?.pause(); ctx.close(); }, 1500);
  } catch (e) {
    note(`${label}: threw ${why(e)}`);
  }
}

$('talk').addEventListener('click', start);
function panel(open) {
  debug.hidden = !open;
  try { localStorage.setItem(DEBUG_KEY, open ? '1' : ''); } catch {}
  if (open) shown();
}
pill.addEventListener('click', () => panel(debug.hidden));
$('call-debug-x').addEventListener('click', () => panel(false));
for (const b of debug.querySelectorAll('[data-beep]')) b.addEventListener('click', () => beep(b.dataset.beep));
// #voicedebug in the address opens it too, before any call.
try { if (localStorage.getItem(DEBUG_KEY) || location.hash === '#voicedebug') panel(true); } catch {}
end.addEventListener('click', () => hangUp(''));
mute.addEventListener('click', () => {
  if (!call) return;
  call.muted = !call.muted;
  mute.classList.toggle('on', call.muted);
  mute.setAttribute('aria-label', call.muted ? 'Unmute' : 'Mute');
  label(call);
});

// The voice button shows when the server has voice on and the browser can record (https or localhost).
if (window.isSecureContext && navigator.mediaDevices?.getUserMedia && window.AudioContext) {
  fetch('api/voice').then((r) => (r.ok ? r.json() : {})).then((j) => {
    S.voice = !!j.enabled;
    render();
  }).catch(() => {});
}
