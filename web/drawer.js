// The navigation drawer (M3 modal): seimei, the recent side threads, agents and watches, files; usage and settings at its foot.
// The menu icon opens it, and so does a sideways swipe anywhere; the phone's back button closes it.
import { $, S, hooks, el, svg, short, when } from './util.js';
import { fromWorker, fromSession, tile, tint, pageOpen } from './agents.js';
import { THR } from './threads.js';
import { WATCH_ICON, due, loadWatches } from './pages.js';
import { tone } from './usage.js';
import { sheet } from './sheet.js';

const wrap = $('drawer');
const list = $('drawer-list');
const foot = $('drawer-foot');
const CHEV = 'm9 6 6 6-6 6';
const ARCH = 'M3 4h18v4H3zM5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8M10 12h4';
const FILES = 'M5 4h14a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zM4 16l4.5-4.5 4 4 2.5-2.5L20 18M15.5 8.5v.01';
const TUNE = 'M4 7h9M17 7h3M4 17h3M11 17h9M15 5a2 2 0 1 1 0 4 2 2 0 0 1 0-4zM9 15a2 2 0 1 1 0 4 2 2 0 0 1 0-4z';
// How many recent rows each section shows; its header opens the rest.
const RECENT = { threads: 3, agents: 4, watches: 3 };

// Set by app.js: where a pick goes (a page, a thread, the threads archive).
export const nav = { go: () => {}, thread: () => {}, agent: () => {}, threadsArchive: () => {} };

export const drawerOpen = () => !wrap.hidden;

function item(icon, label, { trail = null, on = false, cls = '', run }) {
  const b = el('button', `ditem sl${on ? ' on' : ''}${cls ? ` ${cls}` : ''}`);
  b.type = 'button';
  const lbl = el('span', 'lbl');
  if (typeof label === 'string') lbl.textContent = label;
  else lbl.append(...label);
  b.append(icon, lbl);
  if (trail) b.append(trail);
  if (on) b.setAttribute('aria-current', 'page');
  b.addEventListener('click', () => pick(run));
  return b;
}

function head(label, run) {
  const b = el('button', 'dhd sl');
  b.type = 'button';
  b.append(el('span', null, label), svg(CHEV, 18));
  b.addEventListener('click', () => pick(run));
  return b;
}

function glyph() {
  const g = el('i', 'glyph');
  g.setAttribute('aria-hidden', 'true');
  return g;
}

function usageRing(pct) {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  s.setAttribute('class', 'ring-ico');
  s.setAttribute('width', '24');
  s.setAttribute('height', '24');
  s.setAttribute('viewBox', '0 0 24 24');
  s.setAttribute('aria-hidden', 'true');
  s.innerHTML = `<circle class="t" cx="12" cy="12" r="8.5"/><circle class="f" cx="12" cy="12" r="8.5" pathLength="100" stroke-dasharray="${pct} 100"/>`;
  return s;
}

function miniTile(x) {
  const t = tile(x);
  t.classList.add('mini');
  return t;
}

// The agents the user last heard from: what asks and works first, then the newest.
function recentAgents() {
  // One given to the user (released) is with their sessions now.
  const items = [...S.workers.filter((w) => w.state !== 'released').map(fromWorker), ...S.sessions.map(fromSession)]
    .filter((x) => !x.archived);
  const rank = { ask: 0, work: 1 };
  return items.sort((a, b) => (rank[a.state] ?? 2) - (rank[b.state] ?? 2) || b.since - a.since).slice(0, RECENT.agents);
}

export function renderDrawer() {
  if (wrap.hidden) return;
  const tab = S.tab;
  const parts = [];
  const n = S.unread || 0;
  parts.push(item(glyph(), 'seimei', { on: tab === 'chat', run: () => nav.go('chat'),
    trail: n ? el('i', 'nbadge', n > 99 ? '99+' : String(n)) : null }));
  parts.push(el('hr'));

  parts.push(head('Side threads', () => nav.go('threads')));
  for (const t of S.threads.slice(0, RECENT.threads)) {
    parts.push(item(svg(THR, 22), t.name, { run: () => nav.thread(t.id), trail: t.unread ? el('i', 'ndot') : null }));
  }
  if (S.threadsArchived) parts.push(item(svg(ARCH, 22), 'Archived', { cls: 'darch', run: () => nav.threadsArchive() }));
  parts.push(el('hr'));

  parts.push(head('Agents', () => nav.go('agents')));
  for (const x of recentAgents()) {
    const name = el('b', null, x.name);
    const proj = el('small', null, x.project && x.project !== x.name ? x.project : '');
    if (x.project) proj.style.color = tint(x.project);
    parts.push(item(miniTile(x), [name, proj], { cls: `ag ${x.state}`, run: () => nav.agent(x.kind, x.id),
      trail: el('time', null, x.state === 'done' ? when(x.since) : short(x.since)) }));
  }
  parts.push(el('hr'));

  parts.push(head('Watches', () => nav.go('watches')));
  for (const w of S.watches.slice(0, RECENT.watches)) {
    parts.push(item(svg(WATCH_ICON[w.group] || WATCH_ICON.ping, 22), w.title, { run: () => nav.go('watches'),
      trail: w.group === 'time' ? el('time', null, due(w.at)) : null }));
  }
  parts.push(el('hr'));
  parts.push(item(svg(FILES, 22), 'Files', { on: tab === 'files', run: () => nav.go('files') }));
  list.replaceChildren(...parts);

  const w = S.usage?.windows?.five_hour;
  const pct = w ? Math.round(w.utilization * 100) : 0;
  const t = !w ? '' : S.usage.status === 'rejected' ? 'bad' : tone(w.utilization);
  const usage = item(usageRing(pct), 'Usage', { on: tab === 'usage', cls: t === 'ok' ? '' : t, run: () => nav.go('usage'),
    trail: w ? el('time', null, `${pct}%`) : null });
  usage.dataset.tone = t;
  if (w) usage.setAttribute('aria-label', `Usage, 5-hour window ${pct}% used`);
  foot.replaceChildren(usage, item(svg(TUNE, 22), 'Settings', { on: tab === 'settings', run: () => nav.go('settings') }));
}
hooks.drawer = renderDrawer;

// ---------- open and close ----------

// What a pick does once the drawer's history entry is gone (app.js runs it from popstate).
let after = null;

export function openDrawer() {
  if (!wrap.hidden || pageOpen() || !sheet.wrap.hidden) return;
  wrap.hidden = false;
  history.pushState({ drawer: true }, '');
  renderDrawer();
  loadWatches();
  list.querySelector('.ditem.on, .ditem')?.focus({ preventScroll: true });
}

function pick(run) {
  after = run;
  closeDrawer();
}

export function closeDrawer() {
  if (wrap.hidden) return;
  if (history.state?.drawer) history.back();
  else drawerGone();
}

// popstate: the drawer's entry is gone; then the pick, if any.
export function drawerGone() {
  wrap.hidden = true;
  const run = after;
  after = null;
  // Closed without a pick: focus goes back to the menu icon
  if (run) run();
  else $('drawer-btn').focus({ preventScroll: true });
}

$('drawer-btn').addEventListener('click', openDrawer);
$('drawer-scrim').addEventListener('click', closeDrawer);
document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && !wrap.hidden) closeDrawer(); });

// A sideways swipe anywhere opens it, and one back to the left closes it.
// The screen's edges belong to the phone's back gesture, so the swipe cannot wait for an edge.
// The first clear direction past SLOP decides: mostly sideways and toward the drawer's side is the drawer's, unless what is under the finger takes it.
const SLOP = 10;
const COMMIT = 56;
const HOLD = 500;
let swipe = null;

// Whether something under the finger takes a sideways swipe itself: a box that can still scroll that way, or a task row (a swipe right ticks it).
function takes(el, right) {
  for (; el && el !== document.body; el = el.parentElement) {
    if (right && el.classList.contains('swipe')) return true;
    if (el.scrollWidth > el.clientWidth + 1 && /auto|scroll/.test(getComputedStyle(el).overflowX)) {
      if (right ? el.scrollLeft > 0 : el.scrollLeft + el.clientWidth < el.scrollWidth - 1) return true;
    }
  }
  return false;
}

document.addEventListener('touchstart', (e) => {
  swipe = null;
  if (e.touches.length !== 1) return;
  const open = !wrap.hidden;
  if (!open && (pageOpen() || !sheet.wrap.hidden)) return;
  const p = e.touches[0];
  swipe = { x: p.clientX, y: p.clientY, t: e.timeStamp, el: e.target, open, ours: false };
}, { passive: true });
// Not passive: once the swipe is the drawer's, the page under it does not scroll.
document.addEventListener('touchmove', (e) => {
  if (!swipe) return;
  if (e.touches.length !== 1) {
    swipe = null;
    return;
  }
  const p = e.touches[0];
  const dx = p.clientX - swipe.x, dy = p.clientY - swipe.y;
  if (!swipe.ours) {
    if (Math.hypot(dx, dy) < SLOP) return;
    const right = dx > 0;
    // A held finger is a long press (picking text, a message's menu), not a swipe.
    if (Math.abs(dx) <= Math.abs(dy) || right === swipe.open || e.timeStamp - swipe.t > HOLD || (!swipe.open && takes(swipe.el, right))) {
      swipe = null;
      return;
    }
    swipe.ours = true;
  }
  if (e.cancelable) e.preventDefault();
  if (!swipe.open && dx > COMMIT) {
    swipe = null;
    openDrawer();
  } else if (swipe.open && dx < -COMMIT) {
    swipe = null;
    closeDrawer();
  }
}, { passive: false });
document.addEventListener('touchend', () => { swipe = null; }, { passive: true });
document.addEventListener('touchcancel', () => { swipe = null; }, { passive: true });
