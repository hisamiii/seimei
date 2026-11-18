// Voice calls (docs/voice.md): the voice button in an empty composer starts a hands-free call with the voice partner over /api/voice/ws.
// The microphone goes up as 16 kHz PCM16 without pause, silence included; the partner's audio comes down and plays at once.
// The page cuts the partner off itself when the user talks over it, well before the partner's own detection does.
import { $, S, el, snack, say, toLogin } from './util.js';
import { render, toBottom, nearBottom } from './chat.js';
import { Down, Barge, RATE, FRAME, toPcm16, fromPcm16, level } from './pcm.js';

const bar = $('callbar'), pill = $('call-pill'), who = $('call-who'), lvl = $('call-level'), time = $('call-time');
const mute = $('call-mute'), end = $('call-end'), thread = $('thread');
const BARS = 26;
// What the speaker has queued starts this far ahead, so a late chunk does not leave a gap.
const LEAD = 0.06;
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
  // The context is made in the tap: a phone lets it play only then.
  const ctx = new AudioContext();
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
    ctx, stream, ws, src, proc, out, t0: performance.now(), down: new Down(ctx.sampleRate), barge: new Barge(),
    pending: new Float32Array(0), sources: new Set(), until: 0, muted: false, state: 'connecting',
    levels: Array(BARS).fill(0), mic: 0, heard: 0, live: null, lock: null,
  };
  call = c;
  src.connect(proc);
  // A ScriptProcessor runs only while connected; it writes silence.
  proc.connect(ctx.destination);
  proc.onaudioprocess = (e) => up(c, e.inputBuffer.getChannelData(0));
  ws.onmessage = (m) => (typeof m.data === 'string' ? message(c, JSON.parse(m.data)) : play(c, m.data));
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
    const playing = c.ctx.currentTime < c.until;
    if (c.muted) f.fill(0);
    else if (c.barge.push(f, playing)) {
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
  const buf = c.ctx.createBuffer(1, f.length, RATE);
  buf.copyToChannel(f, 0);
  const node = c.ctx.createBufferSource();
  node.buffer = buf;
  node.connect(c.out);
  const at = Math.max(c.ctx.currentTime + (c.until > c.ctx.currentTime ? 0 : LEAD), c.until);
  node.start(at);
  c.until = at + buf.duration;
  c.heard = level(f);
  c.sources.add(node);
  node.onended = () => c.sources.delete(node);
}

// Everything queued to play stops now.
function cut(c) {
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
  } else if (m.type === 'flush') cut(c);
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
  const playing = c.ctx.currentTime < c.until;
  who.textContent = c.muted ? 'Muted' : LABEL[c.state] || '';
  pill.classList.toggle('me', talking);
  pill.classList.toggle('them', playing && !talking);
}

function tick() {
  const c = call;
  if (!c) return;
  const playing = c.ctx.currentTime < c.until;
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
  send(c, { type: 'end' });
  c.ws.close();
  c.proc.onaudioprocess = null;
  c.proc.disconnect();
  c.src.disconnect();
  c.stream.getTracks().forEach((t) => t.stop());
  cut(c);
  c.ctx.close();
  c.lock?.release().catch(() => {});
  endLive(c);
  show(false);
  if (why) snack(why);
  say('Call ended');
}

$('talk').addEventListener('click', start);
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
