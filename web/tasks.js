// The Tasks lane, two views behind one connected button group. Now: what needs the user (a question, a job to look
// over, their own tasks), what runs and how far, and each shikigami's next. All tasks: one quiet row per project
// with a one-line summary; a project opens in place, its done items fold into "N done", paused projects wait
// behind one row. The user ticks their own tasks (with Undo); a task with a shikigami opens its page. Live: the
// stream sends the whole board on every change.
import { $, el, svg, post, keep, snack, clock, plural, wearShape } from './util.js';
import { openWorker } from './agents.js';

let board = keep.read('tasks') || { tasks: [], projects: [] };
const VIEW = 'seimei.tb.view';
const OPEN = 'seimei.tb.open';
let view = localStorage.getItem(VIEW) === 'all' ? 'all' : 'now';
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

export function setTasks(b) {
  board = { tasks: b.tasks || [], projects: b.projects || [] };
  keep.write('tasks', board);
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

// A row: optional tile, title, supporting line, optional bar
function row(t, kind, parts, { tile = true, run = false, cls = '' } = {}) {
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
  }
  const tx = el('span', 'tx');
  tx.append(el('span', 'tl', t.title), sub(parts, cls ? t : null));
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
const ORDER = { asks: 0, review: 1, running: 2 };

function project(p, live, done, next) {
  const out = [];
  const open = !!opened[p.name];
  const b = el('button', `prow sl${open ? ' open' : ''}`);
  b.type = 'button';
  b.setAttribute('aria-expanded', String(open));
  const tx = el('span', 'tx');
  tx.append(el('span', 'tl', p.name), live.length ? summary(live) : el('span', 'ts', p.paused ? 'paused' : plural(done.length, '1 done', 'done')));
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
  for (const t of [...live].sort((a, b) => (ORDER[a.state] ?? 3) - (ORDER[b.state] ?? 3) || byPos(a, b))) {
    const o = { tile: false, cls: ' in' };
    if (t.state === 'asks') out.push(row(t, 'ask', [...own(t), 'question'], o));
    else if (t.state === 'review') out.push(row(t, 'review', [...own(t), 'review'], o));
    else if (t.state === 'running') out.push(row(t, 'run', [...own(t), steps(t)?.cur || 'running'], { ...o, run: true }));
    else if (t.owner === 'you') out.push(row(t, 'you', [t.state === 'paused' ? 'paused' : 'your task'], o));
    else out.push(row(t, 'open', [...own(t), next.has(t.id) ? el('span', 'nx', 'next') : t.state === 'paused' ? 'paused' : 'open'], o));
  }
  if (done.length) {
    const shown = doneShown.has(p.name);
    if (shown) for (const t of done) out.push(row(t, 'done', [...own(t), 'done'], { tile: false, cls: ' in' }));
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
  }
  return out;
}

function all() {
  const next = new Set(nexts(active()).map((t) => t.id));
  const rows = (ps) => ps.map((p, i) => {
    const ts = board.tasks.filter((t) => t.project === p.name);
    const live = ts.filter((t) => t.state !== 'done');
    const done = ts.filter((t) => t.state === 'done').sort((a, b) => (b.done_at || 0) - (a.done_at || 0));
    return { p, i, live, done, r: live.length ? rank(live) : 4 };
  }).filter((x) => x.live.length || x.done.length)
    .sort((a, b) => a.r - b.r || a.i - b.i)
    .flatMap((x) => project(x.p, x.live, x.done, next));
  const out = [];
  const on = rows(board.projects.filter((p) => !p.paused));
  if (on.length) out.push(group('board', on));
  const held = board.projects.filter((p) => p.paused && board.tasks.some((t) => t.project === p.name));
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
  return out;
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
  if (!board.tasks.length) {
    box.replaceChildren(el('p', 'tasks-empty', 'No tasks'));
    return;
  }
  box.replaceChildren(switcher(), ...(view === 'now' ? now(active()) : all()));
  // One emphasized title on the screen: the running task
  box.querySelector('.tr.k-run .tl')?.classList.add('emph');
}
