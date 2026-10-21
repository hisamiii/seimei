// The conversation with seimei: rendering events, live text, approval cards, and the composer.
import { md } from './md.js';
import { $, S, hooks, el, svg, clock, dayKey, dayLabel, home, plural, post, view, snack, finePointer, toLogin, ICON } from './util.js';
import { openWorker } from './agents.js';
import { renderTasks } from './tasks.js';

export const log = $('log');
export const thread = $('thread');
export const traj = $('traj');
const older = $('older'), olderT = $('older-t'), working = $('working');
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
const grow = new ResizeObserver(() => { if (pinned) log.scrollTop = log.scrollHeight; });
grow.observe(thread);
grow.observe(traj);

// ---------- tool activity ----------

const ICONS = {
  Bash: 'M4 17l5-5-5-5M11 19h9',
  Read: 'M6 3h9l4 4v14H6zM14 3v5h5',
  Edit: 'M4 20h4L19 9l-4-4L4 16zM13 7l4 4',
  Search: 'M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM16 16l4 4',
  Web: 'M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3 12h18M12 3c3 3.5 3 14.5 0 18M12 3c-3 3.5-3 14.5 0 18',
  Tool: 'M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2.4-2.4z',
};

// The built-in plugins' tools (mcp__browser__browser_tabs) read like seimei's own, as they were named before them.
const ours = name => name.replace(/^mcp__(desktop|discord|browser|music|queue)__/, 'mcp__seimei__');

export function phrase(name, n) {
  name = ours(name);
  if (name === 'Bash') return `ran ${plural(n, 'a command', 'commands')}`;
  if (name === 'Read') return `read ${plural(n, 'a file', 'files')}`;
  if (/^(Edit|Write|MultiEdit|NotebookEdit)$/.test(name)) return `edited ${plural(n, 'a file', 'files')}`;
  if (/^(Grep|Glob)$/.test(name)) return n === 1 ? 'searched the files' : `searched ${n} times`;
  if (name === 'WebSearch') return 'searched the web';
  if (name === 'WebFetch') return `opened ${plural(n, 'a page', 'pages')}`;
  if (/^(Agent|Task)$/.test(name)) return `started ${plural(n, 'an agent', 'agents')}`;
  if (/^(TodoWrite|TaskCreate|TaskUpdate|TaskList|TaskGet)$/.test(name)) return 'updated its tasks';
  if (name === 'ToolSearch') return 'loaded tools';
  if (name === 'AskUserQuestion') return n === 1 ? 'asked you a question' : `asked you ${n} questions`;
  if (name === 'mcp__seimei__workers') return 'checked the shikigami';
  if (name === 'mcp__seimei__peek') return n === 1 ? 'looked in on a shikigami' : `looked in on shikigami ${n} times`;
  if (name === 'mcp__seimei__message') return n === 1 ? 'messaged a shikigami' : `sent ${n} shikigami messages`;
  if (name === 'mcp__seimei__stop') return 'stopped a shikigami';
  if (name === 'mcp__seimei__show_image') return n === 1 ? 'showed you an image' : `showed you ${n} images`;
  if (name === 'mcp__seimei__send_file') return n === 1 ? 'sent you a file' : `sent you ${n} files`;
  if (name === 'mcp__seimei__browser_tabs') return 'looked at your tabs';
  if (name === 'mcp__seimei__browser_read') return n === 1 ? 'read a page' : `read ${n} page parts`;
  if (name === 'mcp__seimei__browser_open') return n === 1 ? 'opened a tab' : `opened ${n} tabs`;
  if (name === 'mcp__seimei__browser_switch') return 'switched tabs';
  if (name === 'mcp__seimei__browser_close') return 'closed tabs';
  if (name === 'mcp__seimei__browser_download') return n === 1 ? 'started a download' : `started ${n} downloads`;
  if (name === 'mcp__seimei__browser_downloads') return 'checked the downloads';
  if (name === 'mcp__seimei__discord_guilds') return 'looked at your servers';
  if (name === 'mcp__seimei__discord_dms') return 'looked at your DMs';
  if (name === 'mcp__seimei__discord_read') return n === 1 ? 'read Discord' : `read Discord ${n} times`;
  if (name === 'mcp__seimei__discord_search') return n === 1 ? 'searched Discord' : `searched Discord ${n} times`;
  if (name === 'mcp__seimei__discord_send') return n === 1 ? 'sent a Discord message' : `sent ${n} Discord messages`;
  if (name === 'mcp__seimei__discord_edit') return n === 1 ? 'edited a Discord message' : `edited ${n} Discord messages`;
  if (name === 'mcp__seimei__discord_delete') return n === 1 ? 'deleted a Discord message' : `deleted ${n} Discord messages`;
  if (name === 'mcp__seimei__discord_react') return n === 1 ? 'reacted on Discord' : `reacted ${n} times on Discord`;
  if (name === 'mcp__seimei__discord_mark_read') return n === 1 ? 'marked a channel read' : `marked ${n} channels read`;
  const short = name.startsWith('mcp__') ? name.split('__').pop().replace(/_/g, ' ') : name;
  return n === 1 ? `used ${short}` : `used ${short} ${n} times`;
}

// What a call that is still running does, for the live pill and the Agents list.
export function doing(name) {
  if (name === 'Bash') return 'running a command';
  if (name === 'Read') return 'reading a file';
  if (/^(Edit|Write|MultiEdit|NotebookEdit)$/.test(name)) return 'editing a file';
  if (/^(Grep|Glob)$/.test(name)) return 'searching the files';
  if (name === 'WebSearch') return 'searching the web';
  if (name === 'WebFetch') return 'opening a page';
  if (/^(Agent|Task)$/.test(name)) return 'running an agent';
  const short = name.startsWith('mcp__') ? name.split('__').pop().replace(/_/g, ' ') : name;
  return `using ${short}`;
}

// How long something has run: 42s, 4m 12s, 1h 5m.
export function elapsed(since) {
  const s = Math.max(0, Math.floor(Date.now() / 1000 - since));
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m ${s % 60}s` : `${Math.floor(s / 3600)}h ${Math.floor(s % 3600 / 60)}m`;
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
    this.failed = 0;
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

  // The call still running, under the chip: its command and its last output lines, and how long it has run.
  live(id, info) {
    const r = this.rows.get(id);
    if (!r || !r.row.classList.contains('run')) return false;
    if (!this.box) {
      this.box = el('div', 'livebox');
      this.cmd = el('code', 'cmd');
      this.tail = el('pre', 'tail');
      this.box.append(this.cmd, this.tail);
      this.chip.after(this.box);
    }
    const changed = this.live_ !== id;
    this.live_ = id;
    this.liveName = info.tool;
    this.since = info.since;
    this.cmd.textContent = info.what || '';
    this.tail.textContent = (info.output || []).join('\n');
    this.tail.hidden = !info.output?.length;
    this.node.classList.add('running');
    if (changed) this.label();
    this.tick();
    return true;
  }

  tick() {
    if (this.clock && this.since) this.clock.textContent = elapsed(this.since);
  }

  unlive() {
    this.box?.remove();
    this.box = null;
    this.live_ = null;
    this.node.classList.remove('running');
    this.label();
  }

  result(ev) {
    const r = this.rows.get(ev.tool_id);
    if (ev.tool_id === this.live_) this.unlive();
    if (!r || !r.row.classList.contains('run')) return;
    r.row.classList.remove('run');
    r.row.classList.toggle('err', ev.error);
    r.out.textContent = ev.text || '(no output)';
    this.running--;
    if (ev.error) this.failed++;
    this.label();
  }

  label() {
    const counts = new Map(this.counts);
    if (this.live_) counts.set(this.liveName, counts.get(this.liveName) - 1);
    const parts = [...counts].filter(([, n]) => n > 0).map(([name, n]) => phrase(name, n));
    if (this.live_) parts.push(doing(this.liveName));
    const text = parts.join(', ') + (this.failed ? ` · ${this.failed} failed` : '');
    // A failure shows on the collapsed chip too, not only in its row (QA U5).
    this.chip.classList.toggle('err', this.failed > 0);
    const chev = svg(ICON.chev);
    chev.classList.add('chev');
    this.clock = this.live_ ? el('span', 'clock') : null;
    this.chip.replaceChildren(
      this.running ? el('span', 'spin') : svg(iconFor(this.first)),
      el('span', null, text.charAt(0).toUpperCase() + text.slice(1)),
      ...(this.clock ? [this.clock] : []),
      chev,
    );
    this.tick();
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
  if (ev.event === 'replied') return el('div', 'note', `You ${ev.asked ? 'answered' : 'messaged'} ${ev.name}: ${ev.text}`);
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
  if (ev.event === 'summoned') {
    const how = [ev.model, ev.effort, ev.context_cap].filter(Boolean).join(' · ');
    title.append(el('span', 'meta', `${how} · ${ev.cwd.replace(/^\/home\/[^/]+/, '~')}`));
  }
  head.append(badge, title);
  card.append(head);
  if (ev.text) {
    const body = el('div', ev.event === 'summoned' ? 'body task' : 'body');
    if (ev.event === 'summoned') body.textContent = ev.text;
    else body.innerHTML = md(ev.text);
    card.append(body);
    clipWatch.observe(body);
  }
  return card;
}

// Long card bodies get "Show more". Measured when the body has a size: a card drawn on a hidden tab has none yet (QA B8).
const clipWatch = new ResizeObserver((entries) => {
  for (const { target: body } of entries) {
    if (!body.clientHeight) continue;
    clipWatch.unobserve(body);
    const card = body.parentElement;
    if (body.scrollHeight <= body.clientHeight + 4 || card.querySelector('.more')) continue;
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

// Cards by approval id, and outcomes seen so far (an older page can load a card after its outcome).
const approvals = new Map();
const outcomes = new Map();
const LOCK = 'M7 11V8a5 5 0 0 1 10 0v3M6 11h12a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1z';
const OUTCOME = { approved: 'Approved', denied: 'Denied', expired: 'No answer in 10 min, denied', restart: 'Cancelled by a seimei restart' };

const SUB = { message: 'a session you started', desktop: 'a desktop action', discord: 'a Discord action' };

export function approvalSub(ev) {
  if (ev.action === 'edit') return home((ev.detail || '').split('\n')[0]);
  if (ev.action !== 'command') return SUB[ev.action] || '';
  const words = new Set((ev.detail || '').split(/[\s'"=]+/));
  const s = (S.sessions || []).find((x) => x.name && words.has(x.name));
  return s ? `session ${s.name}${s.project ? ` · ${s.project}` : ''}` : '';
}

function approvalCard(ev) {
  const card = el('div', 'wcard approval');
  const badge = el('span', 'badge');
  badge.append(svg(LOCK, 18));
  const head = el('div', 'head');
  const title = el('div', 'title');
  // Cards from before messages went card-free (2026-10-19) held SendMessage text.
  const message = ev.action === 'message';
  title.append(el('b', null, `${ev.who} wants to ${message ? `message ${ev.target}` : ev.target}`));
  // The subtitle names what it touches, not the title again (QA D8): the session a command names, the file an edit writes.
  const sub = approvalSub(ev);
  if (sub) title.append(el('span', 'sub', sub));
  head.append(badge, title);
  const body = el('div', `body ${message || ev.action === 'discord' ? 'task' : 'cmd'}`, ev.detail);
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

function pic(name, size) {
  const img = el('img', 'pic');
  // The aspect ratio reserves the picture's space before it loads, so the thread does not jump.
  if (size?.[0] && size?.[1]) img.style.aspectRatio = `${size[0]} / ${size[1]}`;
  img.src = `api/files/${name}?w=thumb`;
  img.dataset.name = name;
  img.loading = 'lazy';
  img.alt = '';
  return img;
}

// ---------- files ----------

const FILE_ICON = {
  archive: 'M4 7.5 12 3.5l8 4v9l-8 4-8-4zM4 7.5l8 4 8-4M12 11.5v9',
  audio: 'M9 18V5.5l10-2v12.5M9 18a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0zM19 16a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0z',
  video: 'M4 6.5h11v11H4zM15 10.5l5-3v9l-5-3',
  image: 'M6.5 4.5h11a3 3 0 0 1 3 3v9a3 3 0 0 1-3 3h-11a3 3 0 0 1-3-3v-9a3 3 0 0 1 3-3zM20.5 15.5l-4.2-4.2a1.5 1.5 0 0 0-2.1 0L5.5 19.5',
  code: 'M9 8l-4 4 4 4M15 8l4 4-4 4',
  doc: 'M6 3h9l4 4v14H6zM14 3v5h5M9 13h6M9 17h6',
};
const fileIcon = (kind) => FILE_ICON[kind] || FILE_ICON.doc;
export const size = (n) => (n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`);
const KIND_LABEL = { pdf: 'PDF', archive: 'Archive', audio: 'Audio', video: 'Video', image: 'Image', code: 'Code', text: 'Text', file: 'File' };

// The laptop deletes files after 30 days (uploads.py KEEP_DAYS); older chips show as expired.
const KEEP = 30 * 86400;

// A file as a download chip; the laptop always serves it as an attachment, never rendered.
export function fileChip(f, t) {
  const expired = t && Date.now() / 1000 - t > KEEP;
  const a = el(expired ? 'div' : 'a', `fchip${expired ? ' expired' : ' sl'}`);
  if (!expired) {
    a.href = `api/uploads/${f.id}`;
    a.download = f.name;
  }
  const ic = el('span', 'ic');
  ic.append(svg(fileIcon(f.kind), 20));
  const txt = el('span', 'txt');
  txt.append(el('b', null, f.name), el('small', null, expired ? 'Expired' : `${size(f.size)} · ${KIND_LABEL[f.kind] || 'File'}`));
  a.append(ic, txt);
  if (!expired) a.append(svg('M12 4v11M7 10.5l5 5 5-5M5 20h14', 18));
  return a;
}

function sentFile(ev) {
  const fig = el('figure', 'shown file');
  fig.append(fileChip({ ...ev, id: ev.file }, ev.t));
  if (ev.caption) fig.append(el('figcaption', null, ev.caption));
  return fig;
}

export function message(ev) {
  const node = el('div', `msg ${ev.type}`);
  if (ev.type === 'assistant') node.innerHTML = md(ev.text);
  else {
    if (ev.quote) node.append(anchor(ev.quote, ev.quote_id, ev.quote_lane || 't'));
    if (ev.images?.length) {
      const pics = el('div', `pics n${Math.min(ev.images.length, 3)}${ev.text ? '' : ' only'}`);
      ev.images.forEach((n, i) => pics.append(pic(n, ev.sizes?.[i])));
      node.append(pics);
    }
    if (ev.files?.length) {
      const box = el('div', 'files');
      ev.files.forEach((f) => box.append(fileChip(f, ev.t)));
      node.append(box);
    }
    if (ev.text) node.append(document.createTextNode(ev.text));
  }
  node.dataset.t = ev.t;
  return node;
}

function shown(ev) {
  const fig = el('figure', 'shown');
  const img = pic(ev.file, ev.w && [ev.w, ev.h]);
  // As wide as it fits, never upscaled, and no taller than 60vh: the same box the loaded picture gets.
  if (ev.w && ev.h) img.style.width = `min(100%, ${ev.w}px, calc(60vh * ${(ev.w / ev.h).toFixed(4)}))`;
  fig.append(img);
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

// Events in lane "t" happened on their own (see events.py): they render in the trajectory, and the chat folds
// each run of them into one line.
export const inTraj = (ev) => ev.lane === 't';

// Texts of the user's and seimei's messages by event id, for reply anchors and quotes.
const said = new Map();
const REPLY = 'M9 14 4 9l5-5M4 9h10.5a5.5 5.5 0 0 1 0 11H11';
const ENDED = { done: 'finished', failed: 'failed', stopped: 'was stopped', interrupted: 'was interrupted' };

function foldLabel(ev) {
  if (ev.type === 'worker') return `${ev.name} ${ENDED[ev.event] || ev.event}`;
  if (ev.type === 'system') return ev.text.replace(/\.$/, '');
  if (ev.type === 'rate_limit') return limitText(ev);
  // A reply of the user's turn that seimei marked trajectory:.
  if (ev.type === 'assistant' && ev.re) return 'a reply to you';
  return '';
}

// What the user can answer: seimei's messages and the report, question and approval cards. Each gets a reply button
// on hover; a long press replies on touch. Event id -> the composer's quote and the lane it shows in.
const quotable = new Map();
const VERB = { done: 'finished', failed: 'failed', asking: 'asks' };

function replyable(node, ev, text) {
  quotable.set(ev.id, { text, lane: ev.lane === 't' ? 't' : 'c' });
  node.classList.add('replyable');
  node.dataset.rid = ev.id;
  const b = el('button', 'rbtn sl');
  b.type = 'button';
  b.title = 'Reply';
  b.setAttribute('aria-label', 'Reply');
  b.dataset.reply = ev.id;
  b.append(svg(REPLY, 16));
  node.append(b);
}

const firstLine = (t) => ((t || '').split('\n').find((x) => x.trim()) || '').replace(/^[#>*\-\s]+/, '').trim();

// A chip quoting the message this one answers; a tap goes to it (lane c: the chat, t: the trajectory).
function anchor(text, id, lane) {
  const b = el('button', 're sl');
  b.type = 'button';
  b.dataset.goto = id;
  b.dataset.lane = lane;
  const flat = text.replace(/\s+/g, ' ').trim();
  b.append(svg(REPLY, 14), el('span', null, flat.length > 60 ? `${flat.slice(0, 60)}…` : flat));
  return b;
}

// Turns events into nodes. One instance appends live; a fresh one renders older pages for prepending. The chat
// and the trajectory share the open question cards (asks), so a report in one settles a card in the other.
export class Renderer {
  constructor(target, before = null, { lane = 'c', asks = new Map() } = {}) {
    this.target = target;
    this.before = before;
    this.lane = lane;
    this.day = null;
    this.activity = null;
    this.tools = new Map();
    this.notes = new Map();
    // Shikigami whose "asks" card is still open: any later event about them settles it.
    this.asks = asks;
    // Chat: the open fold line, the last message of the user's, and whether something sits after it.
    this.folded = null;
    this.lastUser = null;
    this.between = false;
    // Trajectory: the dispatcher turn being drawn.
    this.turn = null;
    this.cur = null;
  }

  put(node) {
    if (this.cur && !node.dataset.id) node.dataset.id = this.cur;
    this.target.insertBefore(node, this.before);
    return node;
  }

  daily(ev) {
    if (ev.t && dayKey(ev.t) !== this.day) {
      this.day = dayKey(ev.t);
      this.put(el('div', 'note day', dayLabel(ev.t)));
    }
  }

  // An event of the trajectory, seen from the chat: one line per run, naming what happened.
  fold(ev) {
    if (ev.type === 'tool_result' || ev.type === 'approval_done') return;
    this.cur = ev.id;
    this.daily(ev);
    if (!this.folded?.node.isConnected) {
      const node = this.put(el('button', 'fold sl'));
      node.type = 'button';
      node.dataset.goto = ev.id;
      node.dataset.lane = 't';
      this.folded = { node, reports: [], notes: [] };
      this.activity = null;
      this.between = true;
    }
    // Shikigami reports are named first; notes only when nothing else happened.
    const f = this.folded;
    const l = foldLabel(ev);
    const list = ev.type === 'worker' ? f.reports : f.notes;
    if (l && !list.includes(l)) list.push(l);
    const all = [...f.reports, ...f.notes];
    const shown = all.slice(0, Math.max(1, Math.min(3, f.reports.length)));
    const what = all.length ? shown.join(', ') + (all.length > shown.length ? ` and ${all.length - shown.length} more` : '') : 'seimei acted on its own';
    f.node.replaceChildren(el('span', null, what), el('b', null, 'Trajectory'));
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

  // The call that runs now (a session's open tool call); false when it is not drawn here.
  live(info) {
    const a = info && this.tools.get(info.tool_id);
    if (this.liveAct && this.liveAct !== a) this.liveAct.unlive();
    this.liveAct = a && a.live(info.tool_id, info) ? a : null;
    return !!this.liveAct;
  }

  tick() {
    this.liveAct?.tick();
  }

  add(ev) {
    if (ev.type === 'approval_done') return settle(ev.aid, ev.outcome);
    this.cur = ev.id;
    if (ev.type !== 'tool_result') this.folded = null;
    this.daily(ev);
    // Each dispatcher turn in the trajectory opens with what started it.
    if (this.lane === 't' && ev.turn && ev.turn !== this.turn && ev.type !== 'tool_result') {
      this.turn = ev.turn;
      this.activity = null;
      this.put(el('div', 'trig', ev.why ? `after ${ev.why}` : 'on its own'));
    } else if (this.lane === 't' && !ev.turn) this.turn = null;
    // The part of an event turn that came to the chat (a picture, a reply for the user) says what started it.
    if (this.lane === 'c' && ev.from && ev.from !== this.from && ev.type !== 'tool_result') {
      this.from = ev.from;
      this.activity = null;
      this.put(el('div', 'trig', ev.why ? `after ${ev.why}` : 'on its own'));
    } else if (this.lane === 'c' && !ev.from) this.from = null;
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
    if (ev.type === 'user' || ev.type === 'assistant') {
      if (ev.text) said.set(ev.id, ev.text);
      const node = this.put(message(ev));
      if (ev.type === 'user') {
        this.lastUser = ev.id;
        this.between = false;
      } else if (ev.re && (ev.re_pin || ev.re !== this.lastUser || this.between) && (said.has(ev.re) || ev.re_text)) {
        // A reply far from what it answers says which message that was.
        // re_pin: seimei named the message (re:), so the chip shows even right after it.
        node.prepend(anchor(said.get(ev.re) || ev.re_text, ev.re, 'c'));
        this.between = false;
      }
      if (ev.type === 'assistant') replyable(node, ev, ev.text || '');
      if (this.lane === 't' && ev.type === 'assistant') {
        const b = el('button', 'reply-btn sl', 'Reply in chat');
        b.type = 'button';
        b.dataset.reply = ev.id;
        node.append(b);
      }
      return;
    }
    // What the turn itself shows (lane c: a summoned card, a picture) does not come between.
    if (ev.lane !== 'c') this.between = true;
    if (ev.type === 'system') this.note(ev);
    else if (ev.type === 'worker') {
      const open = this.asks.get(ev.worker);
      if (open && ev.event !== 'summoned') {
        open.classList.replace('asking', 'asked');
        open.querySelector('.title').append(el('span', 'meta', ev.event === 'replied' || ev.event === 'done' ? 'Answered' : 'Closed'));
        this.asks.delete(ev.worker);
      }
      const card = this.put(workerCard(ev));
      if (ev.event === 'asking') this.asks.set(ev.worker, card);
      if (VERB[ev.event]) replyable(card, ev, `${ev.name} ${VERB[ev.event]}: ${firstLine(ev.text)}`);
    }
    else if (ev.type === 'image') this.put(shown(ev));
    else if (ev.type === 'file') this.put(sentFile(ev));
    else if (ev.type === 'approval') replyable(this.put(approvalCard(ev)), ev, `${ev.who} wants to ${ev.target}`);
    else if (ev.type === 'rate_limit') this.put(el('div', `note ${ev.status === 'rejected' ? 'error' : 'warn'}`, limitText(ev)));
  }
}

const asks = new Map();
export const main = new Renderer(thread, null, { asks });
export const trail = new Renderer(traj, null, { lane: 't', asks });

// One event, drawn where it belongs: the trajectory gets it whole, the chat a fold line.
function route(ev, chat = main, t = trail) {
  if (inTraj(ev)) {
    t.add(ev);
    chat.fold(ev);
  } else chat.add(ev);
}

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
  sendBtn.disabled = !typing;
  hooks.chrome();
}

export function clearLive() {
  live?.remove();
  live = null;
  liveText = '';
}

let frame = 0;
// Streaming text of the running turn, in the lane the turn belongs to.
export function onDelta(text, lane) {
  const stick = nearBottom();
  if (!live) {
    live = el('div', 'msg assistant live');
    (lane === 't' ? traj : thread).append(live);
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
  // The user's own message always comes into view, even sent while scrolled up.
  const stick = nearBottom() || ev.type === 'user';
  if (ev.type === 'user' && ev.cid && pending.has(ev.cid)) {
    pending.get(ev.cid).remove();
    pending.delete(ev.cid);
  }
  if (ev.type === 'assistant') clearLive();
  route(ev);
  if (live) live.parentNode.append(live);
  count(ev);
  hello();
  if (stick) toBottom();
}

// An empty chat greets instead of showing a blank sheet (QA U14); an empty trajectory says what will show.
function hello() {
  const empty = (t) => ![...t.children].some((c) => !c.matches('.older, .note.day, .traj-empty, [hidden]'));
  $('hello').hidden = S.lane !== 'c' || !empty(thread);
  $('traj-empty').hidden = !empty(traj);
}

// ---------- chat | trajectory ----------

S.lane = 'c';
const SEEN = 'seimei.trajSeen';
let seen = 0, unseen = 0, lastTraj = 0;
try { seen = +localStorage.getItem(SEEN) || 0; } catch {}
// Where each lane was left; null: at the bottom.
const scrolls = { c: null, t: null, k: 0 };

// What counts toward the Trajectory badge: reports, notes and seimei's own words, not its tool calls.
const worth = (ev) => inTraj(ev) && /^(assistant|worker|system|rate_limit)$/.test(ev.type);

function count(ev) {
  if (!worth(ev)) return;
  lastTraj = Math.max(lastTraj, ev.id);
  if (ev.id <= seen) return;
  if (S.lane === 't' && S.tab === 'chat' && !document.hidden) return markSeen(ev.id);
  unseen++;
  badge();
}

function markSeen(id = null) {
  seen = Math.max(seen, id ?? lastTraj);
  unseen = 0;
  try { localStorage.setItem(SEEN, String(seen)); } catch {}
  badge();
}

function badge() {
  const c = $('traj-count');
  c.hidden = !unseen;
  c.textContent = unseen > 99 ? '99+' : unseen;
  $('lt-traj').setAttribute('aria-label', unseen ? `Trajectory, ${unseen} new` : 'Trajectory');
}

export function showLane(lane, goto = null) {
  if (lane !== S.lane) {
    scrolls[S.lane] = pinned ? null : log.scrollTop;
    S.lane = lane;
    thread.hidden = lane !== 'c';
    traj.hidden = lane !== 't';
    $('tasks').hidden = lane !== 'k';
    document.body.dataset.lane = lane;
    for (const [id, l] of [['lt-chat', 'c'], ['lt-traj', 't'], ['lt-tasks', 'k']]) {
      $(id).classList.toggle('on', l === lane);
      $(id).setAttribute('aria-selected', String(l === lane));
    }
    log.setAttribute('aria-label', { c: 'Chat', t: 'Trajectory', k: 'Tasks' }[lane]);
    if (lane === 'k') renderTasks();
    if (live && live.parentNode !== (lane === 't' ? traj : thread)) live.hidden = true;
    else if (live) live.hidden = false;
    hello();
    if (scrolls[lane] === null) toBottom();
    else {
      log.scrollTop = scrolls[lane];
      pinned = nearBottom();
    }
  }
  if (lane === 't') markSeen();
  const node = goto && (lane === 't' ? traj : thread).querySelector(`[data-id="${goto}"]`);
  if (node) {
    node.scrollIntoView({ block: 'center' });
    pinned = nearBottom();
    node.classList.remove('flash');
    void node.offsetWidth;
    node.classList.add('flash');
  }
}
$('lt-chat').addEventListener('click', () => showLane('c'));
$('lt-traj').addEventListener('click', () => showLane('t'));
$('lt-tasks').addEventListener('click', () => showLane('k'));
document.addEventListener('visibilitychange', () => { if (!document.hidden && S.lane === 't' && S.tab === 'chat') markSeen(); });

// ---------- history ----------

let firstId = null;

export function loadHistory(r) {
  r.events.forEach((ev) => route(ev));
  firstId = r.events[0]?.id ?? null;
  older.hidden = olderT.hidden = !firstId || firstId <= 1;
  // The first visit starts with nothing unseen; later ones count what came while away.
  const t = r.events.filter(worth);
  lastTraj = t.at(-1)?.id || 0;
  if (!seen) markSeen(lastTraj);
  unseen = t.filter((ev) => ev.id > seen).length;
  badge();
  hello();
  toBottom();
}

async function loadOlder() {
  older.disabled = true;
  const r = await fetch(`api/history?before=${firstId}&limit=200`).then((x) => x.json()).catch(() => null);
  older.disabled = false;
  if (!r || !r.events.length) return;
  const height = log.scrollHeight;
  const anchor = older.nextSibling;
  const anchorT = traj.querySelector(':scope > :not(.older, .traj-empty)');
  const page = new Renderer(thread, anchor, { asks });
  const pageT = new Renderer(traj, anchorT, { lane: 't', asks });
  r.events.forEach((ev) => route(ev, page, pageT));
  // Drop the day chip that now repeats right after the inserted page.
  for (const a of [anchor, anchorT]) {
    if (a?.classList?.contains('day') && a.textContent === dayLabel(r.events.at(-1).t)) a.remove();
  }
  firstId = r.events[0].id;
  older.hidden = olderT.hidden = firstId <= 1;
  log.scrollTop += log.scrollHeight - height;
}
older.addEventListener('click', loadOlder);
olderT.addEventListener('click', loadOlder);

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
    if (!S.online) snack('Not connected to the laptop yet. Your message stays here.');
    return;
  }
  input.value = '';
  autosize();
  renderAttachments();
  saveDraft();
  const reply = replyTo;
  quoteOff();
  deliver(text, sending, reply);
});

// Reply (hover button, long press, or "Reply in chat" in the trajectory): the next message answers that message or card.
let replyTo = null;
function replyIn(id) {
  replyTo = { id, ...(quotable.get(id) || { text: said.get(id) || '', lane: 't' }) };
  $('quote-text').textContent = `Replying to: ${replyTo.text.replace(/\s+/g, ' ').trim()}`;
  $('quote').hidden = false;
  showLane('c');
  input.focus();
}
function quoteOff() {
  replyTo = null;
  $('quote').hidden = true;
}
$('quote-x').addEventListener('click', quoteOff);

// The message shows at once, dimmed, until the laptop echoes it back (QA U1); a failed one offers a retry.
const pending = new Map();

function pendingBubble(cid, text, sending, reply) {
  const node = el('div', 'msg user pending');
  if (reply) node.append(anchor(reply.text, reply.id, reply.lane || 't'));
  const imgs = sending.filter((a) => a.kind === 'image');
  if (imgs.length) {
    const pics = el('div', `pics n${Math.min(imgs.length, 3)}${text ? '' : ' only'}`);
    imgs.forEach((a) => { const i = el('img', 'pic'); i.src = a.url; pics.append(i); });
    node.append(pics);
  }
  const docs = sending.filter((a) => a.kind === 'file');
  if (docs.length) {
    const box = el('div', 'files');
    docs.forEach((a) => box.append(el('div', 'fchip', a.name)));
    node.append(box);
  }
  if (text) node.append(document.createTextNode(text));
  node.dataset.cid = cid;
  return node;
}

async function deliver(text, sending, reply = null) {
  const cid = Math.random().toString(36).slice(2, 12);
  const node = pendingBubble(cid, text, sending, reply);
  pending.set(cid, node);
  if (S.lane !== 'c') showLane('c');
  thread.append(node);
  if (live?.parentNode === thread) thread.append(live);
  hello();
  toBottom();
  try {
    // Files upload as soon as they are picked; wait for any still on the way.
    await Promise.all(sending.map((a) => a.upload).filter(Boolean));
    if (sending.some((a) => a.kind === 'file' && !a.id)) throw new Error('upload');
    await post('api/send', { text, cid, reply: reply?.id, images: sending.filter((a) => a.kind === 'image').map((a) => ({ data: a.data })),
      files: sending.filter((a) => a.kind === 'file').map((a) => a.id) });
  } catch {
    if (!pending.has(cid)) return;
    node.classList.add('failed');
    const again = el('button', 'retry sl', 'Not sent. Tap to retry');
    again.type = 'button';
    again.addEventListener('click', () => {
      pending.delete(cid);
      node.remove();
      // A file whose upload failed is gone; the rest go again.
      deliver(text, sending.filter((a) => a.kind !== 'file' || a.id), reply);
    });
    node.append(again);
  }
}

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
  return { kind: 'image', data: await toBase64(blob), url: URL.createObjectURL(blob) };
}

const MAX_FILE = 50 << 20;
const PICTURE = /^image\/(jpeg|png|webp|gif)$/;

// Any other file goes up to the laptop right away; the message then names it by id.
function uploadFile(f) {
  const a = { kind: 'file', name: f.name || 'file', size: f.size, id: null };
  a.upload = fetch(`api/upload?name=${encodeURIComponent(a.name)}`, {
    method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: f,
  }).then(async (r) => {
    if (r.status === 401) toLogin();
    const body = await r.json().catch(() => null);
    if (!r.ok || !body?.id) throw new Error(body?.error || 'upload failed');
    Object.assign(a, { id: body.id, kind: 'file', fkind: body.kind });
  }).catch((e) => {
    attachments.splice(attachments.indexOf(a) >>> 0, attachments.includes(a) ? 1 : 0);
    main.add({ type: 'system', level: 'error', text: `Could not upload ${a.name}: ${e.message}` });
  }).finally(() => { a.upload = null; renderAttachments(); });
  return a;
}

async function addFiles(list) {
  for (const f of list) {
    const pics = attachments.filter((a) => a.kind === 'image').length;
    const files = attachments.length - pics;
    try {
      if (PICTURE.test(f.type) && pics < 6) attachments.push(await prepare(f));
      else if (f.size > MAX_FILE) main.add({ type: 'system', level: 'error', text: `${f.name} is over 50 MB.` });
      else if (files < 10) attachments.push(uploadFile(f));
    } catch {
      main.add({ type: 'system', level: 'error', text: `Could not read ${f.name || 'that file'}.` });
    }
  }
  renderAttachments();
}

function renderAttachments() {
  const box = $('attachments');
  box.replaceChildren();
  attachments.forEach((a, i) => {
    const item = el('div', `att${a.kind === 'file' ? ' file' : ''}${a.upload ? ' busy' : ''}`);
    if (a.kind === 'file') {
      item.append(svg(fileIcon(a.fkind), 20), el('span', 'fname', a.name));
    } else {
      const img = el('img');
      img.src = a.url;
      img.alt = '';
      item.append(img);
    }
    const x = el('button', 'remove sl');
    x.type = 'button';
    x.setAttribute('aria-label', 'Remove');
    x.append(svg('M6 6l12 12M18 6 6 18', 14));
    x.addEventListener('click', () => {
      if (a.url) URL.revokeObjectURL(a.url);
      attachments.splice(i, 1);
      renderAttachments();
    });
    item.append(x);
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

// Tap a message to see when it was sent; tap a shikigami card to open it; tap a picture to zoom; tap a fold line or
// an anchor to go to what it names.
function onTap(e) {
  if (press?.fired) return;
  const goto = e.target.closest('[data-goto]');
  if (goto) return showLane(goto.dataset.lane, +goto.dataset.goto);
  const reply = e.target.closest('.reply-btn, .rbtn');
  if (reply) return replyIn(+reply.dataset.reply);
  if (e.target.matches('img.pic')) {
    const n = e.target.dataset.name;
    return view(e.target.src, n ? { full: `api/files/${n}`, save: `api/files/${n}?download=1` } : {});
  }
  const card = e.target.closest('.wcard');
  if (card?.dataset.worker && !e.target.closest('a, .more') && !getSelection().toString()) return openWorker(card.dataset.worker);
  const msg = e.target.closest('.msg');
  if (!msg || e.target.closest('a') || getSelection().toString()) return;
  const t = msg.querySelector(':scope > .time');
  if (t) t.remove();
  else if (msg.dataset.t) msg.append(el('span', 'time', clock(+msg.dataset.t)));
}
thread.addEventListener('click', onTap);
traj.addEventListener('click', onTap);

// A long press on something answerable replies to it (touch only; a mouse has the hover button).
const PRESS = 450;
let press = null;
function pressStart(e) {
  const node = e.target.closest('.replyable');
  if (e.pointerType !== 'touch' || !node || e.target.closest('a, button, input, textarea, pre')) return;
  const p = { x: e.clientX, y: e.clientY, fired: false };
  p.timer = setTimeout(() => {
    p.fired = true;
    navigator.vibrate?.(12);
    replyIn(+node.dataset.rid);
  }, PRESS);
  press = p;
}
function pressMove(e) {
  if (press && !press.fired && Math.hypot(e.clientX - press.x, e.clientY - press.y) > 10) pressEnd();
}
function pressEnd() {
  if (!press) return;
  clearTimeout(press.timer);
  // A fired press swallows the click that may follow it, and no later tap.
  if (press.fired) setTimeout(() => { press = null; }, 400);
  else press = null;
}
for (const box of [thread, traj]) {
  box.addEventListener('pointerdown', pressStart);
  box.addEventListener('pointermove', pressMove);
  // The press fired from a timer, which mobile browsers do not count as a gesture: focus again on the lift, so the
  // keyboard comes up.
  box.addEventListener('pointerup', () => { if (press?.fired) input.focus(); pressEnd(); });
  box.addEventListener('pointercancel', pressEnd);
  box.addEventListener('contextmenu', (e) => { if (press?.fired) e.preventDefault(); });
}
