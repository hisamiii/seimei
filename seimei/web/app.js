import { md } from './md.js';
import { deviceName, register, supported } from './passkey.js';

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
// Whether the reader sits at the bottom; content that grows later (pictures loading) keeps them there.
let pinned = true;
const toBottom = () => {
  log.scrollTop = log.scrollHeight;
  pinned = true;
};
log.addEventListener('scroll', () => { pinned = nearBottom(); }, { passive: true });
// Pictures loading, fonts swapping in and card buttons all grow the thread after it is drawn.
new ResizeObserver(() => { if (pinned) log.scrollTop = log.scrollHeight; }).observe(thread);

// A session that ended (expired, logged out elsewhere, revoked on the laptop) sends the page back to login.
const toLogin = () => location.reload();

async function post(path, body) {
  const r = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (r.status === 401) toLogin();
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
  if (name === 'ToolSearch') return 'loaded tools';
  if (name === 'mcp__seimei__workers') return 'checked the shikigami';
  if (name === 'mcp__seimei__peek') return n === 1 ? 'looked in on a shikigami' : `looked in on shikigami ${n} times`;
  if (name === 'mcp__seimei__message') return n === 1 ? 'messaged a shikigami' : `sent ${n} shikigami messages`;
  if (name === 'mcp__seimei__stop') return 'stopped a shikigami';
  if (name === 'mcp__seimei__show_image') return n === 1 ? 'showed you an image' : `showed you ${n} images`;
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

const STAR = 'M12 2 17.88 20.09 2.49 8.91h19.02L6.12 20.09Z';
const WORKER = {
  summoned: { icon: STAR, title: (n) => `Summoned ${n}` },
  done: { icon: 'M5 12.5l4.5 4.5L19 7.5', title: (n) => `${n} finished` },
  failed: { icon: 'M12 7v6M12 17v.01M10.3 3.9 2.4 18a2 2 0 0 0 1.7 3h15.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z', title: (n) => `${n} failed` },
  asking: { icon: 'M9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3M12 17v.01', title: (n) => `${n} asks` },
};

function workerCard(ev) {
  const kind = WORKER[ev.event];
  if (!kind) {
    const what = ev.event === 'stopped' ? 'was stopped' : 'was interrupted by a restart';
    return el('div', 'note', `${ev.name} ${what}`);
  }
  const card = el('div', `wcard ${ev.event}`);
  card.dataset.worker = ev.worker;
  const badge = el('span', 'badge');
  badge.append(svg(kind.icon, 18));
  const head = el('div', 'head');
  const title = el('div', 'title');
  title.append(el('b', null, kind.title(ev.name)));
  if (ev.event === 'summoned') title.append(el('span', 'meta', `${ev.model} \u00b7 ${ev.cwd.replace(/^\/home\/[^/]+/, '~')}`));
  head.append(badge, title);
  card.append(head);
  if (ev.text) {
    const body = el('div', ev.event === 'summoned' ? 'body task' : 'body');
    if (ev.event === 'summoned') body.textContent = ev.text;
    else body.innerHTML = md(ev.text);
    card.append(body);
    requestAnimationFrame(() => {
      if (body.scrollHeight > body.clientHeight + 4) {
        card.classList.add('clipped');
        const more = el('button', 'more sl', 'Show more');
        more.type = 'button';
        more.addEventListener('click', () => {
          card.classList.toggle('open');
          more.textContent = card.classList.contains('open') ? 'Show less' : 'Show more';
        });
        card.append(more);
      }
    });
  }
  return card;
}

// ---------- approvals ----------

// Cards by approval id, and outcomes seen so far (an older page can load a card after its outcome).
const approvals = new Map();
const outcomes = new Map();
const LOCK = 'M7 11V8a5 5 0 0 1 10 0v3M6 11h12a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1z';
const OUTCOME = { approved: 'Approved', denied: 'Denied', expired: 'No answer in 10 min, denied' };

function approvalCard(ev) {
  const card = el('div', 'wcard approval');
  const badge = el('span', 'badge');
  badge.append(svg(LOCK, 18));
  const head = el('div', 'head');
  const title = el('div', 'title');
  // Cards from before messages went card-free (2026-10-19) held SendMessage text.
  const message = ev.action === 'message';
  title.append(el('b', null, `${ev.who} wants to ${message ? `message ${ev.target}` : ev.target}`));
  title.append(el('span', 'sub', message ? 'a session you started' : 'a session command'));
  head.append(badge, title);
  const body = el('div', `body ${message ? 'task' : 'cmd'}`, ev.detail);
  const foot = el('div', 'foot');
  const left = el('span', 'left');
  const deny = el('button', 'btn text sl', 'Deny');
  const ok = el('button', 'btn filled sl', 'Approve');
  for (const [b, decision] of [[deny, 'deny'], [ok, 'approve']]) {
    b.type = 'button';
    b.addEventListener('click', async () => {
      deny.disabled = ok.disabled = true;
      try {
        await post(`api/approvals/${ev.aid}`, { decision });
      } catch {
        if (!outcomes.has(ev.aid)) deny.disabled = ok.disabled = false;
      }
    });
  }
  foot.append(left, deny, ok);
  card.append(head, body, foot);
  card.expires = ev.expires;
  approvals.set(ev.aid, card);
  if (outcomes.has(ev.aid)) settle(ev.aid, outcomes.get(ev.aid));
  else tick();
  return card;
}

function settle(aid, outcome) {
  outcomes.set(aid, outcome);
  const card = approvals.get(aid);
  if (!card) return;
  card.classList.add('settled', outcome);
  card.querySelectorAll('.foot button').forEach((b) => b.remove());
  card.querySelector('.foot .left').textContent = OUTCOME[outcome] || outcome;
  render();
}

const waitingApprovals = () => [...approvals.entries()].filter(([aid]) => !outcomes.has(aid));

function tick() {
  const now = Date.now() / 1000;
  for (const [, card] of waitingApprovals()) {
    const left = Math.ceil((card.expires - now) / 60);
    card.querySelector('.foot .left').textContent = left > 0 ? `Auto-denies in ${left} min` : 'Expiring';
  }
}
setInterval(tick, 20000);

function pic(name) {
  const img = el('img', 'pic');
  img.src = `api/files/${name}`;
  img.loading = 'lazy';
  img.alt = '';
  return img;
}

function message(ev) {
  const node = el('div', `msg ${ev.type}`);
  if (ev.type === 'assistant') node.innerHTML = md(ev.text);
  else {
    if (ev.images?.length) {
      const pics = el('div', `pics n${Math.min(ev.images.length, 3)}${ev.text ? '' : ' only'}`);
      ev.images.forEach((n) => pics.append(pic(n)));
      node.append(pics);
    }
    if (ev.text) node.append(document.createTextNode(ev.text));
  }
  node.dataset.t = ev.t;
  return node;
}

function shown(ev) {
  const fig = el('figure', 'shown');
  fig.append(pic(ev.file));
  if (ev.caption) fig.append(el('figcaption', null, ev.caption));
  return fig;
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
    if (ev.type === 'approval_done') return settle(ev.aid, ev.outcome);
    if (ev.t && dayKey(ev.t) !== this.day) {
      this.day = dayKey(ev.t);
      this.put(el('div', 'note day', dayLabel(ev.t)));
    }
    // The summoned card already says it.
    if (ev.type === 'tool' && ev.name === 'mcp__seimei__summon') return;
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
    else if (ev.type === 'worker') this.put(workerCard(ev));
    else if (ev.type === 'image') this.put(shown(ev));
    else if (ev.type === 'approval') this.put(approvalCard(ev));
    else if (ev.type === 'rate_limit') this.put(el('div', `note ${ev.status === 'rejected' ? 'error' : 'warn'}`, limitText(ev)));
  }
}

const main = new Renderer(thread);

// ---------- live state ----------

function render() {
  const s = !online ? (awaySince ? 'away' : 'connecting')
    : !session.connected ? 'starting'
    : waitingApprovals().length ? 'waiting'
    : session.busy ? 'working' : 'idle';
  const text = { away: `away since ${awaySince ? clock(awaySince / 1000) : ''}`, connecting: 'connecting',
    starting: 'session starting', waiting: 'waiting for your OK', working: 'working', idle: 'idle' }[s];
  $('status').dataset.s = s;
  $('status-text').textContent = text;
  document.body.classList.toggle('offline', !online && !!awaySince);
  const away = !!(awaySince && Date.now() - awaySince > 4000 && !online);
  if (away && $('away').hidden) requestAnimationFrame(toBottom);
  $('away').hidden = !away;
  working.hidden = !(online && session.busy && !live) || s === 'waiting';
  const typing = input.value.trim() !== '' || attachments.length > 0;
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
  } else if (ev.type === 'workers') {
    setWorkers(ev);
  } else if (ev.type === 'w') {
    sheetEvent(ev);
  }
}

// ---------- connection ----------

let source = null;
let awayTimer = 0;
let lastSeen = 0;
const SILENT = 40000;
const GRACE = 8000;
let quietUntil = 0;

// A phone pauses a background tab and its stream drops; that is not the laptop sleeping.
// Losses while hidden or during a quiet reconnect only count once that reconnect fails.
function lost() {
  if (online && !document.hidden && Date.now() > quietUntil) awaySince = Date.now();
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
    // The browser retries on its own unless the server refused the request outright,
    // and EventSource cannot see why, so ask whether the session is still valid.
    if (source.readyState === EventSource.CLOSED) {
      fetch('api/auth/me')
        .then((r) => (r.status === 401 ? toLogin() : setTimeout(connect, 10000)))
        .catch(() => setTimeout(connect, 10000));
    }
  };
}

// The server pings every 15 s. A suspended laptop never closes the connection, so silence is the signal.
function watchdog() {
  if (document.hidden) return;
  if (Date.now() - lastSeen > SILENT) {
    lost();
    connect();
  }
}
setInterval(watchdog, 5000);

// Back in front: reconnect quietly, and show the away card only if the laptop does not answer.
function resume() {
  if (online && Date.now() - lastSeen < SILENT) return;
  const since = Date.now();
  quietUntil = since + GRACE;
  awaySince = null;
  online = false;
  render();
  connect();
  setTimeout(() => {
    if (!online && !awaySince) {
      awaySince = since;
      render();
    }
  }, GRACE);
}
document.addEventListener('visibilitychange', () => { if (!document.hidden) resume(); });
window.addEventListener('pageshow', (e) => { if (e.persisted) resume(); });

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
  const res = await fetch('api/history?limit=200').catch(() => null);
  if (res?.status === 401) return toLogin();
  const r = res?.ok ? await res.json() : null;
  if (r) {
    r.events.forEach((ev) => main.add(ev));
    lastId = r.last;
    firstId = r.events[0]?.id ?? null;
    older.hidden = !firstId || firstId <= 1;
    session = r.state;
    setWorkers({ list: r.workers, total: r.workers.length });
    toBottom();
  }
  autosize();
  render();
  connect();
  account();
}

// ---------- account ----------

const menu = $('menu'), card = $('fp-card');

async function account() {
  const r = await fetch('api/auth/me').then((x) => x.json()).catch(() => null);
  if (!r?.auth) return;
  $('menu-btn').hidden = false;
  $('add-passkey').hidden = !supported();
  let dismissed = false;
  try { dismissed = localStorage.getItem('seimei.fp-card') === 'no'; } catch {}
  if (supported() && r.passkeys === 0 && !dismissed) {
    card.hidden = false;
    requestAnimationFrame(toBottom);
  }
}

async function addPasskey() {
  menu.hidden = true;
  const r = await register(deviceName()).catch(() => ({ ok: false }));
  if (r.status === 401) return toLogin();
  if (r.ok) card.hidden = true;
  const text = r.ok ? 'Fingerprint login is set up on this device.'
    : r.exists ? 'This device already has a passkey for seimei.'
    : r.cancelled ? null : 'Could not set up the fingerprint login.';
  if (text) {
    main.add({ type: 'system', level: r.ok || r.exists ? '' : 'error', text });
    toBottom();
  }
}

$('menu-btn').addEventListener('click', (e) => {
  e.stopPropagation();
  menu.hidden = !menu.hidden;
});
document.addEventListener('click', (e) => { if (!menu.contains(e.target)) menu.hidden = true; });
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') menu.hidden = true; });
$('add-passkey').addEventListener('click', addPasskey);
$('fp-yes').addEventListener('click', addPasskey);
$('fp-no').addEventListener('click', () => {
  card.hidden = true;
  try { localStorage.setItem('seimei.fp-card', 'no'); } catch {}
});
$('logout').addEventListener('click', () => post('api/auth/logout', {}).finally(toLogin));

// ---------- input ----------

function autosize() {
  input.style.height = 'auto';
  input.style.height = `${Math.min(input.scrollHeight, 168)}px`;
}

// The draft survives reloads; it lives only in this browser.
const DRAFT = 'seimei.draft';
function saveDraft() {
  try {
    if (input.value) localStorage.setItem(DRAFT, input.value);
    else localStorage.removeItem(DRAFT);
  } catch {}
}
try { input.value = localStorage.getItem(DRAFT) || ''; } catch {}

$('composer').addEventListener('submit', async (e) => {
  e.preventDefault();
  const text = input.value.trim();
  const sending = attachments.splice(0);
  if ((!text && !sending.length) || !online) {
    attachments.push(...sending);
    return;
  }
  input.value = '';
  autosize();
  renderAttachments();
  try {
    await post('api/send', { text, images: sending.map((a) => ({ data: a.data })) });
    if (!input.value) saveDraft();
    sending.forEach((a) => URL.revokeObjectURL(a.url));
  } catch {
    input.value = text;
    attachments.unshift(...sending);
    renderAttachments();
    saveDraft();
    autosize();
    main.add({ type: 'system', level: 'error', text: 'Could not send that. Try again.' });
    render();
  }
});

input.addEventListener('input', () => { autosize(); render(); saveDraft(); });
input.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && finePointer) {
    e.preventDefault();
    $('composer').requestSubmit();
  }
});
stopBtn.addEventListener('click', () => post('api/interrupt', {}).catch(() => {}));
older.addEventListener('click', loadOlder);
$('retry').addEventListener('click', () => location.reload());

// ---------- images ----------

// The model reads images up to 1568 px on the long side; anything bigger only costs upload time.
const MAX_SIDE = 1568;
const attachments = [];

const toBase64 = (blob) => new Promise((resolve, reject) => {
  const r = new FileReader();
  r.onload = () => resolve(String(r.result).split(',')[1]);
  r.onerror = reject;
  r.readAsDataURL(blob);
});

async function prepare(file) {
  const bmp = await createImageBitmap(file);
  const scale = Math.min(1, MAX_SIDE / Math.max(bmp.width, bmp.height));
  let blob = file;
  const lossless = /^image\/(png|gif|webp)$/.test(file.type);
  if (scale < 1 || !lossless || file.size > 3.5e6) {
    const c = document.createElement('canvas');
    c.width = Math.round(bmp.width * scale);
    c.height = Math.round(bmp.height * scale);
    const ctx = c.getContext('2d');
    ctx.fillStyle = '#fff';
    ctx.fillRect(0, 0, c.width, c.height);
    ctx.drawImage(bmp, 0, 0, c.width, c.height);
    blob = await new Promise((resolve) => c.toBlob(resolve, 'image/jpeg', 0.85));
  }
  bmp.close?.();
  return { data: await toBase64(blob), url: URL.createObjectURL(blob) };
}

async function addFiles(list) {
  for (const f of list) {
    if (!f.type.startsWith('image/') || attachments.length >= 6) continue;
    try {
      attachments.push(await prepare(f));
    } catch {
      main.add({ type: 'system', level: 'error', text: `Could not read ${f.name || 'that image'}.` });
    }
  }
  renderAttachments();
}

function renderAttachments() {
  const box = $('attachments');
  box.replaceChildren();
  attachments.forEach((a, i) => {
    const item = el('div', 'att');
    const img = el('img');
    img.src = a.url;
    img.alt = '';
    const x = el('button', 'remove sl');
    x.type = 'button';
    x.setAttribute('aria-label', 'Remove');
    x.append(svg('M6 6l12 12M18 6 6 18', 14));
    x.addEventListener('click', () => {
      URL.revokeObjectURL(a.url);
      attachments.splice(i, 1);
      renderAttachments();
    });
    item.append(img, x);
    box.append(item);
  });
  box.hidden = attachments.length === 0;
  render();
}

$('attach').addEventListener('click', () => $('file').click());
$('file').addEventListener('change', (e) => {
  addFiles([...e.target.files]);
  e.target.value = '';
});
input.addEventListener('paste', (e) => {
  const imgs = [...(e.clipboardData?.files || [])].filter((f) => f.type.startsWith('image/'));
  if (imgs.length) {
    e.preventDefault();
    addFiles(imgs);
  }
});
$('composer').addEventListener('dragover', (e) => e.preventDefault());
$('composer').addEventListener('drop', (e) => {
  e.preventDefault();
  addFiles([...(e.dataTransfer?.files || [])]);
});

const viewer = $('viewer');
function view(src) {
  $('viewer-img').src = src;
  viewer.hidden = false;
}
viewer.addEventListener('click', () => { viewer.hidden = true; });
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') viewer.hidden = true; });

// Tap a message to see when it was sent; tap a shikigami card to open it; tap a picture to zoom.
thread.addEventListener('click', (e) => {
  if (e.target.matches('img.pic')) return view(e.target.src);
  const card = e.target.closest('.wcard');
  if (card && !e.target.closest('a, .more') && !getSelection().toString()) return openWorker(card.dataset.worker);
  const msg = e.target.closest('.msg');
  if (!msg || e.target.closest('a') || getSelection().toString()) return;
  const t = msg.querySelector(':scope > .time');
  if (t) t.remove();
  else if (msg.dataset.t) msg.append(el('span', 'time', clock(+msg.dataset.t)));
});

// ---------- shikigami ----------

const TONES = {
  starting: ['working', 'run'], running: ['working', 'run'], asking: ['needs you', 'ask'], done: ['done', 'ok'],
  failed: ['failed', 'bad'], stopped: ['stopped', 'idle'], interrupted: ['interrupted', 'idle'],
};
const ORDER = { asking: 0, starting: 1, running: 1, failed: 2, done: 3, stopped: 4, interrupted: 4 };
const ACTIVE = new Set(['starting', 'running', 'asking']);
const home = (p) => p.replace(/^\/home\/[^/]+/, '~');
let workers = [];
let workersTotal = 0;

function stateText(w) {
  const at = clock(w.updated);
  return {
    starting: 'Starting', running: `Working since ${at}`, asking: 'Waiting for your answer', done: `Finished at ${at}`,
    failed: `Failed at ${at}`, stopped: `Stopped at ${at}`, interrupted: `Interrupted at ${at}`,
  }[w.state] || w.state;
}

function setWorkers(ev) {
  workers = ev.list;
  workersTotal = ev.total ?? workersTotal;
  const nav = $('chips');
  nav.replaceChildren();
  const list = [...workers].sort((a, b) => (ORDER[a.state] ?? 5) - (ORDER[b.state] ?? 5) || b.updated - a.updated);
  for (const w of list) {
    const [label, tone] = TONES[w.state] || [w.state, 'idle'];
    const b = el('button', `wchip ${tone} sl`);
    b.type = 'button';
    b.append(el('span', 'dot'), el('span', 'name', w.name), el('span', 'st', label));
    b.addEventListener('click', () => openWorker(w.id));
    nav.append(b);
  }
  if (workersTotal) {
    const all = el('button', 'wchip all sl');
    all.type = 'button';
    all.setAttribute('aria-label', 'All shikigami');
    all.append(svg('M4 6h16M4 12h16M4 18h10', 18));
    all.addEventListener('click', openAll);
    nav.append(all);
  }
  if (sheet.worker) {
    const w = workers.find((x) => x.id === sheet.worker.id);
    if (w) updateHead({ ...sheet.worker, ...w });
  }
}

// One bottom sheet, filled with either a worker or the list of all workers.
const sheet = { wrap: $('sheet-wrap'), body: $('sheet-body'), worker: null, log: null, renderer: null, head: null };

function showSheet() {
  if (sheet.wrap.hidden) {
    sheet.wrap.hidden = false;
    history.pushState({ sheet: true }, '');
  }
}

function closeSheet(fromHistory = false) {
  if (sheet.wrap.hidden) return;
  sheet.wrap.hidden = true;
  sheet.worker = null;
  sheet.renderer = null;
  if (!fromHistory && history.state?.sheet) history.back();
}

$('scrim').addEventListener('click', () => closeSheet());
window.addEventListener('popstate', () => closeSheet(true));
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') closeSheet(); });

async function getJSON(path) {
  const r = await fetch(path).catch(() => null);
  if (r?.status === 401) toLogin();
  return r?.ok ? r.json() : null;
}

function updateHead(w) {
  sheet.worker = w;
  const [, tone] = TONES[w.state] || [w.state, 'idle'];
  const h = sheet.head;
  h.badge.className = `wbadge ${tone}`;
  h.sub.textContent = stateText(w);
  const active = ACTIVE.has(w.state);
  h.stop.hidden = !active;
  h.input.placeholder = w.state === 'asking' ? `Answer ${w.name}` : `Message ${w.name}`;
  h.question.hidden = !(w.state === 'asking' && w.question);
  if (w.question) h.question.textContent = w.question;
}

async function openWorker(id) {
  const r = await getJSON(`api/workers/${encodeURIComponent(id)}`);
  if (!r) return;
  const w = r.worker;
  const body = sheet.body;
  body.replaceChildren();

  const badge = el('span', 'wbadge');
  badge.append(el('span'));
  const titles = el('div');
  const sub = el('div', 'sub');
  titles.append(el('h2', null, w.name), sub);
  const head = el('div', 'whead');
  head.append(badge, titles);

  const facts = el('div', 'facts');
  facts.append(el('span', 'mono', home(w.cwd)), el('span', null, w.model), el('span', null, `started ${clock(w.created)}`));

  const task = el('details', 'task');
  task.append(el('summary', null, 'Task'), el('div', null, w.task));

  const question = el('div', 'question');
  const log = el('div', 'wlog');
  sheet.renderer = new Renderer(log);
  r.events.forEach((ev) => sheet.renderer.add(ev));
  sheet.log = log;

  const form = el('form', 'sheet-actions');
  const field = el('label', 'field');
  const input = el('textarea');
  input.rows = 1;
  input.setAttribute('enterkeyhint', 'send');
  field.append(input);
  const send = el('button', 'fab sl');
  send.type = 'submit';
  send.setAttribute('aria-label', 'Send');
  send.append(svg('M12 19V5M5.5 11.5 12 5l6.5 6.5', 20));
  const stop = el('button', 'round-stop sl');
  stop.type = 'button';
  stop.setAttribute('aria-label', 'Stop');
  stop.append(el('span'));
  form.append(field, send, stop);

  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    const text = input.value.trim();
    if (!text) return;
    input.value = '';
    try {
      await post(`api/workers/${encodeURIComponent(w.id)}/send`, { text });
    } catch {
      input.value = text;
    }
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && finePointer) {
      e.preventDefault();
      form.requestSubmit();
    }
  });
  stop.addEventListener('click', () => post(`api/workers/${encodeURIComponent(w.id)}/stop`, {}).catch(() => {}));

  body.append(head, facts, task, question, log, form);
  sheet.head = { badge, sub, stop, input, question };
  updateHead(w);
  showSheet();
  log.scrollTop = log.scrollHeight;
}

function sheetEvent(ev) {
  if (!sheet.renderer || sheet.worker?.id !== ev.worker) return;
  const log = sheet.log;
  const stick = log.scrollHeight - log.scrollTop - log.clientHeight < 80;
  sheet.renderer.add(ev.ev);
  if (stick) log.scrollTop = log.scrollHeight;
}

async function openAll() {
  const r = await getJSON('api/workers');
  if (!r) return;
  sheet.worker = null;
  sheet.renderer = null;
  const list = el('div', 'wlist');
  for (const w of r.workers) {
    const [, tone] = TONES[w.state] || [w.state, 'idle'];
    const row = el('button', `wrow ${tone} sl`);
    row.type = 'button';
    const d = new Date(w.updated * 1000);
    const when = d.toDateString() === new Date().toDateString() ? clock(w.updated) : d.toLocaleDateString([], { day: 'numeric', month: 'short' });
    row.append(el('i', 'dot'), el('b', null, w.name), el('time', null, when), el('span', null, w.summary || stateText(w)));
    row.addEventListener('click', () => openWorker(w.id));
    list.append(row);
  }
  sheet.body.replaceChildren(el('h2', 'list', 'Shikigami'), list);
  showSheet();
}

start();
