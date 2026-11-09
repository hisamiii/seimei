// Shikigami and the user's own agent sessions: the Agents list, and the full-screen page of one of them.
import { $, S, hooks, el, svg, clock, at as atTime, home, short, when, getJSON, snack, finePointer, toLogin, ICON, keep, wearShape, enter, leave } from './util.js';
import { Renderer, doing as doingPhrase, elapsed, loader } from './chat.js';
import { sheet, showSheet, closeSheet } from './sheet.js';
import { metricFacts } from './usage.js';
import { current as themeNow } from './theme.js';
import { THR, threadDetails, sendSheet, threadSeen } from './threads.js';

const page = $('agents');
const sp = $('spage');

const ACTIVE = new Set(['starting', 'running', 'asking']);

function stateText(w) {
  const at = atTime(w.updated);
  const since = at.replace(/^at /, '').replace(/^on /, '');
  return {
    starting: 'Starting', running: `Working since ${since}`, asking: 'Needs you', done: `Finished ${at}`,
    failed: `Failed ${at}`, stopped: `Stopped ${at}`, interrupted: `Interrupted ${at}`,
    laptop: `In a terminal on the server since ${since}`, released: `Your session since ${since}`,
  }[w.state] || w.state;
}

const cap = (t) => t.charAt(0).toUpperCase() + t.slice(1);

// What a shikigami is up to: its latest step.
function workerLine(w) {
  const d = w.doing;
  if (!d) return w.state === 'starting' ? 'Starting' : '';
  if (d.text) return d.text;
  // A command is the news itself
  if (d.name === 'Bash' && d.summary) return d.summary;
  return d.summary ? `${cap(doingPhrase(d.name))}: ${d.summary}` : cap(doingPhrase(d.name));
}

// What a session is up to: its open call and how long it has run.
function sessionLine(d) {
  if (!d) return '';
  const what = d.tool === 'Bash' ? d.what : `${doingPhrase(d.tool)}: ${d.what}`;
  return `${d.tool === 'Bash' ? what : cap(what)}${d.since ? ` · ${elapsed(d.since).split(' ')[0]}` : ''}`;
}

// ---------- the user's own sessions: which turns are new to them ----------

const SEEN = 'seimei.seen';
let seen = {};
try { seen = JSON.parse(localStorage.getItem(SEEN) || '{}'); } catch {}
const saveSeen = () => { try { localStorage.setItem(SEEN, JSON.stringify(seen)); } catch {} };

export function setSessions(list) {
  // A session seen for the first time starts read; only turns that end after that get marked.
  let fresh = false;
  for (const s of list) {
    if (!(s.session_id in seen)) {
      seen[s.session_id] = s.status === 'busy' ? 0 : s.active;
      fresh = true;
    }
  }
  if (fresh) saveSeen();
  S.sessions = list;
  changed();
  if (P?.kind === 'session') {
    const s = list.find((x) => x.pid === P.id);
    if (s) {
      P.row = s;
      head();
    }
  }
}

function markSeen(s) {
  seen[s.session_id] = s.active;
  saveSeen();
}

// ---------- shikigami ----------

let all = null;
let allFetch = 0;
let showAll = false;

export function setWorkers(ev) {
  S.workers = ev.list;
  S.workersTotal = ev.total ?? S.workersTotal;
  if (P?.kind === 'worker') {
    const w = S.workers.find((x) => x.id === P.id);
    if (w) {
      P.row = { ...P.row, ...w };
      head();
    }
  }
  // The full list (finished ones from days ago too) is fetched only while the tab shows.
  if (S.tab === 'agents') refreshAll();
  changed();
}

async function refreshAll() {
  const now = Date.now();
  if (now - allFetch < 2000) return;
  allFetch = now;
  const r = await getJSON('api/workers');
  if (r) {
    all = r.workers;
    renderAgents();
  }
}

// ---------- one model for both kinds ----------

// The theme's project colors (Catppuccin's by default); a project keeps its place in the list everywhere.
const TINTS = ['#89b4fa', '#a6e3a1', '#94e2d5', '#cba6f7', '#f5c2e7', '#b4befe', '#f9e2af', '#74c7ec', '#eba0ac', '#fab387'];
export function tint(project) {
  if (!project) return themeNow?.['text-3'] || '#a6adc8';
  const tints = themeNow?.tints || TINTS;
  let h = 0;
  for (const c of project) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return tints[h % tints.length];
}

export function fromWorker(w) {
  const state = w.state === 'asking' ? 'ask' : ACTIVE.has(w.state) ? 'work' : 'done';
  return {
    kind: 'worker', id: w.id, name: w.name, project: w.project, context: w.context, state, since: w.updated,
    line: state === 'ask' ? (w.question || '') : state === 'work' ? workerLine(w) : (w.summary || stateText(w)),
    bad: w.state === 'failed', end: w.state, archived: !!w.archived, find: [w.name, w.task, w.summary, w.now && `${w.now.id} ${w.now.title}`],
  };
}

export function fromSession(s) {
  const asking = s.question || s.status === 'waiting';
  const state = asking ? 'ask' : s.status === 'busy' ? 'work' : 'idle';
  return {
    kind: 'session', id: s.pid, name: s.name, project: s.project, context: s.context, state, since: s.since,
    line: s.question ? (s.ask || '') : asking ? (s.waiting_text || 'Waiting on the server')
      : state === 'work' ? sessionLine(s.doing) : (s.line || ''),
    fresh: state === 'idle' && s.active > (seen[s.session_id] || 0) + 1,
    archived: !!s.archived, find: [s.name, s.project, s.line],
  };
}

// A side thread as a row of the lists and the page's header: the agents' idle tile wearing the thread icon.
export function fromThread(t) {
  const work = t.state === 'running' || t.state === 'starting';
  const line = work ? (t.doing ? workerLine({ doing: t.doing }) : 'Working') : t.shikigami ? 'Became a shikigami' : (t.last || '');
  return {
    kind: 'thread', id: t.id, name: t.name, context: t.context, state: work ? 'work' : 'idle', since: t.updated, line,
    icon: THR, archived: !!t.archived, fresh: t.unread > 0, sent: t.sent, find: [t.name, t.last, t.quote],
  };
}

// The navigation badge: working agents, and whether anything waits on the user.
export function agentCounts() {
  const items = [...S.workers.map(fromWorker), ...S.sessions.map(fromSession)].filter((x) => !x.archived);
  return { working: items.filter((x) => x.state === 'work').length, asks: items.filter((x) => x.state === 'ask').length };
}

function changed() {
  hooks.chrome();
  if (S.tab === 'agents') renderAgents();
}
setInterval(() => { if (S.tab === 'agents') renderAgents(); }, 30000);

// ---------- the list ----------

const Q_ICON = 'M4 5h16v11H9l-5 4zM10 9.2a2 2 0 1 1 2.6 1.9c-.4.2-.6.5-.6.9M12 13.6v.1';
const CLOCK = 'M12 4a8 8 0 1 1 0 16 8 8 0 0 1 0-16zM12 8v4l2.5 2';
// How a finished shikigami ended, when not by finishing.
const END = { stopped: 'M7.5 7.5h9v9h-9z', interrupted: 'M4 12a8 8 0 1 0 2.4-5.7M4 4.5v4h4' };

export function tile(x) {
  // A live one wears its identity shape (a working one morphs it); finished ones are plain circles
  const live = x.state !== 'done';
  const t = el('span', `tile${live ? ' sh' : ''} ${x.state}${x.bad ? ' bad' : ''}`);
  t.style.setProperty('--c', tint(x.project));
  if (live) wearShape(t, x.name);
  if (x.state === 'work') {
    t.append(loader('bare small'));
    return t;
  }
  const icon = x.icon || (x.state === 'ask' ? Q_ICON : x.state === 'idle' ? CLOCK : x.bad ? 'M12 7v6M12 17v.01' : END[x.end] || ICON.check);
  t.append(svg(icon, 20));
  return t;
}

// The part of a line around a search match, with the match marked.
export function marked(text, q) {
  const i = q ? text.toLowerCase().indexOf(q) : -1;
  if (i < 0) return [text];
  // A match far into the line keeps a few words before it in view; the cut falls between words.
  const from = i > 40 ? text.lastIndexOf(' ', i - 24) + 1 : 0;
  const m = el('mark', null, text.slice(i, i + q.length));
  return [`${from ? '…' : ''}${text.slice(from, i)}`, m, text.slice(i + q.length)];
}

export function row(x, q = '') {
  const b = el('button', `agent sl ${x.state}${x.archived ? ' arch' : ''}`);
  b.type = 'button';
  const top = el('span', 'top');
  top.append(el('b', null, x.name));
  if (x.sent) top.append(el('span', 'tag sent', 'sent'));
  const t = el('time', x.fresh ? 'fresh' : null, x.state === 'done' ? when(x.since) : short(x.since));
  top.append(t);
  const sub = el('span', 'sub');
  if (x.project && x.project !== x.name) {
    const p = el('span', 'proj', x.project);
    p.style.color = tint(x.project);
    sub.append(p, x.line ? ' · ' : '');
  }
  // A search match in the task, not the line: show the task around it.
  const hit = q && !x.line.toLowerCase().includes(q) && x.find.find((f) => f && f !== x.name && f.toLowerCase().includes(q));
  const line = el('span', 'line');
  line.append(...marked(hit || x.line, q));
  sub.append(line);
  const txt = el('span', 'txt');
  txt.append(top, sub);
  b.append(tile(x), txt);
  let held = false;
  b.addEventListener('click', () => { if (!held) openPage(x.kind, x.id); held = false; });
  b.dataset.id = x.id;
  // Right-click, or a long press on touch: the row's menu.
  // Only a finished or idle agent can be archived: one that works or asks always shows.
  if (x.state !== 'done' && x.state !== 'idle') return b;
  b.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    if (sheet.view !== 'rowmenu') rowMenu(x);
  });
  let timer = 0;
  b.addEventListener('pointerdown', (e) => {
    if (e.pointerType !== 'touch') return;
    timer = setTimeout(() => { held = true; rowMenu(x); }, 550);
  });
  for (const ev of ['pointerup', 'pointercancel', 'pointerleave']) b.addEventListener(ev, () => clearTimeout(timer));
  return b;
}

function group(title, cls, items, q) {
  const sec = el('section', `agroup ${cls}`);
  const h = el('h2', null, title);
  h.hidden = !title;
  sec.append(h, ...items.map((x) => row(x, q)));
  return sec;
}

// ---------- the archive: rows hidden from the list; nothing is stopped or deleted ----------

export const ARCH = 'M3 4h18v4H3zM5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8M10 12h4';
let archView = false;
let archQuery = '';

export const archOpen = () => archView;
export function closeArch() {
  archView = false;
  archQuery = '';
  renderAgents();
}

function openArch() {
  archView = true;
  history.pushState({ arch: true }, '');
  renderAgents();
  page.scrollTop = 0;
}

export async function setArchived(x, on, undo = true) {
  const path = x.kind === 'worker' ? `api/workers/${encodeURIComponent(x.id)}/archive`
    : x.kind === 'thread' ? `api/threads/${x.id}/archive` : `api/sessions/${x.id}/archive`;
  const r = await postJSON(path, { archived: on });
  if (!r.ok) return snack(r.error);
  // An archived conversation is not kept in this browser.
  if (on && !(P && P.kind === x.kind && String(P.id) === String(x.id))) keep.drop(`${x.kind}-${x.id}`);
  // The server's update follows; show it now.
  if (x.kind === 'worker') for (const w of [...S.workers, ...(all || [])]) { if (w.id === x.id) w.archived = on; }
  else if (x.kind === 'thread') for (const t of [...S.threads, ...(S.threadsAll || [])]) { if (t.id === x.id) t.archived = on; }
  else for (const s of S.sessions) { if (s.pid === x.id) s.archived = on; }
  if (P && P.kind === x.kind && P.id === x.id) {
    P.row.archived = on;
    head();
  }
  changed();
  hooks.threads?.();
  if (undo) snack(`${on ? 'Archived' : 'Unarchived'} ${x.name}`, { label: 'Undo', run: () => setArchived(x, !on, false) });
}

export function rowMenu(x) {
  const opt = (icon, label, hint, run) => {
    const b = el('button', 'mopt sl');
    b.type = 'button';
    const t = el('span', null, label);
    if (hint) t.append(el('small', null, hint));
    b.append(svg(icon, 22), t);
    b.addEventListener('click', () => { closeSheet(); run(); });
    return b;
  };
  const who = x.name;
  const hint = x.archived ? null : x.kind === 'session' ? 'Keeps running. Back when it does something.' : 'A message brings it back';
  sheet.body.replaceChildren(el('div', 'mwho', who),
    opt(ARCH, x.archived ? 'Unarchive' : 'Archive', hint, () => setArchived(x, !x.archived)),
    opt('M5 12h14M13 6l6 6-6 6', 'Open', null, () => openPage(x.kind, x.id)));
  showSheet('rowmenu');
}

function archived() {
  return [...(all || S.workers).filter((w) => w.archived && !ACTIVE.has(w.state) && w.state !== 'released').map(fromWorker),
    ...S.sessions.filter((s) => s.archived).map(fromSession)].sort((a, b) => b.since - a.since);
}

function archRow(n) {
  const b = el('button', 'archrow sl');
  b.type = 'button';
  const t = el('span', 'txt');
  t.append(el('b', null, 'Archived'), el('span', null, `${n} shikigami and session${n === 1 ? '' : 's'}`));
  const tl = el('span', 'tile done');
  tl.append(svg(ARCH, 20));
  b.append(tl, t, svg('m9 6 6 6-6 6', 20));
  b.addEventListener('click', openArch);
  return b;
}

function archPage() {
  const list = archived();
  const q = archQuery.trim().toLowerCase();
  const hits = q ? list.filter((x) => x.find.some((f) => f && f.toLowerCase().includes(q))) : list;
  const head = el('div', 'ahead');
  const back = el('button', 'back sl');
  back.type = 'button';
  back.setAttribute('aria-label', 'Back to agents');
  back.append(svg('M15 5l-7 7 7 7', 22));
  back.addEventListener('click', () => history.back());
  head.append(back, el('h2', null, 'Archived'), el('span', 'count', q ? `${hits.length} of ${list.length}` : `${list.length}`));
  const search = el('label', 'asearch');
  const input = el('input');
  input.type = 'search';
  input.placeholder = 'Search';
  input.setAttribute('aria-label', 'Search the archive');
  input.value = archQuery;
  input.addEventListener('input', () => {
    archQuery = input.value;
    const pos = input.selectionStart;
    renderAgents();
    const again = page.querySelector('.asearch input');
    again.focus();
    again.setSelectionRange(pos, pos);
  });
  search.append(svg('M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM20 20l-3.5-3.5', 20), input);
  const parts = [head, search];
  if (hits.length) parts.push(group(q ? 'Matches' : '', 'done', hits, q));
  else parts.push(el('div', 'empty', q ? 'No matches' : 'Nothing archived. Idle ones move here after 12 hours.'));
  return parts;
}

function renderAgents() {
  if (archView) {
    const top = page.scrollTop;
    page.replaceChildren(...archPage());
    page.scrollTop = top;
    return;
  }
  const items = [...S.workers.filter((w) => ACTIVE.has(w.state)).map(fromWorker), ...S.sessions.map(fromSession)]
    .filter((x) => !x.archived);
  const by = (st) => items.filter((x) => x.state === st).sort((a, b) => b.since - a.since);
  const ask = by('ask'), work = by('work'), idle = by('idle');
  const parts = [];
  if (ask.length) parts.push(group('Needs you', 'ask', ask));
  if (work.length) parts.push(group('Working', 'work', work));
  if (idle.length) parts.push(group('Idle', 'idle', idle));
  // One given to the user (released) is with their sessions now.
  const done = (all || S.workers).filter((w) => !ACTIVE.has(w.state) && !w.archived && w.state !== 'released').sort((a, b) => b.updated - a.updated);
  if (done.length) {
    const shown = showAll ? done : done.slice(0, 15);
    parts.push(group('Finished', 'done', shown.map(fromWorker)));
    if (done.length > shown.length) {
      const more = el('button', 'older sl', `Show all ${done.length}`);
      more.type = 'button';
      more.addEventListener('click', () => { showAll = true; renderAgents(); });
      parts.push(more);
    }
  }
  const arch = archived().length;
  if (arch) parts.push(archRow(arch));
  if (!parts.length) {
    const empty = el('div', 'empty');
    empty.append(el('b', null, 'No shikigami or sessions yet'));
    parts.push(empty);
  }
  const top = page.scrollTop;
  page.replaceChildren(...parts);
  page.scrollTop = top;
}

export function showAgents() {
  if (archView && !history.state?.arch) {
    archView = false;
    archQuery = '';
  }
  renderAgents();
  allFetch = 0;
  refreshAll();
}

// ---------- the page of one agent ----------

// The open page: kind 'worker' or 'session', its id, its latest row, and the parts that update live.
let P = null;

function ring(pct) {
  const b = el('button', 'ring sl');
  b.type = 'button';
  // No context figure yet: an info button, not an empty track that reads as a spinner.
  if (pct == null) {
    b.classList.add('info');
    b.setAttribute('aria-label', 'Details');
    b.append(svg('M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 11v5M12 7.5v.01', 22));
    return b;
  }
  const n = Math.max(0, Math.min(100, pct ?? 0));
  b.setAttribute('aria-label', pct == null ? 'Details' : `Context ${n}% used`);
  b.innerHTML = `<svg width="34" height="34" viewBox="0 0 36 36" aria-hidden="true"><circle class="t" cx="18" cy="18" r="15"/>`
    + `<circle class="f" cx="18" cy="18" r="15" pathLength="100" stroke-dasharray="${n} 100" transform="rotate(-90 18 18)"/></svg>`;
  b.append(el('span', null, pct == null ? '' : String(n)));
  b.classList.toggle('hot', n >= 80);
  return b;
}

function head() {
  if (!P) return;
  if (P.kind === 'thread') return threadHead();
  const x = P.kind === 'worker' ? fromWorker(P.row) : fromSession(P.row);
  P.title.textContent = x.name;
  const sub = P.sub;
  sub.replaceChildren();
  const dot = el('i', x.state);
  sub.append(dot);
  if (x.project && x.project !== x.name) {
    const p = el('b', null, x.project);
    p.style.color = tint(x.project);
    sub.append(p, ' · ');
  }
  const word = { ask: 'Needs you', work: 'Working', idle: 'Idle', done: P.row.state ? stateText(P.row) : 'Done' }[x.state];
  sub.append(el('span', x.state, x.state === 'done' ? word : [word, short(x.since).replace('now', '')].filter(Boolean).join(' · ')));
  const r = ring(x.context);
  r.addEventListener('click', details);
  P.ring.replaceWith(r);
  P.ring = r;
  if (P.kind === 'worker') {
    P.stop.hidden = !ACTIVE.has(P.row.state) || P.row.state === 'asking';
    question(P.row.state === 'asking' ? { text: P.row.question } : null);
  }
  const can = P.kind === 'worker' ? P.row.started !== false && !['released', 'starting'].includes(P.row.state)
    : ['interactive', 'background'].includes(P.row.kind);
  P.hand.hidden = !can;
  P.hand.textContent = P.kind === 'worker' ? 'Take over' : 'Hand off';
  P.hand.setAttribute('aria-label', P.kind === 'worker' ? `Take over ${x.name}` : `Hand ${x.name} off to seimei`);
  P.banner.hidden = !P.row.archived;
  if (P.row.archived) {
    const un = el('button', 'sl', 'Unarchive');
    un.type = 'button';
    un.addEventListener('click', () => setArchived(x, false));
    P.banner.replaceChildren(svg(ARCH, 20), el('span', null, P.kind === 'worker'
      ? 'Archived. A message brings it back.' : 'Archived, still running'), un);
  }
  composerState();
}

// A side thread's header: its model and state, Send while it can be sent, and why the composer is closed when it is.
function threadHead() {
  const r = P.row;
  const draft = !P.id;
  const x = fromThread({ ...r, id: P.id });
  P.title.textContent = draft ? 'New side thread' : r.name;
  const word = draft ? 'New' : x.state === 'work' ? 'Working' : r.shikigami ? 'Shikigami' : 'Idle';
  P.sub.replaceChildren(el('span', x.state, [r.model, word].filter(Boolean).join(' · ')));
  const ringEl = ring(draft ? null : x.context);
  ringEl.addEventListener('click', () => !draft && threadDetails(P.row, threadChanged));
  ringEl.hidden = draft;
  P.ring.replaceWith(ringEl);
  P.ring = ringEl;
  P.hand.textContent = 'Send';
  P.hand.setAttribute('aria-label', 'Send to seimei');
  P.hand.hidden = draft || !!r.shikigami || !P.renderer.target.querySelector('.msg.assistant');
  P.stop.hidden = x.state !== 'work';
  P.dot.hidden = !hooks.seimeiNews?.();
  P.banner.hidden = !r.archived && !r.shikigami;
  if (r.shikigami) {
    const open = el('button', 'sl', 'Open');
    open.type = 'button';
    open.addEventListener('click', () => openPage('worker', r.shikigami));
    P.banner.replaceChildren(svg(ICON.check, 20), el('span', null, 'Became a shikigami'), open);
  } else if (r.archived) {
    const un = el('button', 'sl', 'Unarchive');
    un.type = 'button';
    un.addEventListener('click', () => setArchived(x, false));
    P.banner.replaceChildren(svg(ARCH, 20), el('span', null, 'Archived. A message brings it back.'), un);
  }
  P.quote.hidden = !draft || !r.quote;
  composerState();
}

// The thread's row changed on the server (a sheet's action, the stream): the page takes it.
function threadChanged(row) {
  if (P?.kind !== 'thread' || !row || row.id !== P.id) return;
  P.row = { ...P.row, ...row };
  head();
}

// The stream's threads listing: the open page takes its row.
export function threadRows(list) {
  if (P?.kind !== 'thread' || !P.id) return;
  const t = list.find((x) => x.id === P.id);
  if (t) threadChanged(t);
}

// Take over: the shikigami goes on as the user's own session (their tap is the OK).
// Hand off: the session becomes a shikigami once the user approves the card in the chat, as when seimei adopts one.
async function handoff() {
  if (!P) return;
  if (P.kind === 'thread') return sendSheet(P.row);
  const { kind, id, row } = P;
  P.hand.disabled = true;
  const r = await postJSON(kind === 'worker' ? `api/workers/${encodeURIComponent(id)}/release` : `api/sessions/${id}/adopt`, {});
  if (P) P.hand.disabled = false;
  if (!r.ok) return snack(r.error);
  if (kind === 'worker') {
    snack(`${row.name} is yours, on the server`);
    history.back();
  } else snack('Allow it in Chat to hand it off');
}

// The question card at the end of the timeline: options to tap when the server can be reached.
function question(ask) {
  const key = ask ? ask.tool_id || ask.text : null;
  if (key === P.askKey) return;
  P.askKey = key;
  P.ask = ask;
  P.qbox.replaceChildren();
  P.qbox.hidden = !ask;
  if (!ask) return composerState();
  const top = el('div', 'qhead');
  const ic = el('span', 'qic');
  ic.append(svg(Q_ICON, 18));
  const t = el('span', 'qt');
  const q = ask.questions?.[0];
  t.append(el('b', null, q?.question || ask.text || ''));
  top.append(ic, t);
  P.qbox.append(top);
  if (q) {
    const opts = el('div', 'qopts');
    P.qbox.append(opts);
    (q.options || []).forEach((o, i) => {
      const b = el('button', 'qopt sl');
      b.type = 'button';
      b.append(el('b', null, o.label));
      if (o.description) b.append(el('small', null, o.description));
      b.disabled = !ask.answerable || !!P.row.reach;
      b.hidden = !!P.row.reach;
      b.addEventListener('click', () => answer({ option: i + 1 }, b));
      opts.append(b);
    });
    if (!ask.answerable || P.row.reach) P.qbox.append(el('p', 'qnote', 'Answer on the server'));
  }
  composerState();
  stick(true);
}

async function answer(body, btn) {
  for (const b of P.qbox.querySelectorAll('button')) b.disabled = true;
  btn?.classList.add('picked');
  const r = await postJSON(`api/sessions/${P.id}/answer`, { tool_id: P.ask.tool_id, ...body });
  if (!r.ok) {
    btn?.classList.remove('picked');
    for (const b of P.qbox.querySelectorAll('button')) b.disabled = false;
    note(r.error);
    return false;
  }
  note(null);
  poll(true);
  // The keys may not land on the server: if the same question is still open after 10 s, it can be answered again.
  btn?.append(el('span', 'sent', 'Sent'));
  const key = P.askKey;
  setTimeout(() => {
    if (!P || P.askKey !== key) return;
    btn?.classList.remove('picked');
    btn?.querySelector('.sent')?.remove();
    for (const b of P.qbox.querySelectorAll('button')) b.disabled = !P.ask?.answerable || !!P.row.reach;
  }, 10000);
  return true;
}

export async function postJSON(path, body) {
  const r = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }).catch(() => null);
  if (r?.status === 401) toLogin();
  const j = await r?.json().catch(() => null);
  return r?.ok ? { ...(j && typeof j === 'object' ? j : {}), ok: true } : { ok: false, error: j?.error || (r ? `The server said no (${r.status})` : "The server didn't answer") };
}

function note(text, cls = 'error') {
  P.note.textContent = text || '';
  P.note.className = `snote ${cls}`;
  P.note.hidden = !text;
}

// Why the composer cannot type into this session, if it cannot.
function blocked() {
  if (P.kind === 'thread') return P.row.shikigami ? 'Now a shikigami' : null;
  if (P.kind === 'worker') return null;
  const s = P.row;
  if (s.reach && (s.via !== 'seimei' || P.ask)) return P.ask ? 'Answer on the server' : s.reach;
  if (P.ask && !P.ask.answerable) return 'Answer on the server';
  if (s.status === 'waiting' && !s.question) return `${s.waiting_text || 'Waiting on the server'}. Answer it there.`;
  return null;
}

function composerState() {
  if (!P) return;
  const why = blocked();
  P.input.disabled = !!why;
  P.attach.disabled = !!why;
  const nm = (P.row.name || '').length > 22 ? `${P.row.name.slice(0, 21).trim()}…` : P.row.name;
  P.input.placeholder = P.kind === 'thread' && !P.id ? 'Ask on the side'
    : why ? (P.kind === 'thread' ? why : 'Can\'t reply from here right now') : P.ask ? 'Your answer' : `Message ${nm}`;
  if (P.kind === 'session') {
    if (why) note(why, 'info');
    else if (P.note.classList.contains('info')) note(null);
  }
  sendState();
}

// ---------- attachments: everything uploads to the server at once, and the message names the files ----------

const MAX_FILE = 50 << 20;

function addFiles(list) {
  for (const f of list) {
    if (P.files.length >= 10) break;
    if (f.size > MAX_FILE) {
      note(`${f.name} is larger than 50 MB.`);
      continue;
    }
    const a = { name: f.name || 'file', id: null, url: f.type.startsWith('image/') ? URL.createObjectURL(f) : null };
    a.upload = fetch(`api/upload?name=${encodeURIComponent(a.name)}`, {
      method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: f,
    }).then(async (r) => {
      if (r.status === 401) toLogin();
      const body = await r.json().catch(() => null);
      if (!r.ok || !body?.id) throw new Error(body?.error || 'upload failed');
      a.id = body.id;
    }).catch((e) => {
      P?.files.splice(P.files.indexOf(a) >>> 0, P.files.includes(a) ? 1 : 0);
      note(`Couldn't upload ${a.name}: ${e.message}`);
    }).finally(() => { a.upload = null; tray(); });
    P.files.push(a);
  }
  tray();
}

function tray() {
  if (!P) return;
  P.tray.replaceChildren();
  P.files.forEach((a, i) => {
    const item = el('div', `att${a.url ? '' : ' file'}${a.upload ? ' busy' : ''}`);
    if (a.url) {
      const img = el('img');
      img.src = a.url;
      img.alt = a.name;
      item.append(img);
    } else {
      item.append(svg('M6 3h8l4 4v14H6zM14 3v4h4', 20), el('span', 'fname', a.name));
    }
    const x = el('button', 'remove sl');
    x.type = 'button';
    x.setAttribute('aria-label', `Remove ${a.name}`);
    x.append(svg('M6 6l12 12M18 6 6 18', 14));
    x.addEventListener('click', () => {
      if (a.url) URL.revokeObjectURL(a.url);
      P.files.splice(i, 1);
      tray();
    });
    item.append(x);
    P.tray.append(item);
  });
  P.tray.hidden = !P.files.length;
  sendState();
}

function sendState() {
  if (!P) return;
  const typing = P.input.value.trim() !== '' || P.files.length > 0;
  P.send.disabled = P.input.disabled || !typing;
  if (P.kind === 'thread') {
    P.stop.hidden = !ACTIVE.has(P.row.state) || typing;
    P.send.hidden = !P.stop.hidden;
  } else if (P.kind === 'worker') {
    // Asking: the answer goes through Send (disabled until typed); Stop is in the details sheet.
    P.stop.hidden = !ACTIVE.has(P.row.state) || P.row.state === 'asking' || typing;
    P.send.hidden = !P.stop.hidden;
  }
}

async function submit(e) {
  e.preventDefault();
  const text = P.input.value.trim();
  const files = P.files.splice(0);
  if (!text && !files.length) return;
  P.input.value = '';
  autosize();
  tray();
  const answering = P.kind === 'session' && P.ask?.answerable;
  const sent = P.kind === 'session' && !answering ? pend(text || files.map((a) => a.name).join(', ')) : null;
  stick(true);
  await Promise.all(files.map((a) => a.upload).filter(Boolean));
  const ids = files.filter((a) => a.id).map((a) => a.id);
  let r;
  const p = P;
  if (P.kind === 'thread' && !P.id) r = await postJSON('api/threads', { text, files: ids, quote: P.row.quote?.id });
  else if (P.kind === 'thread') r = await postJSON(`api/threads/${P.id}/send`, { text, files: ids });
  else if (P.kind === 'worker') r = await postJSON(`api/workers/${encodeURIComponent(P.id)}/send`, { text, files: ids });
  else if (answering) r = await postJSON(`api/sessions/${P.id}/answer`, { tool_id: P.ask.tool_id, text, files: ids });
  else r = await postJSON(`api/sessions/${P.id}/send`, { text, files: ids });
  if (!r.ok) {
    if (sent) unpend(sent);
    P.input.value = text;
    P.files.unshift(...files.filter((a) => a.id));
    tray();
    autosize();
    note(r.error);
    return;
  }
  files.forEach((a) => a.url && URL.revokeObjectURL(a.url));
  // The session held a relayed message for the user's OK: say where to give it.
  note(r.held ? r.note || 'It waits for your OK in the session.' : null, 'held');
  // The draft is a thread now: its log so far, and the stream's events for it from here.
  if (r.thread && P === p && !P.id) {
    P.id = r.thread.id;
    P.row = { ...P.row, ...r.thread };
    hooks.threads?.();
    const h = await getJSON(`api/threads/${P.id}`);
    if (h && P === p) add(h.events);
  }
  if (r.via === 'seimei') snack(r.held ? 'Held until you allow it in its terminal' : 'In its inbox');
  composerState();
  stick(true);
  if (P.kind === 'session') poll(true);
}

// A sent message shows dimmed at the end until the transcript has it.
function pend(text) {
  const node = el('div', 'msg user pending', text);
  P.pending.push(node);
  P.thread.append(node);
  return node;
}

function unpend(node) {
  if (P) P.pending = P.pending.filter((n) => n !== node);
  node.remove();
}

function autosize() {
  P.input.style.height = 'auto';
  P.input.style.height = `${Math.min(P.input.scrollHeight, 160)}px`;
}

// ---------- the timeline ----------

// Like a chat: new events keep the reader at the bottom unless they scrolled up.
const nearBottom = () => P.main.scrollHeight - P.main.scrollTop - P.main.clientHeight < 120;
function stick(force = false) {
  if (P && (force || P.pinned)) P.main.scrollTop = P.main.scrollHeight;
}

// Draws what is new (an event with an id already drawn is skipped) and keeps the page's last KEEP_PAGE events.
const KEEP_PAGE = 400;
// What a session's timeline holds: a copy of another shape is drawn anew, so bump it when the server shows more of a transcript.
const SHAPE = 2;
function add(events) {
  const fresh = events.filter((ev) => ev.id == null || !P.ids.has(String(ev.id)));
  for (const ev of fresh) {
    if (ev.id != null) P.ids.add(String(ev.id));
    if (typeof ev.id === 'number') P.last = Math.max(P.last, ev.id);
    P.renderer.add(ev);
    // The sent message arrived: its dimmed copy goes (the oldest, when the transcript's text differs).
    if (ev.type === 'user' && P.pending.length) {
      const i = P.pending.findIndex((n) => n.textContent === ev.text);
      P.pending.splice(Math.max(i, 0), 1)[0].remove();
    }
  }
  if (fresh.length) {
    if (P.kind === 'thread') seenSoon();
    P.empty?.remove();
    P.thread.append(...P.pending);
    P.raw = P.raw.concat(fresh).slice(-KEEP_PAGE);
    const p = P;
    if (p.id != null) keep.later(`${p.kind}-${p.id}`, () => ({ events: p.raw, last: p.last, next: p.next, row: p.row, shape: SHAPE }));
  }
}

// One poll at a time: only the latest one draws and schedules the next, so an older answer never moves the offset back.
let pollTimer = 0;
let pollRun = 0;
// The poll whose read is on its way, and whether the transcript grew meanwhile: then one more read follows it.
let reading = 0;
let again = false;
async function poll(soon = false) {
  clearTimeout(pollTimer);
  if (!P || P.kind !== 'session') return;
  const p = P;
  const run = ++pollRun;
  if (soon) await new Promise((r) => setTimeout(r, 600));
  if (run !== pollRun) return;
  reading = run;
  const r = await getJSON(`api/sessions/${p.id}${p.next != null ? `?from=${p.next}` : ''}`, () => {
    p.row = { ...p.row, reach: 'The session has ended', via: null };
  });
  if (reading === run) reading = 0;
  if (P !== p || run !== pollRun) return;
  if (r && ((p.next != null && r.next < p.next) || r.reset || r.session.session_id !== p.row.session_id)) {
    // The session went on in another transcript, or ran far ahead: draw it again.
    keep.drop(`session-${p.id}`);
    return openPage('session', p.id);
  }
  pollTimer = setTimeout(poll, document.hidden ? 8000 : 2000);
  if (!r) return composerState();
  p.next = r.next;
  p.row = { ...p.row, ...r.session, reach: r.reach, via: r.via };
  add(r.events);
  p.renderer.live(r.doing && { ...r.doing, output: r.live?.output });
  question(r.ask);
  head();
  if (r.session?.session_id) markSeen(r.session);
  stick();
  if (again) {
    again = false;
    poll();
  }
}
document.addEventListener('visibilitychange', () => { if (!document.hidden) poll(); });

// The server saw the session's transcript grow: read on now rather than at the next poll.
export function sessionEvent(ev) {
  if (P?.kind !== 'session' || String(P.id) !== String(ev.pid)) return;
  if (reading && reading === pollRun) again = true;
  else poll();
}

setInterval(() => {
  if (!P) return;
  P.renderer.tick();
}, 1000);

export function sheetEvent(ev) {
  if ((P?.kind !== 'worker' && P?.kind !== 'thread') || P.id !== ev.worker) return;
  add([ev.ev]);
  if (P.kind === 'thread') head();
  stick();
}

// What the user saw of an open thread, told to the server a moment later (its unread count and push go by it).
let seenTimer = 0;
function seenSoon() {
  clearTimeout(seenTimer);
  seenTimer = setTimeout(() => {
    if (P?.kind === 'thread' && P.id && !document.hidden && !sp.hidden) threadSeen(P.id, P.last);
  }, 800);
}
document.addEventListener('visibilitychange', () => { if (!document.hidden && P?.kind === 'thread') seenSoon(); });

function details() {
  const r = P.row;
  const body = [];
  const titles = el('div', 'dhead');
  titles.append(el('h2', null, r.name), el('div', 'sub', P.kind === 'worker' ? 'Shikigami' : `Session ${r.kind || ''}`.trim()));
  body.push(titles);
  // A permission card it waits on.
  if (P.kind === 'worker' && r.card) {
    const w = el('div', 'dwait');
    w.append(svg(LOCK, 16), el('span', null, `Needs you: ${r.card.replace(/^wants to /, '').split(':')[0]}`));
    body.push(w);
  }
  // Facts as a quiet key/value list; what can be done sits below as buttons.
  const facts = el('dl', 'kv');
  const fact = (k, v, cls) => v && facts.append(el('dt', null, k), el('dd', cls, v));
  // Model, effort and permissions of a live shikigami open the Change sheet.
  const tunable = P.kind === 'worker' && !['released', 'laptop'].includes(r.state);
  const tune = (k, v) => {
    if (!tunable) return fact(k, v);
    const dd = el('dd', 'tune sl');
    dd.append(el('span', null, v || 'default'), svg(CHEV, 16));
    dd.tabIndex = 0;
    dd.addEventListener('click', () => changeSheet(r));
    facts.append(el('dt', 'tune', k), dd);
  };
  fact('Folder', home(r.cwd), 'mono');
  tune('Model', r.model);
  const effort = r.effort || r.effort_now;
  tune('Effort', r.effort_next ? `${effort || 'default'} → ${r.effort_next} at its next turn`
    : effort && `${effort}${r.effort || P.kind !== 'worker' ? '' : ' · default'}`);
  if (P.kind === 'worker') tune('Permissions', MODE_LABEL[r.mode] || r.mode);
  fact('Compacts at', r.context_cap);
  fact('Subagents', r.subagents);
  fact('Context', r.context != null && `${r.context}% used`);
  fact(P.kind === 'worker' ? 'Started' : 'Last active', cap(atTime(P.kind === 'worker' ? r.created : r.active).replace(/^at /, 'today at ')));
  // What its run took (crates/core/src/metrics.rs): fetched when the sheet opens, since pricing it takes the server a moment.
  if (P.kind === 'worker') {
    const id = P.id;
    getJSON(`api/workers/${encodeURIComponent(id)}/metrics`).then((m) => {
      if (m?.metrics && P?.id === id && facts.isConnected) for (const [k, v] of metricFacts(m.metrics)) fact(k, v);
    });
  }
  body.push(facts);
  const actions = el('div', 'dactions');
  if (P.kind === 'worker' && !['laptop', 'released'].includes(r.state) && r.started !== false) actions.append(laptopButton(r));
  if (P.kind === 'worker' && ACTIVE.has(r.state)) actions.append(stopButton(r));
  const x = P.kind === 'worker' ? fromWorker(r) : fromSession(r);
  if (x.state === 'done' || x.state === 'idle') {
    const b = el('button', 'dbtn tonal sl');
    b.type = 'button';
    b.append(svg(ARCH, 18), el('span', null, r.archived ? 'Unarchive' : 'Archive'));
    b.addEventListener('click', () => { closeSheet(); setArchived(x, !r.archived); });
    actions.append(b);
  }
  if (actions.childElementCount) body.push(actions);
  if (P.kind === 'worker' && r.task) body.push(taskRow(r.task, r.now));
  if (P.kind === 'session') body.push(el('p', 'usage-note', r.via === 'seimei' ? 'seimei writes messages to its Claude Code inbox: this terminal can\'t be typed into' : r.reach || 'Typed into its terminal on the server'));
  sheet.body.replaceChildren(...body);
  showSheet('details');
}

const LOCK = 'M7 11V8a5 5 0 0 1 10 0v3M6 11h12a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1z';
const CHEV = 'M9 6l6 6-6 6';
const EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max'];
const MODELS = ['opus', 'sonnet', 'haiku'];
const MODE_LABEL = { plan: 'Plan first', default: 'Ask first', acceptEdits: 'Allow edits', auto: 'Auto', dontAsk: "Only what's allowed", bypassPermissions: 'Bypass' };
const MODE_NOTE = { acceptEdits: 'Commands still ask', auto: 'Claude Code decides', dontAsk: 'Never asks', bypassPermissions: 'Deny rules still apply' };

// One sheet for effort, model and permissions: only the user changes these here (seimei's tune tool does effort and model).
function changeSheet(w) {
  const pick = { effort: w.effort_next || w.effort || w.effort_now || 'high', model: w.model, mode: w.mode };
  const seg = (key, values) => {
    const row = el('div', 'seg');
    for (const v of values) {
      const b = el('button', `sl${pick[key] === v ? ' on' : ''}`, v);
      b.type = 'button';
      b.addEventListener('click', () => { pick[key] = v; row.querySelectorAll('button').forEach((x) => x.classList.toggle('on', x === b)); });
      row.append(b);
    }
    return row;
  };
  const modes = el('div', 'radios');
  const shown = ['plan', 'default', 'acceptEdits', 'bypassPermissions'];
  if (!shown.includes(w.mode)) shown.splice(3, 0, w.mode);
  for (const m of shown) {
    const b = el('button', `radio sl${pick.mode === m ? ' on' : ''}`);
    b.type = 'button';
    const t = el('span', 'rt');
    t.append(el('b', null, MODE_LABEL[m] || m), el('small', null, MODE_NOTE[m] || ''));
    b.append(el('span', 'rdot'), t);
    b.addEventListener('click', () => { pick.mode = m; modes.querySelectorAll('.radio').forEach((x) => x.classList.toggle('on', x === b)); });
    modes.append(b);
  }
  const err = el('p', 'snote error');
  err.hidden = true;
  const cancel = el('button', 'btn text sl', 'Cancel');
  const apply = el('button', 'btn filled sl', 'Apply');
  cancel.type = apply.type = 'button';
  cancel.addEventListener('click', () => details());
  apply.addEventListener('click', async () => {
    const body = {};
    if (pick.effort !== (w.effort_next || w.effort || w.effort_now)) body.effort = pick.effort;
    if (pick.model !== w.model) body.model = pick.model;
    if (pick.mode !== w.mode) body.mode = pick.mode;
    if (!Object.keys(body).length) return details();
    apply.disabled = true;
    const r = await postJSON(`api/workers/${encodeURIComponent(w.id)}/tune`, body);
    apply.disabled = false;
    if (!r.ok) {
      err.textContent = r.error || "Couldn't change it";
      err.hidden = false;
      return;
    }
    P.row = { ...P.row, ...r };
    details();
  });
  const btns = el('div', 'dbtns');
  btns.append(cancel, apply);
  sheet.body.replaceChildren(el('h2', 'change-h', `Change ${w.name}`),
    el('p', 'lab', 'Effort'), seg('effort', EFFORTS), el('p', 'lab', 'Model'), seg('model', MODELS),
    el('p', 'hint', 'From its next turn. A new model rebuilds the cache once.'),
    el('p', 'lab', 'Permissions'), modes, err, btns);
  showSheet('change');
}

function stopButton(w) {
  const b = el('button', 'dbtn stop sl');
  b.type = 'button';
  const sq = el('span', 'square');
  b.append(sq, el('span', null, `Stop ${w.name}`));
  b.addEventListener('click', async () => {
    b.disabled = true;
    await postJSON(`api/workers/${encodeURIComponent(w.id)}/stop`, {});
    closeSheet();
  });
  return b;
}

function laptopButton(w) {
  const b = el('button', 'dbtn sl');
  b.type = 'button';
  const label = el('span', null, 'Open in a terminal');
  b.append(svg(ICON.laptop, 16), label);
  let armed = false;
  b.addEventListener('click', async () => {
    const cur = S.workers.find((x) => x.id === w.id) || w;
    // A running worker gets stopped first, so ask for a second tap.
    if (ACTIVE.has(cur.state) && !armed) {
      armed = true;
      label.textContent = 'Stop it and open in a terminal';
      return;
    }
    b.disabled = true;
    const r = await postJSON(`api/workers/${encodeURIComponent(w.id)}/laptop`, {});
    b.disabled = false;
    armed = false;
    if (r.ok) {
      b.hidden = true;
    } else {
      label.textContent = 'Open in a terminal';
      sheet.body.append(el('p', 'snote error', `Couldn't open ${w.name} on the server: ${r.error}`));
    }
  });
  return b;
}

// The task as one tonal row with a preview; tapping it unfolds the whole text.
// Its board item now when it has one; the summon text, its first task, opens under it.
function taskRow(text, now) {
  const box = el('div', 'taskd');
  const headBtn = el('button', 'taskd-head sl');
  headBtn.type = 'button';
  const ic = el('span', 'ic');
  ic.append(svg(ICON.list, 18));
  const txt = el('span', 'txt');
  txt.append(el('b', null, now ? `Now on ${now.id}` : 'Task'), el('span', 'preview', now ? now.title : text));
  const chev = svg(ICON.chev, 20);
  chev.classList.add('chev');
  headBtn.append(ic, txt, chev);
  const full = el('div', 'full', now ? `${now.title}\n\nFirst task: ${text}` : text);
  full.hidden = true;
  headBtn.addEventListener('click', () => {
    full.hidden = !full.hidden;
    box.classList.toggle('open', !full.hidden);
  });
  box.append(headBtn, full);
  return box;
}

function build(kind, id, row) {
  const bar = el('header', 'sbar');
  const back = el('button', 'back sl');
  back.type = 'button';
  back.setAttribute('aria-label', 'Back');
  back.append(svg('M15 5l-7 7 7 7', 22));
  back.addEventListener('click', () => history.back());
  // News in seimei's chat while away from it, as the navigation's dot was
  const dot = el('i', 'ndot');
  dot.hidden = true;
  back.append(dot);
  const titles = el('div', 'titles');
  const title = el('h1');
  const sub = el('div', 'sub');
  titles.append(title, sub);
  const r = el('span');
  // One tap each way between seimei and the user: Take over on a shikigami, Hand off on a session.
  const hand = el('button', 'handoff sl');
  hand.type = 'button';
  hand.hidden = true;
  hand.addEventListener('click', handoff);
  bar.append(back, titles, hand, r);

  const main = el('main', 'stl');
  const thread = el('div', 'sthread');
  const empty = el('div', 'note', 'Nothing yet');
  thread.append(empty);
  const qbox = el('section', 'qcard');
  qbox.hidden = true;
  qbox.setAttribute('aria-label', 'Question');
  main.append(thread, qbox);

  const noteEl = el('p', 'snote');
  noteEl.hidden = true;
  const trayEl = el('div', 'attachments');
  trayEl.hidden = true;
  const form = el('form', 'scomposer');
  const field = el('div', 'field');
  const attach = el('button', 'attach sl');
  attach.type = 'button';
  attach.setAttribute('aria-label', 'Attach photos or files');
  attach.append(svg('M20 11.5l-7.8 7.8a5 5 0 0 1-7-7l8.5-8.5a3.3 3.3 0 0 1 4.7 4.7l-8.5 8.5a1.7 1.7 0 0 1-2.4-2.4l7.8-7.8', 22));
  const input = el('textarea');
  input.rows = 1;
  input.setAttribute('enterkeyhint', 'send');
  input.setAttribute('aria-label', 'Message');
  const file = el('input');
  file.type = 'file';
  file.multiple = true;
  file.hidden = true;
  field.append(attach, input);
  const send = el('button', 'fab sl');
  send.type = 'submit';
  send.setAttribute('aria-label', 'Send');
  send.append(svg(ICON.send, 20));
  const stop = el('button', 'fab tonal sl');
  stop.type = 'button';
  stop.setAttribute('aria-label', 'Stop');
  stop.append(el('span', 'square'));
  stop.hidden = true;
  form.append(field, file, send, stop);
  const foot = el('div', 'sfoot');
  // A draft thread about a message of seimei's shows which one
  const quote = el('div', 'squote');
  quote.hidden = true;
  if (row.quote) quote.append(svg('M9 14 4 9l5-5M4 9h10.5a5.5 5.5 0 0 1 0 11H11', 16), el('span', null, row.quote.text.replace(/\s+/g, ' ').trim()));
  foot.append(noteEl, quote, trayEl, form);
  const banner = el('div', 'archbar');
  banner.hidden = true;
  sp.replaceChildren(bar, banner, main, foot);

  P = { kind, id, row, title, sub, ring: r, hand, banner, dot, quote, main, thread, empty, qbox, note: noteEl, tray: trayEl, input, attach, send, stop,
    files: [], next: null, ask: null, askKey: undefined, pinned: true, renderer: new Renderer(thread), ids: new Set(), raw: [], last: 0, pending: [] };

  main.addEventListener('scroll', () => { if (P) P.pinned = nearBottom(); }, { passive: true });
  new ResizeObserver(() => stick()).observe(thread);
  attach.addEventListener('click', () => file.click());
  file.addEventListener('change', (e) => { addFiles([...e.target.files]); e.target.value = ''; });
  input.addEventListener('input', () => { autosize(); sendState(); });
  input.addEventListener('paste', (e) => {
    const fs = [...(e.clipboardData?.files || [])];
    if (fs.length) {
      e.preventDefault();
      addFiles(fs);
    }
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && finePointer) {
      e.preventDefault();
      form.requestSubmit();
    }
  });
  form.addEventListener('submit', submit);
  stop.addEventListener('click', () => postJSON(kind === 'thread' ? `api/threads/${P.id}/stop` : `api/workers/${encodeURIComponent(id)}/stop`, {}));
}

export async function openPage(kind, id) {
  const reopen = P && P.kind === kind && P.id === id;
  // The page as last seen draws at once; the server then sends only what came since.
  const copy = keep.read(`${kind}-${id}`);
  const kept = kind === 'session' && copy?.shape !== SHAPE ? null : copy;
  const live = kind === 'worker' ? S.workers.find((x) => x.id === id) : kind === 'thread' ? S.threads.find((x) => x.id === id) : null;
  if (kept?.events) {
    build(kind, id, { ...kept.row, ...live });
    P.next = kept.next ?? null;
    add(kept.events);
    shown(reopen);
  }
  const p = P;
  const path = kind === 'worker' || kind === 'thread'
    ? `api/${kind}s/${encodeURIComponent(id)}${kept ? `?after=${kept.last || 0}` : ''}`
    : `api/sessions/${id}${kept?.next != null ? `?from=${kept.next}` : ''}`;
  let gone = false;
  let r = await getJSON(path, () => { gone = true; });
  // A pid the system gave to another session since: that one is new here.
  if (r && kind === 'session' && kept?.next != null && kept.row?.session_id && r.session.session_id !== kept.row.session_id) {
    r = await getJSON(`api/sessions/${id}`);
    if (r) r.reset = true;
  }
  if (kept && P !== p) return;
  if (!r && kept && gone) return note('Gone from the server: this is the last copy');
  if (!r) {
    // The copy, or the page being drawn again, keeps polling; the stream's reconnect refreshes a worker's page.
    if (kind === 'session' && P?.kind === kind && P.id === id) pollTimer = setTimeout(poll, 2000);
    return snack(kept ? 'Offline: this is the last copy' : "Couldn't reach the server");
  }
  // Drawn anew when there was no copy, or the server says the copy is too old to extend.
  const rec = r.worker || r.thread;
  if (!kept || r.reset) build(kind, id, kind !== 'session' ? { ...live, ...rec } : { ...r.session, reach: r.reach, via: r.via });
  else P.row = kind !== 'session' ? { ...P.row, ...live, ...rec } : { ...P.row, ...r.session, reach: r.reach, via: r.via };
  if (kind !== 'session') add(r.events);
  else {
    // A poll still on its way read from the old offset.
    pollRun++;
    P.next = r.next;
    add(r.events);
    P.renderer.live(r.doing && { ...r.doing, output: r.live?.output });
    question(r.ask);
    if (r.session.session_id) markSeen(r.session);
    changed();
    clearTimeout(pollTimer);
    pollTimer = setTimeout(poll, 2000);
  }
  if (!kept || r.reset) shown(reopen);
  else {
    head();
    stick();
  }
}

// Forward and backward: the page comes in from the end edge and goes back out to it.
// Focus goes to its title, and on close back to the row that opened it.
let closing = null;
let opener = null;
function shown(reopen) {
  head();
  closing?.cancel();
  closing = null;
  if (sp.hidden) {
    opener = document.activeElement;
    sp.hidden = false;
    enter(sp, 'translateX(100%)');
    P?.title?.setAttribute('tabindex', '-1');
    P?.title?.focus({ preventScroll: true });
  }
  sp.hidden = false;
  document.body.classList.add('spage-open');
  if (!reopen && !history.state?.spage) history.pushState({ spage: true }, '');
  requestAnimationFrame(() => stick(true));
}

// Back online: the open page fetches what came while the stream was down.
export async function refreshPage() {
  if (P?.kind === 'session') return poll();
  if (!P) return;
  const p = P;
  if (!p.id) return;
  const r = await getJSON(`api/${p.kind}s/${encodeURIComponent(p.id)}?after=${p.last}`);
  if (!r || P !== p) return;
  p.row = p.kind === 'thread' ? { ...p.row, ...r.thread } : { ...p.row, ...r.worker, ...S.workers.find((x) => x.id === p.id) };
  add(r.events);
  head();
  stick();
}

export const openWorker = (id) => openPage('worker', id);
export const openThread = (id) => openPage('thread', id);
export const pageKind = () => (sp.hidden ? null : P?.kind);

// A new side thread, from the top bar or about a message of seimei's (quote: its event id and text): a page with no thread yet.
export function openDraft(quote = null) {
  build('thread', null, { name: '', model: '', state: 'new', quote });
  shown(false);
  requestAnimationFrame(() => P?.input.focus());
}
export const pageOpen = () => !sp.hidden;

export function closePage() {
  clearTimeout(pollTimer);
  clearTimeout(seenTimer);
  closeSheet(true);
  P?.files.forEach((a) => a.url && URL.revokeObjectURL(a.url));
  P = null;
  const done = () => {
    closing = null;
    sp.hidden = true;
    sp.replaceChildren();
    document.body.classList.remove('spage-open');
    if (opener?.isConnected) opener.focus({ preventScroll: true });
    opener = null;
  };
  if (sp.hidden) return done();
  const out = leave(sp, 'translateX(100%)');
  closing = out;
  out.onfinish = () => { if (closing === out) done(); };
}
