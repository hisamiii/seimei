// Voice calls (docs/voice.md): the voice button in an empty composer starts a hands-free call with the voice partner over /api/voice/ws.
// The microphone goes up as 16 kHz PCM16 without pause, silence included; the partner's audio comes down and plays at once.
// The page cuts the partner off itself when the user talks over it, well before the partner's own detection does.
import { $, S, el, snack, say, toLogin } from './util.js';
import { render, toBottom, nearBottom } from './chat.js';
import { Down, Barge, BARGE, RATE, FRAME, toPcm16, fromPcm16, level } from './pcm.js';
import { frameDb } from './phrases.js';
import { Stream, streamable } from './stream.js';

const bar = $('callbar'), pill = $('call-pill'), who = $('call-who'), lvl = $('call-level'), time = $('call-time');
const speedBtn = $('call-speed');
const mute = $('call-mute'), end = $('call-end'), debug = $('call-debug'), readout = $('call-readout');
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
const PATHS = { page: 'the page audio', element: 'an audio element', fresh: 'a new audio context', stream: 'an audio stream' };
// The stream plays outside the browser's audio graph, so its echo canceller never sees it and the partner hears itself.
// There the microphone goes up as silence while the partner plays, and this long after, unless the page hears the user over it.
const GATE_TAIL = 0.3;
// On such a barge the partner gets the microphone's last this many frames too (300 ms): the words that set it off.
const GATE_KEEP = 15;
// How fast the partner talks, kept on this device and sent as audio_speed; a change mid-call goes up too. Phonic sets it when a call starts only.
const SPEED_KEY = 'seimei.voiceSpeed';
const SPEEDS = [0.9, 1, 1.1, 1.2, 1.3];
// The stream must have started playing by then, or the call falls back to Web Audio.
const STREAM_CHECK_MS = 2000;
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

// Firefox on Android may play nothing through Web Audio while a media element plays fine (docs/voice.md): there the partner goes through the stream.
// A page cannot hear whether its own Web Audio is audible, so the choice goes by browser; the stream's own start is checked.
function choose() {
  const ua = navigator.userAgent;
  return streamable() && /Android/.test(ua) && /Firefox\//.test(ua) ? 'stream' : 'page';
}

// The partner's clock: the stream's playhead or the context's time.
const clock = (c) => (c.sink.stream ? c.sink.stream.now() : c.sink.ctx.currentTime);

async function start() {
  if (call) return;
  // The context and the stream's element are started in the tap: a phone lets them play only then.
  const ctx = new AudioContext();
  ctx.resume().catch(() => {});
  const player = choose() === 'stream' ? new Stream(RATE) : null;
  const started = player?.start();
  started?.catch(() => {});
  let stream;
  try {
    stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true } });
  } catch {
    ctx.close();
    player?.close();
    snack('Allow the microphone to talk');
    return;
  }
  const url = new URL('api/voice/ws', location.href);
  const sp = speed();
  showSpeed();
  if (sp) url.searchParams.set('speed', String(sp));
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  const ws = new WebSocket(url);
  ws.binaryType = 'arraybuffer';
  const src = ctx.createMediaStreamSource(stream);
  const proc = ctx.createScriptProcessor(1024, 1, 1);
  const out = ctx.createGain();
  out.connect(ctx.destination);
  const c = {
    // Where the partner's audio goes: a context and the node to connect to (see PATHS).
    sink: player ? { stream: player, path: 'stream' } : { ctx, out, path: 'page' }, got: 0, peak: -Infinity, beeps: 0, fallback: '',
    ctx, stream, ws, src, proc, out, t0: performance.now(), since: Date.now() / 1000, saying: '', down: new Down(ctx.sampleRate), barge: new Barge(BARGE, learned()),
    pending: new Float32Array(0), sources: new Set(), until: 0, muted: false, state: 'connecting',
    levels: Array(BARS).fill(0), mic: 0, heard: 0, lock: null,
    // What was scheduled to play, as [start, end, dBFS], for the echo check.
    played: [], stats: { played: 0, chunks: 0, barges: 0, cut: 0, flushes: 0, gated: 0 }, held: [],
  };
  // A phone may suspend the context, at a call or a route change: the next tap brings it back.
  ctx.onstatechange = () => { if (call === c && ctx.state !== 'running') document.addEventListener('pointerdown', () => ctx.resume().catch(() => {}), { once: true }); };
  call = c;
  if (player) {
    // The self-test: a stream that will not play, or whose playhead stands still, gives way to Web Audio.
    const back = (why) => {
      if (call !== c || c.sink.stream !== player) return;
      c.fallback = why;
      route(c, 'page');
    };
    started.catch((e) => back(`play() refused, ${e.name}`));
    setTimeout(() => { if (player.now() < 0.2) back(`stood still at ${player.now().toFixed(2)} s`); }, STREAM_CHECK_MS);
  }
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

const upFrame = (c, pcm) => {
  if (c.ws.readyState === WebSocket.OPEN) c.ws.send(pcm.buffer);
};

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
      for (const h of c.held) upFrame(c, h);
      c.held = [];
    }
    c.mic = c.muted ? 0 : level(f);
    if (c.sink.stream && !c.muted && clock(c) < c.until + GATE_TAIL) {
      // Gated: the frame waits in case it starts a barge, and silence goes up in its place.
      c.held = [...c.held, toPcm16(f)].slice(-GATE_KEEP);
      c.stats.gated += FRAME / RATE;
      upFrame(c, new Int16Array(FRAME));
    } else {
      c.held = [];
      upFrame(c, toPcm16(f));
    }
  }
  c.pending = buf.slice(i);
}

function send(c, msg) {
  if (c.ws.readyState === WebSocket.OPEN) c.ws.send(JSON.stringify(msg));
}

// The partner's audio, played back to back as it comes.
function play(c, data, beep = false) {
  const f = fromPcm16(data);
  if (!f.length) return;
  c.peak = 20 * Math.log10(Math.max(1 / 32768, f.reduce((m, x) => Math.max(m, Math.abs(x)), 0)));
  if (c.sink.stream) {
    // Only a reply's first chunk counts for the wait: later ones queue behind it.
    const [a, b] = c.sink.stream.push(new Int16Array(data), !beep && clock(c) >= c.until);
    c.until = b;
    c.heard = level(f);
    c.played.push([a, b, frameDb(f)]);
    c.stats.played += b - a;
    c.stats.chunks++;
    return;
  }
  const { ctx, out } = c.sink;
  const buf = ctx.createBuffer(1, f.length, RATE);
  buf.copyToChannel(f, 0);
  const node = ctx.createBufferSource();
  node.buffer = buf;
  node.connect(out);
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
  const now = clock(c);
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
    ...c.stats, echo: Math.round(c.barge.echo * 10) / 10, muted: c.muted, got: c.got, path: c.sink.path, fallback: c.fallback,
    out_state: c.sink.stream ? (c.sink.stream.el.paused ? 'paused' : 'playing') : c.sink.ctx.state, out_time: Math.round(clock(c) * 10) / 10,
    stream_wait_ms: c.sink.stream?.wait() ?? undefined, stream_stalls: c.sink.stream?.stalls, beeps: c.beeps, agent: navigator.userAgent,
    mic: { echo_cancellation: t.echoCancellation, noise_suppression: t.noiseSuppression, auto_gain: t.autoGainControl, rate: t.sampleRate } });
}

// Everything queued to play stops now.
function cut(c) {
  c.stats.cut += Math.max(0, c.until - clock(c));
  c.sink.stream?.cut();
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
  else if (m.type === 'error') snack(m.text);
}

function label(c) {
  const talking = !c.muted && c.barge.talking();
  const playing = clock(c) < c.until;
  who.textContent = c.muted ? 'Muted' : LABEL[c.state] || '';
  pill.classList.toggle('me', talking);
  pill.classList.toggle('them', playing && !talking);
}

function tick() {
  const c = call;
  if (!c) return;
  const playing = clock(c) < c.until;
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
  if (c.sink.ctx && c.sink.ctx !== c.ctx) c.sink.ctx.close();
  c.sink.stream?.close();
  c.audio?.pause();
  c.lock?.release().catch(() => {});
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
    const s = c.sink.ctx, st = c.sink.stream;
    if (st) {
      const ahead = (st.at / st.rate - st.now()) * 1000;
      lines.push(`stream ${st.el.paused ? 'paused' : 'playing'}, clock ${st.now().toFixed(1)} s, ${ahead.toFixed(0)} ms queued · wait ${st.wait() ?? '-'} ms · stalls ${st.stalls} · mic ${c.ctx.state}`);
    } else lines.push(`audio ${s.state} at ${s.sampleRate} Hz, clock ${s.currentTime.toFixed(1)} s · mic ${c.ctx.state}${c.fallback ? ` · stream fell back: ${c.fallback}` : ''}`);
    lines.push(
      `got ${c.got} chunks, played ${c.stats.chunks} (${c.stats.played.toFixed(1)} s), last peak ${c.peak.toFixed(0)} dBFS`,
      `barge cuts ${c.stats.barges} · flushes ${c.stats.flushes}${c.sink.stream ? ` · mic gated ${c.stats.gated.toFixed(1)} s` : ''}`);
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
  if (c.sink.ctx && c.sink.ctx !== c.ctx) c.sink.ctx.close();
  c.sink.stream?.close();
  c.audio?.pause();
  c.audio = null;
  c.out.disconnect();
  if (path === 'stream') {
    const stream = new Stream(RATE);
    stream.start().catch((e) => note(`stream: play() refused, ${why(e)}`));
    c.sink = { stream, path };
    return;
  }
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
      if (c.sink.stream) {
        const st = c.sink.stream, t0 = st.now();
        play(c, tone().buffer, true);
        setTimeout(() => note(`${label}: element ${st.el.paused ? 'paused' : 'playing'}, clock moved ${(st.now() - t0).toFixed(2)} s, stalls ${st.stalls}`), 600);
        return;
      }
      c.sink.ctx.resume().catch(() => {});
      play(c, tone().buffer, true);
      watch(label, c.sink.ctx, c.audio ? (c.audio.paused ? 'paused' : 'playing') : '');
      return;
    }
    if (path === 'stream') {
      const st = new Stream(RATE);
      st.start().catch((e) => note(`${label}: play() refused, ${why(e)}`));
      st.push(tone());
      setTimeout(() => note(`${label}: element ${st.el.paused ? 'paused' : 'playing'}, clock ${st.now().toFixed(2)} s, stalls ${st.stalls}`), 900);
      setTimeout(() => st.close(), 2000);
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
// The device's speed, or null for the server's default.
function speed() {
  try {
    const v = Number(localStorage.getItem(SPEED_KEY));
    return SPEEDS.includes(v) ? v : null;
  } catch {
    return null;
  }
}

const times = (v) => `${v.toFixed(1)}×`;

function showSpeed() {
  const v = speed() ?? 1;
  speedBtn.textContent = times(v);
  speedBtn.setAttribute('aria-label', `Speed ${times(v)}`);
}

speedBtn.addEventListener('click', () => {
  const v = SPEEDS[(SPEEDS.indexOf(speed() ?? 1) + 1) % SPEEDS.length];
  try { localStorage.setItem(SPEED_KEY, String(v)); } catch {}
  showSpeed();
  if (call) send(call, { type: 'speed', speed: v });
});
showSpeed();

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
