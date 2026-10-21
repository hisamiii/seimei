// The Tasks lane: seimei's board by project. Each project is a section with a thin rule in its Agents-tab tint;
// done items fold into one "N done" line, paused projects stay shut until opened. The user ticks their own ("you")
// items; a task with a shikigami opens its page. Live: the stream sends the whole board on every change.
import { $, el, post, keep, snack } from './util.js';
import { openWorker, tint } from './agents.js';

let board = keep.read('tasks') || { tasks: [], projects: [] };
const OPEN = 'seimei.tb.open';
// A project's section opened or shut by hand (name -> bool); others follow their default (paused ones shut).
let opened = {};
try { opened = JSON.parse(localStorage.getItem(OPEN) || '{}'); } catch {}
const doneShown = new Set();
const root = () => $('tasks');

export function setTasks(b) {
  board = { tasks: b.tasks || [], projects: b.projects || [] };
  keep.write('tasks', board);
  if (!root().hidden) renderTasks();
}

export async function loadTasks() {
  const r = await fetch('api/tasks').catch(() => null);
  if (r?.ok) setTasks(await r.json());
}

const NS = 'http://www.w3.org/2000/svg';
function glyph(state) {
  const s = document.createElementNS(NS, 'svg');
  s.setAttribute('width', '18');
  s.setAttribute('height', '18');
  s.setAttribute('viewBox', '0 0 18 18');
  s.setAttribute('aria-hidden', 'true');
  s.innerHTML = state === 'done'
    ? '<path d="M4 9.5l3.2 3.2L14 5.8" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>'
    : '<circle cx="9" cy="9" r="6.5" fill="none" stroke="currentColor" stroke-width="1.6"/>'
      + (state === 'running' ? '<path d="M9 4.5a4.5 4.5 0 0 1 0 9z" fill="currentColor"/>' : '')
      + (state === 'paused' ? '<path d="M7.3 6.5v5M10.7 6.5v5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>' : '');
  return s;
}

function chevron(open) {
  const s = document.createElementNS(NS, 'svg');
  s.setAttribute('width', '18');
  s.setAttribute('height', '18');
  s.setAttribute('viewBox', '0 0 24 24');
  s.setAttribute('aria-hidden', 'true');
  s.innerHTML = `<path d="${open ? 'M6 9l6 6 6-6' : 'M9 6l6 6-6 6'}" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>`;
  return s;
}

// What a project's header says on the right, and where it sorts: running work first, paused last.
function summary(p, ts) {
  const live = ts.filter((t) => t.state !== 'done');
  if (p.paused) return ['paused', 4];
  if (live.some((t) => t.state === 'running')) return ['running', 0];
  if (live.length && live.every((t) => t.owner === 'you')) return ['you', 2];
  if (live.length) return ['open', 1];
  return ['done', 3];
}

function line(t) {
  const you = t.owner === 'you';
  const row = el(t.worker ? 'button' : 'div', `ln s-${t.state}`);
  if (t.worker) {
    row.type = 'button';
    row.addEventListener('click', () => openWorker(t.worker));
  }
  let g;
  if (you) {
    // The user's own item: the circle ticks it off, the check takes it back.
    g = el('button', `g s-${t.state} tick`);
    g.type = 'button';
    g.setAttribute('aria-label', t.state === 'done' ? `Reopen ${t.title}` : `Tick off ${t.title}`);
    g.addEventListener('click', (e) => { e.stopPropagation(); tick(t); });
  } else g = el('span', `g s-${t.state}`);
  g.append(glyph(t.state));
  const title = el('span', 't');
  if (t.id.startsWith('#')) title.append(el('span', 'id', t.id));
  title.append(t.title);
  row.append(g, title, el('span', you ? 'o you' : 'o', t.owner));
  return row;
}

async function tick(t) {
  const done = t.state !== 'done';
  // Shown at once; the board that comes back over the stream confirms it.
  t.state = done ? 'done' : 'open';
  t.done_at = done ? Date.now() / 1000 : null;
  renderTasks();
  try {
    await post(`api/tasks/${encodeURIComponent(t.id.replace(/^#/, ''))}/tick`, { done });
  } catch {
    snack('Could not reach the laptop.');
    loadTasks();
  }
}

export function renderTasks() {
  const box = root();
  const order = { running: 0, open: 1, paused: 2 };
  const projects = board.projects.map((p, i) => {
    const ts = board.tasks.filter((t) => t.project === p.name);
    const [label, rank] = summary(p, ts);
    return { p, ts, label, rank, i };
  }).filter((x) => x.ts.length).sort((a, b) => a.rank - b.rank || a.i - b.i);
  const out = [];
  for (const { p, ts, label } of projects) {
    const open = opened[p.name] ?? !p.paused;
    const sec = el('section', `pj${p.paused ? ' held' : ''}${open ? '' : ' shut'}`);
    sec.style.setProperty('--c', p.color || tint(p.name));
    const head = el('button', 'ph sl');
    head.type = 'button';
    head.setAttribute('aria-expanded', String(open));
    const cv = el('span', 'cv');
    cv.append(chevron(open));
    head.append(cv, el('b', null, p.name), el('small', label === 'you' ? 'you' : null, label));
    head.addEventListener('click', () => {
      opened[p.name] = !open;
      try { localStorage.setItem(OPEN, JSON.stringify(opened)); } catch {}
      renderTasks();
    });
    sec.append(head);
    if (open) {
      const live = ts.filter((t) => t.state !== 'done').sort((a, b) => order[a.state] - order[b.state]);
      for (const t of live) sec.append(line(t));
      const done = ts.filter((t) => t.state === 'done').sort((a, b) => (b.done_at || 0) - (a.done_at || 0));
      if (done.length) {
        if (doneShown.has(p.name)) for (const t of done) sec.append(line(t));
        const fold = el('button', 'fold3 sl', doneShown.has(p.name) ? 'hide done' : `${done.length} done`);
        fold.type = 'button';
        const g = el('span', 'g s-done');
        g.append(glyph('done'));
        fold.prepend(g);
        fold.addEventListener('click', () => {
          if (doneShown.has(p.name)) doneShown.delete(p.name);
          else doneShown.add(p.name);
          renderTasks();
        });
        sec.append(fold);
      }
    }
    out.push(sec);
  }
  if (!out.length) out.push(el('p', 'tasks-empty', 'Nothing on the board. seimei adds what it queues here.'));
  box.replaceChildren(...out);
}
