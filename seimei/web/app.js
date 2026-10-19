import { md } from './md.js';

const $ = (id) => document.getElementById(id);
const log = $('log'), thread = $('thread'), older = $('older'), working = $('working');
const input = $('input'), sendBtn = $('send'), stopBtn = $('stop');
const finePointer = matchMedia('(pointer: fine)').matches;

let lastId = 0;
let firstId = null;
let online = false;
let awaySince = null;
let session = { connected: false, busy: false };
let live = null;
let liveText = '';

// ---------- helpers ----------

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

const clock = (t) => new Date(t * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
const dayKey = (t) => new Date(t * 1000).toDateString();

function dayLabel(t) {
  const d = new Date(t * 1000);
  const today = new Date();
  const yesterday = new Date(Date.now() - 864e5);
  if (d.toDateString() === today.toDateString()) return 'Today';
  if (d.toDateString() === yesterday.toDateString()) return 'Yesterday';
  return d.toLocaleDateString([], { weekday: 'long', day: 'numeric', month: 'long' });
}

const nearBottom = () => log.scrollHeight - log.scrollTop - log.clientHeight < 120;
const toBottom = () => { log.scrollTop = log.scrollHeight; };

async function post(path, body) {
  const r = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!r.ok) throw new Error(`${r.status}`);
  return r.json();
}

const ICONS = {
  Bash: 'M4 17l5-5-5-5M11 19h9',
  Read: 'M6 3h9l4 4v14H6zM14 3v5h5',
  Edit: 'M4 20h4L19 9l-4-4L4 16zM13 7l4 4',
  Search: 'M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM16 16l4 4',
  Web: 'M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3 12h18M12 3c3 3.5 3 14.5 0 18M12 3c-3 3.5-3 14.5 0 18',
  Tool: 'M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2.4-2.4z',
};

function svg(path, size = 16) {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  s.setAttribute('width', size);
  s.setAttribute('height', size);
  s.setAttribute('viewBox', '0 0 24 24');
  s.setAttribute('fill', 'none');
  s.setAttribute('stroke', 'currentColor');
  s.setAttribute('stroke-width', '2');
  s.setAttribute('stroke-linecap', 'round');
  s.setAttribute('stroke-linejoin', 'round');
  s.setAttribute('aria-hidden', 'true');
  const p = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  p.setAttribute('d', path);
  s.append(p);
  return s;
}

// ---------- tool activity ----------

const plural = (n, one, many) => (n === 1 ? one : `${n} ${many}`);

function phrase(name, n) {
  if (name === 'Bash') return `ran ${plural(n, 'a command', 'commands')}`;
  if (name === 'Read') return `read ${plural(n, 'a file', 'files')}`;
  if (/^(Edit|Write|MultiEdit|NotebookEdit)$/.test(name)) return `edited ${plural(n, 'a file', 'files')}`;
  if (/^(Grep|Glob)$/.test(name)) return n === 1 ? 'searched the files' : `searched ${n} times`;
  if (name === 'WebSearch') return 'searched the web';
  if (name === 'WebFetch') return `opened ${plural(n, 'a page', 'pages')}`;
  if (/^(Agent|Task)$/.test(name)) return `started ${plural(n, 'an agent', 'agents')}`;
  if (/^(TodoWrite|TaskCreate|TaskUpdate|TaskList|TaskGet)$/.test(name)) return 'updated its tasks';
  const short = name.startsWith('mcp__') ? name.split('__').pop().replace(/_/g, ' ') : name;
  return n === 1 ? `used ${short}` : `used ${short} ${n} times`;
}

function iconFor(name) {
  if (name === 'Bash') return ICONS.Bash;
  if (name === 'Read') return ICONS.Read;
  if (/^(Edit|Write|MultiEdit|NotebookEdit)$/.test(name)) return ICONS.Edit;
  if (/^(Grep|Glob)$/.test(name)) return ICONS.Search;
  if (/^Web/.test(name)) return ICONS.Web;
  return ICONS.Tool;
}

class Activity {
  constructor() {
    this.node = el('div', 'activity');
    this.chip = el('button', 'chip sl');
    this.chip.type = 'button';
    this.calls = el('div', 'calls');
    this.calls.hidden = true;
    this.node.append(this.chip, this.calls);
    this.counts = new Map();
    this.rows = new Map();
    this.running = 0;
    this.first = null;
    this.chip.addEventListener('click', () => {
      this.calls.hidden = !this.calls.hidden;
      this.node.classList.toggle('open', !this.calls.hidden);
    });
  }

  add(ev) {
    this.first ??= ev.name;
    this.counts.set(ev.name, (this.counts.get(ev.name) || 0) + 1);
    const row = el('button', 'call sl run');
    row.type = 'button';
    row.append(el('b', null, ev.name.startsWith('mcp__') ? ev.name.split('__').pop() : ev.name), el('span', null, ev.summary));
    const out = el('pre', 'out');
    out.hidden = true;
    row.addEventListener('click', () => { if (out.textContent) out.hidden = !out.hidden; });
    this.calls.append(row, out);
    this.rows.set(ev.tool_id, { row, out });
    this.running++;
    this.label();
  }

  result(ev) {
    const r = this.rows.get(ev.tool_id);
    if (!r || !r.row.classList.contains('run')) return;
    r.row.classList.remove('run');
    r.row.classList.toggle('err', ev.error);
    r.out.textContent = ev.text || '(no output)';
    this.running--;
    this.label();
  }

  label() {
    const parts = [...this.counts].map(([name, n]) => phrase(name, n));
    const text = parts.join(', ');
    const chev = svg('M9 6l6 6-6 6');
    chev.classList.add('chev');
    this.chip.replaceChildren(
      this.running ? el('span', 'spin') : svg(iconFor(this.first)),
      el('span', null, text.charAt(0).toUpperCase() + text.slice(1)),
      chev,
    );
  }
}

// ---------- rendering ----------

function message(ev) {
  const node = el('div', `msg ${ev.type}`);
  if (ev.type === 'assistant') node.innerHTML = md(ev.text);
  else node.textContent = ev.text;
  node.dataset.t = ev.t;
  return node;
}

function limitText(ev) {
  const name = { five_hour: '5-hour', seven_day: 'Weekly' }[ev.window] || ev.window;
  const what = ev.status === 'rejected' ? 'limit reached' : ev.status === 'allowed' ? 'limit cleared' : 'limit getting close';
  return `${name} ${what}${ev.resets_at && ev.status !== 'allowed' ? `, resets ${clock(ev.resets_at)}` : ''}`;
}

// Turns events into nodes. One instance appends live; a fresh one renders older pages for prepending.
class Renderer {
  constructor(target, before = null) {
    this.target = target;
    this.before = before;
    this.day = null;
    this.activity = null;
    this.tools = new Map();
  }

  put(node) {
    this.target.insertBefore(node, this.before);
    return node;
  }

  add(ev) {
    if (ev.t && dayKey(ev.t) !== this.day) {
      this.day = dayKey(ev.t);
      this.put(el('div', 'note day', dayLabel(ev.t)));
    }
    if (ev.type === 'tool') {
      if (!this.activity) {
        this.activity = new Activity();
        this.put(this.activity.node);
      }
      this.activity.add(ev);
      this.tools.set(ev.tool_id, this.activity);
      return;
    }
    if (ev.type === 'tool_result') {
      this.tools.get(ev.tool_id)?.result(ev);
      return;
    }
    this.activity = null;
    if (ev.type === 'user' || ev.type === 'assistant') this.put(message(ev));
    else if (ev.type === 'system') this.put(el('div', `note ${ev.level || ''}`, ev.text));
    else if (ev.type === 'rate_limit') this.put(el('div', `note ${ev.status === 'rejected' ? 'error' : 'warn'}`, limitText(ev)));
  }
}

const main = new Renderer(thread);

// ---------- live state ----------

function render() {
  const s = !online ? (awaySince ? 'away' : 'connecting')
    : !session.connected ? 'starting'
    : session.busy ? 'working' : 'idle';
  const text = { away: `away since ${awaySince ? clock(awaySince / 1000) : ''}`, connecting: 'connecting',
    starting: 'session starting', working: 'working', idle: 'idle' }[s];
  $('status').dataset.s = s;
  $('status-text').textContent = text;
  document.body.classList.toggle('offline', !online && !!awaySince);
  const away = !!(awaySince && Date.now() - awaySince > 4000 && !online);
  if (away && $('away').hidden) requestAnimationFrame(toBottom);
  $('away').hidden = !away;
  working.hidden = !(online && session.busy && !live);
  const typing = input.value.trim() !== '';
  stopBtn.hidden = !(session.busy && !typing);
  sendBtn.hidden = !stopBtn.hidden;
}

function clearLive() {
  live?.remove();
  live = null;
  liveText = '';
}

let frame = 0;
function onDelta(text) {
  const stick = nearBottom();
  if (!live) {
    live = el('div', 'msg assistant live');
    thread.append(live);
    render();
  }
  liveText += text;
  if (!frame) {
    frame = requestAnimationFrame(() => {
      frame = 0;
      if (live) live.innerHTML = md(liveText);
      if (stick) toBottom();
    });
  }
}

function handle(ev) {
  if (ev.id) {
    if (ev.id <= lastId) return;
    lastId = ev.id;
    const stick = nearBottom();
    if (ev.type === 'assistant') clearLive();
    main.add(ev);
    if (live) thread.append(live);
    if (stick) toBottom();
    render();
  } else if (ev.type === 'state') {
    session = ev;
    if (!ev.busy) clearLive();
    render();
  } else if (ev.type === 'delta') {
    onDelta(ev.text);
  }
}

// ---------- connection ----------

let source = null;
let awayTimer = 0;
let lastSeen = 0;
const SILENT = 40000;

function lost() {
  if (online) awaySince = Date.now();
  online = false;
  render();
  clearTimeout(awayTimer);
  awayTimer = setTimeout(render, 4500);
}

function connect() {
  source?.close();
  lastSeen = Date.now();
  source = new EventSource(`api/stream?after=${lastId}`);
  source.onopen = () => {
    online = true;
    awaySince = null;
    lastSeen = Date.now();
    render();
  };
  source.onmessage = (e) => {
    lastSeen = Date.now();
    handle(JSON.parse(e.data));
  };
  source.onerror = () => {
    lost();
    // The browser retries on its own unless the server refused the request outright.
    if (source.readyState === EventSource.CLOSED) setTimeout(connect, 10000);
  };
}

// The server pings every 15 s. A suspended laptop never closes the connection, so silence is the signal.
function watchdog() {
  if (Date.now() - lastSeen > SILENT) {
    lost();
    connect();
  }
}
setInterval(watchdog, 5000);
document.addEventListener('visibilitychange', () => { if (!document.hidden) watchdog(); });

async function loadOlder() {
  older.disabled = true;
  const r = await fetch(`api/history?before=${firstId}&limit=200`).then((x) => x.json()).catch(() => null);
  older.disabled = false;
  if (!r || !r.events.length) return;
  const height = log.scrollHeight;
  const anchor = older.nextSibling;
  const page = new Renderer(thread, anchor);
  r.events.forEach((ev) => page.add(ev));
  // Drop the day chip that now repeats right after the inserted page.
  if (anchor?.classList?.contains('day') && anchor.textContent === dayLabel(r.events.at(-1).t)) anchor.remove();
  firstId = r.events[0].id;
  older.hidden = firstId <= 1;
  log.scrollTop += log.scrollHeight - height;
}

async function start() {
  const r = await fetch('api/history?limit=200').then((x) => x.json()).catch(() => null);
  if (r) {
    r.events.forEach((ev) => main.add(ev));
    lastId = r.last;
    firstId = r.events[0]?.id ?? null;
    older.hidden = !firstId || firstId <= 1;
    session = r.state;
    toBottom();
  }
  render();
  connect();
}

// ---------- input ----------

function autosize() {
  input.style.height = 'auto';
  input.style.height = `${Math.min(input.scrollHeight, 168)}px`;
}

$('composer').addEventListener('submit', async (e) => {
  e.preventDefault();
  const text = input.value.trim();
  if (!text || !online) return;
  input.value = '';
  autosize();
  render();
  try {
    await post('api/send', { text });
  } catch {
    input.value = text;
    autosize();
    main.add({ type: 'system', level: 'error', text: 'Could not send that. Try again.' });
    render();
  }
});

input.addEventListener('input', () => { autosize(); render(); });
input.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && finePointer) {
    e.preventDefault();
    $('composer').requestSubmit();
  }
});
stopBtn.addEventListener('click', () => post('api/interrupt', {}).catch(() => {}));
older.addEventListener('click', loadOlder);
$('retry').addEventListener('click', () => location.reload());

// Tap a message to see when it was sent.
thread.addEventListener('click', (e) => {
  const msg = e.target.closest('.msg');
  if (!msg || e.target.closest('a') || getSelection().toString()) return;
  const t = msg.querySelector(':scope > .time');
  if (t) t.remove();
  else if (msg.dataset.t) msg.append(el('span', 'time', clock(+msg.dataset.t)));
});

start();
