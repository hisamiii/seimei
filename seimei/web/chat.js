// The conversation with seimei: rendering events, live text, approval cards, and the composer.
import { md } from './md.js';
import { $, S, hooks, el, svg, clock, dayKey, dayLabel, plural, post, view, finePointer, ICON } from './util.js';
import { openWorker } from './agents.js';

export const log = $('log');
export const thread = $('thread');
const older = $('older'), working = $('working');
const input = $('input'), sendBtn = $('send'), stopBtn = $('stop');

let live = null;
let liveText = '';

export const nearBottom = () => log.scrollHeight - log.scrollTop - log.clientHeight < 120;
// Whether the reader sits at the bottom; content that grows later (pictures loading) keeps them there.
let pinned = true;
export const toBottom = () => {
  log.scrollTop = log.scrollHeight;
  pinned = true;
};
log.addEventListener('scroll', () => { pinned = nearBottom(); }, { passive: true });
// Pictures loading, fonts swapping in and card buttons all grow the thread after it is drawn.
new ResizeObserver(() => { if (pinned) log.scrollTop = log.scrollHeight; }).observe(thread);

// ---------- tool activity ----------

const ICONS = {
  Bash: 'M4 17l5-5-5-5M11 19h9',
  Read: 'M6 3h9l4 4v14H6zM14 3v5h5',
  Edit: 'M4 20h4L19 9l-4-4L4 16zM13 7l4 4',
  Search: 'M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM16 16l4 4',
  Web: 'M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3 12h18M12 3c3 3.5 3 14.5 0 18M12 3c-3 3.5-3 14.5 0 18',
  Tool: 'M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2.4-2.4z',
};

export function phrase(name, n) {
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
    const chev = svg(ICON.chev);
    chev.classList.add('chev');
    this.chip.replaceChildren(
      this.running ? el('span', 'spin') : svg(iconFor(this.first)),
      el('span', null, text.charAt(0).toUpperCase() + text.slice(1)),
      chev,
    );
  }
}

// ---------- cards ----------

const WORKER = {
  summoned: { icon: ICON.star, title: (n) => `Summoned ${n}` },
  done: { icon: ICON.check, title: (n) => `${n} finished` },
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
  if (ev.event === 'summoned') title.append(el('span', 'meta', `${ev.model} · ${ev.cwd.replace(/^\/home\/[^/]+/, '~')}`));
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
  const SUB = { message: 'a session you started', edit: 'a file edit', desktop: 'a desktop action' };
  const sub = SUB[ev.action] || (/niri|window/.test(ev.target) ? 'a desktop command' : 'a session command');
  title.append(el('span', 'sub', sub));
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

export const waitingApprovals = () => [...approvals.entries()].filter(([aid]) => !outcomes.has(aid));

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

export function message(ev) {
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

// A system line repeated within this many seconds counts up on the first one instead of showing again.
const REPEAT = 900;

// Turns events into nodes. One instance appends live; a fresh one renders older pages for prepending.
export class Renderer {
  constructor(target, before = null) {
    this.target = target;
    this.before = before;
    this.day = null;
    this.activity = null;
    this.tools = new Map();
    this.notes = new Map();
  }

  put(node) {
    this.target.insertBefore(node, this.before);
    return node;
  }

  note(ev) {
    const seen = this.notes.get(ev.text);
    if (seen && seen.node.isConnected && (ev.t || 0) - seen.t < REPEAT) {
      seen.n++;
      seen.t = ev.t || seen.t;
      seen.node.dataset.n = `×${seen.n}`;
      return;
    }
    const node = this.put(el('div', `note ${ev.level || ''}`, ev.text));
    this.notes.set(ev.text, { node, t: ev.t || 0, n: 1 });
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
    else if (ev.type === 'system') this.note(ev);
    else if (ev.type === 'worker') this.put(workerCard(ev));
    else if (ev.type === 'image') this.put(shown(ev));
    else if (ev.type === 'approval') this.put(approvalCard(ev));
    else if (ev.type === 'rate_limit') this.put(el('div', `note ${ev.status === 'rejected' ? 'error' : 'warn'}`, limitText(ev)));
  }
}

export const main = new Renderer(thread);

// ---------- live state ----------

export function render() {
  const s = !S.online ? (S.awaySince ? 'away' : 'connecting')
    : !S.session.connected ? 'starting'
    : waitingApprovals().length ? 'waiting'
    : S.session.busy ? 'working' : 'idle';
  const text = { away: `away since ${S.awaySince ? clock(S.awaySince / 1000) : ''}`, connecting: 'connecting',
    starting: 'session starting', waiting: 'waiting for your OK', working: 'working', idle: 'idle' }[s];
  S.status = { s, text };
  document.body.classList.toggle('offline', !S.online && !!S.awaySince);
  const away = !!(S.awaySince && Date.now() - S.awaySince > 4000 && !S.online);
  if (away && $('away').hidden) requestAnimationFrame(toBottom);
  $('away').hidden = !away;
  working.hidden = !(S.online && S.session.busy && !live) || s === 'waiting';
  const typing = input.value.trim() !== '' || attachments.length > 0;
  stopBtn.hidden = !(S.session.busy && !typing);
  sendBtn.hidden = !stopBtn.hidden;
  hooks.chrome();
}

export function clearLive() {
  live?.remove();
  live = null;
  liveText = '';
}

let frame = 0;
export function onDelta(text) {
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

// A logged event: draw it, keep the streaming reply last, and follow the bottom.
export function addEvent(ev) {
  const stick = nearBottom();
  if (ev.type === 'assistant') clearLive();
  main.add(ev);
  if (live) thread.append(live);
  if (stick) toBottom();
}

// ---------- history ----------

let firstId = null;

export function loadHistory(r) {
  r.events.forEach((ev) => main.add(ev));
  firstId = r.events[0]?.id ?? null;
  older.hidden = !firstId || firstId <= 1;
  toBottom();
}

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
older.addEventListener('click', loadOlder);

// ---------- composer ----------

export function autosize() {
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
  if ((!text && !sending.length) || !S.online) {
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

// Tap a message to see when it was sent; tap a shikigami card to open it; tap a picture to zoom.
thread.addEventListener('click', (e) => {
  if (e.target.matches('img.pic')) return view(e.target.src);
  const card = e.target.closest('.wcard');
  if (card?.dataset.worker && !e.target.closest('a, .more') && !getSelection().toString()) return openWorker(card.dataset.worker);
  const msg = e.target.closest('.msg');
  if (!msg || e.target.closest('a') || getSelection().toString()) return;
  const t = msg.querySelector(':scope > .time');
  if (t) t.remove();
  else if (msg.dataset.t) msg.append(el('span', 'time', clock(+msg.dataset.t)));
});
