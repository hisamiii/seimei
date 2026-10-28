// The Tasks lane, two views behind one connected button group. Now: what needs the user (a question, a job to look
// over, their own tasks), what runs and how far, and each shikigami's next. All tasks: one quiet row per project
// with a one-line summary; a project opens in place, its done items fold into "N done", paused projects wait
// behind one row, and done work sits in the Archived row: what is older than a day leaves the board, projects
// with only done items fold away into it. The user ticks their own tasks (with Undo); a task with a shikigami opens
// its page. Live: the stream sends the whole board on every change.
import { $, el, svg, post, keep, snack, clock, plural, wearShape, getJSON, dayLabel } from './util.js';
import { openWorker, marked } from './agents.js';

let board = keep.read('tasks') || { tasks: [], projects: [], archived: 0 };
const VIEW = 'seimei.tb.view';
const OPEN = 'seimei.tb.open';
let view = 'now';
try { if (localStorage.getItem(VIEW) === 'all') view = 'all'; } catch {}
// Projects opened by hand on All tasks (name -> true)
let opened = {};
try { opened = JSON.parse(localStorage.getItem(OPEN) || '{}'); } catch {}
const doneShown = new Set();
let moreMine = false;
let pausedShown = false;
// When Needs you last emptied on this screen: All clear says since when
let clearSince = null;
const root = () => $('tasks');

const ASK = 'M4 5h16v11H9l-5 4zM10 9.2a2 2 0 1 1 2.6 1.9c-.4.2-.6.5-.6.9M12 13.6v.1';
const REVIEW = 'M9 4.5H6.5A1.5 1.5 0 0 0 5 6v13.5A1.5 1.5 0 0 0 6.5 21h11a1.5 1.5 0 0 0 1.5-1.5V6a1.5 1.5 0 0 0-1.5-1.5H15'
  + 'M9.8 3h4.4a.8.8 0 0 1 .8.8v1.4a.8.8 0 0 1-.8.8H9.8a.8.8 0 0 1-.8-.8V3.8a.8.8 0 0 1 .8-.8zM8.5 13.5l2.5 2.5 4.5-4.5';
const CHECK = 'M5 12.5l4.5 4.5L19 7.5';
const RIGHT = 'M9 6l6 6-6 6';
const DOWN = 'M6 9l6 6 6-6';
const ARCH = 'M3 4h18v4H3zM5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8M10 12h4';
// A paused item nobody touched this long gets flagged; seimei asks the user whether it is dead (the server's PAUSED_STALE)
const STALE = 3 * 86400;

export function setTasks(b) {
  const moved = (b.archived || 0) !== board.archived;
  board = { tasks: b.tasks || [], projects: b.projects || [], archived: b.archived || 0 };
  keep.write('tasks', board);
  if (archView && moved) loadArch();
  if (!root().hidden) renderTasks();
}

export async function loadTasks() {
  const r = await fetch('api/tasks').catch(() => null);
  if (r?.ok) setTasks(await r.json());
}

const byPos = (a, b) => (a.pos || 0) - (b.pos || 0);
const mine = (t) => t.owner === 'you' && t.state === 'open';

// Live tasks of projects that aren't paused
function active() {
  const held = new Set(board.projects.filter((p) => p.paused).map((p) => p.name));
  return board.tasks.filter((t) => t.state !== 'done' && t.state !== 'paused' && !held.has(t.project));
}

// Each shikigami's next: its first open task in queue order
function nexts(ts) {
  const seen = new Set();
  const out = [];
  for (const t of [...ts].sort(byPos)) {
    if (t.state === 'open' && t.owner !== 'you' && !seen.has(t.owner)) {
      seen.add(t.owner);
      out.push(t);
    }
  }
  return out;
}

function steps(t) {
  const ss = t.steps || [];
  if (!ss.length) return null;
  const done = ss.filter((s) => s.state === 'completed').length;
  const cur = ss.find((s) => s.state === 'in_progress')?.title;
  return { done, n: ss.length, cur };
}

function bar(t) {
  const si = steps(t);
  const b = el('span', 'tb');
  const prog = el('span', si ? 'prog' : 'prog ind');
  const i = el('i');
  if (si) i.style.width = `${Math.round((100 * si.done) / si.n)}%`;
  else i.style.width = '0%';
  prog.append(i);
  b.append(prog);
  if (si) b.append(el('span', 'pn', `${si.done} of ${si.n}`));
  return b;
}

// The board id ends the supporting line: there when you need to say "#62", out of the way of the titles
function sub(parts, t) {
  const s = el('span', 'ts');
  const all = [...parts];
  if (t?.id?.startsWith('#')) all.push(el('span', 'ref', t.id));
  all.forEach((p, i) => {
    if (i) s.append(' · ');
    s.append(p);
  });
  return s;
}

// Days since a parked item (paused, or of a paused project) was touched, when that is STALE or more
function stale(t) {
  const parked = t.state === 'paused' || board.projects.some((p) => p.paused && p.name === t.project);
  const idle = Date.now() / 1000 - Math.max(t.since || 0, t.updated || 0);
  return t.state !== 'done' && parked && idle >= STALE ? Math.floor(idle / 86400) : 0;
}

const untouched = (days) => el('span', 'sw', `untouched ${days} days`);

// A row: optional tile, title, supporting line, optional bar
function row(t, kind, parts, { tile = true, run = false, cls = '', q = '' } = {}) {
  const r = el('div', `tr k-${kind}${cls}`);
  if (t.worker) {
    r.classList.add('go');
    r.tabIndex = 0;
    r.setAttribute('role', 'button');
    r.addEventListener('click', () => openWorker(t.worker));
    r.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openWorker(t.worker); } });
  }
  if (tile && kind === 'ask') {
    const tt = el('span', 'tt ask');
    wearShape(tt, t.owner);
    tt.append(svg(ASK, 20));
    r.append(tt);
  } else if (tile && kind === 'review') {
    const tt = el('span', 'tt review');
    wearShape(tt, t.owner);
    tt.append(svg(REVIEW, 20));
    r.append(tt);
  } else if (t.owner === 'you') {
    // The user's own task: the circle ticks it off, the check takes it back
    const done = t.state === 'done';
    const tt = el('button', `tt you${done ? ' done' : ''}`);
    tt.type = 'button';
    tt.setAttribute('role', 'checkbox');
    tt.setAttribute('aria-checked', String(done));
    tt.setAttribute('aria-label', done ? `Reopen ${t.title}` : `Tick off ${t.title}`);
    if (done) tt.append(svg(CHECK, 20));
    tt.addEventListener('click', (e) => { e.stopPropagation(); tick(t); });
    r.append(tt);
    if (!done) swipeToTick(r, t);
  }
  const tx = el('span', 'tx');
  const tl = el('span', 'tl');
  tl.append(...marked(t.title, q));
  tx.append(tl, sub(parts, cls ? t : null));
  if (run) tx.append(bar(t));
  r.append(tx);
  return r;
}

function group(cls, nodes) {
  const g = el('div', cls ? `tgrp ${cls}` : 'tgrp');
  g.append(...nodes);
  return g;
}

function head(text, n, cls) {
  const h = el('h2', cls ? `tg ${cls}` : 'tg', text);
  if (n != null) h.append(' ', el('small', null, String(n)));
  return h;
}

function now(ts) {
  const out = [];
  const asks = ts.filter((t) => t.state === 'asks').sort(byPos);
  const review = ts.filter((t) => t.state === 'review').sort(byPos);
  const yours = ts.filter(mine).sort(byPos);
  const running = ts.filter((t) => t.state === 'running').sort(byPos);
  const needs = asks.length + review.length + yours.length;
  if (needs) {
    clearSince = null;
    const rows = [
      ...asks.map((t) => row(t, 'ask', [t.owner, 'question'])),
      ...review.map((t) => row(t, 'review', [t.owner, 'review'])),
      ...(moreMine ? yours : yours.slice(0, 1)).map((t) => row(t, 'you', ['your task'])),
    ];
    if (yours.length > 1 && !moreMine) {
      const more = el('button', 'tmore sl', `${yours.length - 1} more of yours`);
      more.type = 'button';
      more.addEventListener('click', () => { moreMine = true; renderTasks(); });
      rows.push(more);
    }
    out.push(head('Needs you', needs, 'ask'), group('needs', rows));
  } else {
    clearSince ??= Math.max(0, ...board.tasks.map((t) => t.done_at || 0)) || Date.now() / 1000;
    const c = el('div', 'allclear');
    const shape = el('span', 'shape');
    const ic = svg(CHECK, 24);
    ic.setAttribute('stroke-width', '2.4');
    shape.append(ic);
    const txt = el('span', 'txt');
    txt.append(el('b', null, 'All clear'), el('span', null, `Since ${clock(clearSince)}`));
    c.append(shape, txt);
    out.push(c);
  }
  if (running.length) {
    out.push(head('Running', running.length, 'run'),
      group(null, running.map((t) => row(t, 'run', [t.owner, ...(steps(t)?.cur ? [steps(t).cur] : [])], { run: true }))));
  }
  const next = nexts(ts);
  if (next.length) out.push(head('Up next'), group(null, next.map((t) => row(t, 'open', [t.owner]))));
  return out;
}

// A project's summary: words a person would say; what needs the user is warm, what runs is primary
function summary(ts) {
  const n = (f) => ts.filter(f).length;
  const parts = [];
  const asks = n((t) => t.state === 'asks');
  const review = n((t) => t.state === 'review');
  const you = n(mine);
  const running = n((t) => t.state === 'running');
  const open = n((t) => t.state === 'open' && t.owner !== 'you');
  if (asks) parts.push(el('span', 'sw', plural(asks, '1 question', 'questions')));
  if (review) parts.push(el('span', 'sw', `${review} review`));
  if (you) parts.push(el('span', 'sw', `${you} for you`));
  if (running) parts.push(el('span', 'sn', `${running} running`));
  if (open) parts.push(`${open} open`);
  const s = el('span', 'ts');
  parts.forEach((p, i) => {
    if (i) s.append(' · ');
    s.append(p);
  });
  return s;
}

const rank = (ts) => (ts.some((t) => t.state === 'asks') ? 0 : ts.some((t) => t.state === 'review') ? 1
  : ts.some((t) => t.state === 'running') ? 2 : 3);
// Within a project, a timeline: what waits for a check, the current item, the queue, what is parked
const ORDER = { review: 0, asks: 1, running: 1, open: 2 };

function project(p, live, done, next) {
  const out = [];
  const open = !!opened[p.name];
  const b = el('button', `prow sl${open ? ' open' : ''}`);
  b.type = 'button';
  b.setAttribute('aria-expanded', String(open));
  const tx = el('span', 'tx');
  const sum = live.length ? summary(live) : el('span', 'ts', p.paused ? 'paused' : plural(done.length, '1 done', 'done'));
  const days = Math.max(0, ...live.map(stale));
  if (days) sum.append(sum.childNodes.length ? ' · ' : '', untouched(days));
  tx.append(el('span', 'tl', p.name), sum);
  const tc = el('span', 'tc');
  tc.append(svg(open ? DOWN : RIGHT, 20));
  b.append(tx, tc);
  b.addEventListener('click', () => {
    if (open) delete opened[p.name];
    else opened[p.name] = true;
    try { localStorage.setItem(OPEN, JSON.stringify(opened)); } catch {}
    renderTasks();
  });
  out.push(b);
  if (!open) return out;
  const own = (t) => (t.owner === t.project || t.owner === 'you' ? [] : [t.owner]);
  // Done first, quiet and folded, oldest at the top: the timeline runs down into the current item
  if (done.length) {
    const shown = doneShown.has(p.name);
    const f = el('button', 'tfold in sl');
    f.type = 'button';
    f.setAttribute('aria-expanded', String(shown));
    f.append(svg(CHECK, 18), shown ? 'Hide done' : `${done.length} done`);
    f.addEventListener('click', () => {
      if (shown) doneShown.delete(p.name);
      else doneShown.add(p.name);
      renderTasks();
    });
    out.push(f);
    if (shown) for (const t of [...done].reverse()) out.push(row(t, 'done', [...own(t), 'done'], { tile: false, cls: ' in' }));
  }
  for (const t of [...live].sort((a, b) => (ORDER[a.state] ?? 3) - (ORDER[b.state] ?? 3) || byPos(a, b))) {
    const o = { tile: false, cls: ' in' };
    const days = stale(t);
    if (days) {
      out.push(row(t, t.owner === 'you' ? 'you' : 'open', [...own(t), t.state === 'paused' ? 'paused' : t.state, untouched(days)], o));
      continue;
    }
    if (t.state === 'asks') out.push(row(t, 'ask', [...own(t), 'question'], o));
    else if (t.state === 'review') out.push(row(t, 'review', [...own(t), 'review'], o));
    else if (t.state === 'running') out.push(row(t, 'run', [...own(t), steps(t)?.cur || 'running'], { ...o, run: true }));
    else if (t.owner === 'you') out.push(row(t, 'you', [t.state === 'paused' ? 'paused' : 'your task'], o));
    else out.push(row(t, 'open', [...own(t), next.has(t.id) ? el('span', 'nx', 'next') : t.state === 'paused' ? 'paused' : 'open'], o));
  }
  return out;
}

function all() {
  const next = new Set(nexts(active()).map((t) => t.id));
  // A project with only done items folds away into the Archived row
  const rows = (ps) => ps.map((p, i) => {
    const ts = board.tasks.filter((t) => t.project === p.name);
    const live = ts.filter((t) => t.state !== 'done');
    const done = ts.filter((t) => t.state === 'done').sort((a, b) => (b.done_at || 0) - (a.done_at || 0));
    return { p, i, live, done, r: live.length ? rank(live) : 4 };
  }).filter((x) => x.live.length)
    .sort((a, b) => a.r - b.r || a.i - b.i)
    .flatMap((x) => project(x.p, x.live, x.done, next));
  const out = [];
  const on = rows(board.projects.filter((p) => !p.paused));
  if (on.length) out.push(group('board', on));
  const held = board.projects.filter((p) => p.paused && board.tasks.some((t) => t.project === p.name && t.state !== 'done'));
  if (held.length) {
    const b = el('button', 'tall quiet sl');
    b.type = 'button';
    b.setAttribute('aria-expanded', String(pausedShown));
    const s = el('span');
    s.append(el('b', null, plural(held.length, '1 paused project', 'paused projects')), el('small', null, held.map((p) => p.name).join(', ')));
    b.append(s, svg(pausedShown ? DOWN : RIGHT, 20));
    b.addEventListener('click', () => { pausedShown = !pausedShown; renderTasks(); });
    out.push(b);
    if (pausedShown) out.push(group('board held', rows(held)));
  }
  const n = doneCount();
  if (n) out.push(archRow(n));
  return out;
}

// ---------- the archive: every done item, today's from the board and older ones from the server ----------

let archView = false;
let archQuery = '';
let archList = [];

export const tasksArchOpen = () => archView;
const doneCount = () => board.archived + board.tasks.filter((t) => t.state === 'done').length;

async function loadArch() {
  const r = await getJSON('api/tasks/archive');
  if (r) archList = r.tasks || [];
  if (archView) renderTasks();
}

function closeTasksArchQuiet() {
  archView = false;
  archQuery = '';
}

export function closeTasksArch() {
  closeTasksArchQuiet();
  renderTasks();
}

function archRow(n) {
  const b = el('button', 'archrow sl');
  b.type = 'button';
  const t = el('span', 'txt');
  t.append(el('b', null, 'Archived'));
  // The projects folded away say where their done items went
  const gone = board.projects.filter((p) => !board.tasks.some((x) => x.project === p.name && x.state !== 'done'));
  t.append(el('span', null, gone.length ? `${gone.map((p) => p.name).join(', ')}: all done` : `${n} done`));
  const tl = el('span', 'tile done');
  tl.append(svg(ARCH, 20));
  b.append(tl, t, svg(RIGHT, 20));
  b.addEventListener('click', () => {
    archView = true;
    history.pushState({ tarch: true }, '');
    renderTasks();
    root().parentElement.scrollTop = 0;
    loadArch();
  });
  return b;
}

function archPage() {
  const seen = new Set(archList.map((t) => t.id));
  const list = [...board.tasks.filter((t) => t.state === 'done' && !seen.has(t.id)), ...archList]
    .sort((a, b) => (b.done_at || b.updated || 0) - (a.done_at || a.updated || 0));
  const q = archQuery.trim().toLowerCase();
  const hits = q ? list.filter((t) => [t.id, t.title, t.project, t.owner].some((f) => f && f.toLowerCase().includes(q))) : list;
  const head_ = el('div', 'ahead');
  const back = el('button', 'back sl');
  back.type = 'button';
  back.setAttribute('aria-label', 'Back to tasks');
  back.append(svg('M15 5l-7 7 7 7', 22));
  back.addEventListener('click', () => history.back());
  head_.append(back, el('h2', null, 'Archived'), el('span', 'count', q ? `${hits.length} of ${list.length}` : `${list.length}`));
  const search = el('label', 'asearch');
  const input = el('input');
  input.type = 'search';
  input.placeholder = 'Search';
  input.setAttribute('aria-label', 'Search the archive');
  input.value = archQuery;
  input.addEventListener('input', () => {
    archQuery = input.value;
    const pos = input.selectionStart;
    renderTasks();
    const again = root().querySelector('.asearch input');
    again.focus();
    again.setSelectionRange(pos, pos);
  });
  search.append(svg('M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM20 20l-3.5-3.5', 20), input);
  const parts = [head_, search];
  if (!hits.length) parts.push(el('p', 'tasks-empty', q ? 'No matches' : 'Nothing done yet. Done items move here a day after.'));
  // By the day they were done, as the activity log
  let day = null;
  let rows = [];
  const flush = () => { if (rows.length) parts.push(head(day), group(null, rows)); rows = []; };
  for (const t of hits) {
    const d = dayLabel(t.done_at || t.updated);
    if (d !== day) { flush(); day = d; }
    const by = t.owner === 'you' || t.owner === t.project ? [] : [t.owner];
    rows.push(row(t, 'done', [t.project, ...by], { tile: false, cls: ' in', q }));
  }
  flush();
  return parts;
}

// An open task of the user's: a swipe to the right past MARK ticks it off (the circle stays the accessible way).
// Only the row's content moves; the row itself turns into the "Done" container, so the group keeps its corners.
const MARK = 96;
function swipeToTick(r, t) {
  r.classList.add('swipe');
  let start = null;
  let dx = 0;
  let on = false;
  let swipedAt = 0;
  r.addEventListener('pointerdown', (e) => {
    if (e.button) return;
    start = { x: e.clientX, y: e.clientY, id: e.pointerId };
    dx = 0;
    on = false;
  });
  r.addEventListener('pointermove', (e) => {
    if (!start || e.pointerId !== start.id) return;
    const mx = e.clientX - start.x;
    const my = e.clientY - start.y;
    if (!on) {
      // A scroll is not a swipe: the first clear direction decides.
      if (Math.abs(my) > 8 && Math.abs(my) >= Math.abs(mx)) start = null;
      if (!start || mx < 12) return;
      on = true;
      r.setPointerCapture(e.pointerId);
      r.classList.add('swiping');
    }
    const armed = dx >= MARK;
    dx = Math.max(0, mx);
    r.style.setProperty('--dx', `${dx}px`);
    if (dx >= MARK !== armed) {
      r.classList.toggle('armed', dx >= MARK);
      navigator.vibrate?.(8);
    }
  });
  const end = (e) => {
    if (!start) return;
    start = null;
    if (!on) return;
    swipedAt = Date.now();
    r.classList.remove('swiping', 'armed');
    r.style.removeProperty('--dx');
    if (e.type === 'pointerup' && dx >= MARK) tick(t);
  };
  r.addEventListener('pointerup', end);
  r.addEventListener('pointercancel', end);
  // The click that ends a swipe is no tap on the circle.
  r.addEventListener('click', (e) => { if (Date.now() - swipedAt < 400) e.stopPropagation(); }, true);
}

async function tick(t, undo = true) {
  const done = t.state !== 'done';
  // Shown at once; the board that comes back over the stream confirms it
  t.state = done ? 'done' : 'open';
  t.done_at = done ? Date.now() / 1000 : null;
  renderTasks();
  try {
    await post(`api/tasks/${encodeURIComponent(t.id.replace(/^#/, ''))}/tick`, { done });
    if (undo) snack(done ? 'Task done' : 'Task reopened', { label: 'Undo', run: () => tick(t, false) });
    if (archView) loadArch();
  } catch {
    snack("Couldn't reach the server");
    loadTasks();
  }
}

function switcher() {
  const g = el('div', 'tview');
  g.setAttribute('role', 'tablist');
  g.setAttribute('aria-label', 'Tasks view');
  for (const [k, label] of [['now', 'Now'], ['all', 'All tasks']]) {
    const b = el('button', k === view ? 'on' : null, label);
    b.type = 'button';
    b.dataset.view = k;
    b.setAttribute('role', 'tab');
    b.setAttribute('aria-selected', String(k === view));
    b.addEventListener('click', () => {
      if (view === k) return;
      view = k;
      try { localStorage.setItem(VIEW, k); } catch {}
      renderTasks();
      root().querySelector('.tview .on')?.focus();
    });
    g.append(b);
  }
  return g;
}

export function renderTasks() {
  const box = root();
  // Back from the archive while another lane was in front
  if (archView && !history.state?.tarch) closeTasksArchQuiet();
  if (archView) {
    box.replaceChildren(...archPage());
    return;
  }
  if (!board.tasks.length && !board.archived) {
    box.replaceChildren(el('p', 'tasks-empty', 'No tasks'));
    return;
  }
  box.replaceChildren(switcher(), ...(view === 'now' ? now(active()) : all()));
}
