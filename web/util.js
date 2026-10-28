// Helpers and the state every part of the page shares.

export const $ = (id) => document.getElementById(id);
export const finePointer = matchMedia('(pointer: fine)').matches;
const lessMotion = matchMedia('(prefers-reduced-motion: reduce)');
export const reduced = () => lessMotion.matches;
// Screen transitions (BEHAVIOR.md 1) use M3's emphasized easings, not springs; reduced motion gets a short fade
const EASE_IN = 'cubic-bezier(.05, .7, .1, 1)';
const EASE_OUT = 'cubic-bezier(.3, 0, .8, .15)';
export function enter(node, from, ms = 300) {
  // A leave holds its end state (fill forwards) until the node is hidden: drop it, or it covers this enter
  node._leave?.cancel();
  if (reduced()) return node.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 150 });
  return node.animate([{ transform: from }, { transform: 'none' }], { duration: ms, easing: EASE_IN });
}
export function leave(node, to, ms = 200) {
  node._leave = reduced() ? node.animate([{ opacity: 1 }, { opacity: 0 }], { duration: 100, fill: 'forwards' })
    : node.animate([{ transform: 'none' }, { transform: to }], { duration: ms, easing: EASE_OUT, fill: 'forwards' });
  return node._leave;
}

// The one quiet live region (BEHAVIOR.md 6): a short line when a card arrives, a reply finishes or a card settles
export function say(text) {
  const r = document.getElementById('say');
  if (!r || !text) return;
  r.textContent = '';
  requestAnimationFrame(() => { r.textContent = text; });
}

// An M3 dialog for what can't be undone: a headline that asks, a body, Cancel and a verb
export function ask({ headline, body, confirm }) {
  return new Promise((done) => {
    const d = el('dialog', 'dlg');
    d.setAttribute('aria-labelledby', 'dlg-h');
    const h = el('h2', null, headline);
    h.id = 'dlg-h';
    const acts = el('div', 'acts');
    const no = el('button', 'btn text sl', 'Cancel');
    const yes = el('button', 'btn text sl', confirm);
    no.type = yes.type = 'button';
    acts.append(no, yes);
    d.append(h, el('p', null, body), acts);
    const back = document.activeElement;
    const close = (v) => { d.close(); d.remove(); back?.focus?.(); done(v); };
    no.addEventListener('click', () => close(false));
    yes.addEventListener('click', () => close(true));
    d.addEventListener('cancel', (e) => { e.preventDefault(); close(false); });
    d.addEventListener('click', (e) => { if (e.target === d) close(false); });
    document.body.append(d);
    d.showModal();
    no.focus();
  });
}

// Live state; app.js routes stream events into it and the views read it.
export const S = {
  online: false,
  awaySince: null,
  session: { connected: false, busy: false },
  status: { s: 'connecting', text: 'connecting' },
  tab: 'chat',
  unread: false,
  workers: [],
  workersTotal: 0,
  sessions: [],
  usage: null,
};

// Set by app.js: redraw the header and the navigation bar after state changes.
export const hooks = { chrome: () => {} };

export function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

export function svg(path, size = 16) {
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

export const clock = (t) => new Date(t * 1000).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
export const dayKey = (t) => new Date(t * 1000).toDateString();
export const home = (p) => (p || '').replace(/^\/home\/[^/]+/, '~');
// A shikigami's identity: one shape of the M3 Expressive library (m3x.css), the same for a name everywhere, and a
// phase, so working tiles don't morph in step. The order and the salt put the names of final/ on their shapes.
const SHAPES = ['cookie4', 'cookie12', 'cookie6', 'sunny', 'cookie9', 'flower', 'clover8', 'clover4', 'puffy-diamond', 'gem', 'pentagon'];
function nameHash(name) {
  let h = 0;
  for (const c of name || '') h = (h * 101 + c.charCodeAt(0)) >>> 0;
  return h;
}
const shapeOf = (name) => SHAPES[nameHash(name) % SHAPES.length];
function phaseOf(name) {
  let x = (nameHash(name) ^ 736562) >>> 0;
  x = (x ^ (x >>> 16)) >>> 0;
  x = Math.imul(x, 0x45d9f3b) >>> 0;
  x = (x ^ (x >>> 16)) >>> 0;
  return (x % 96) * 50;
}
export function wearShape(node, name) {
  node.style.setProperty('--me', `var(--shape-${shapeOf(name)})`);
  // Into the 4.8 s morph cycle of a working tile; the turn keeps its own time
  node.style.animationDelay = `-${phaseOf(name)}ms, 0ms`;
}
export const plural = (n, one, many) => (n === 1 ? one : `${n} ${many}`);

export function dayLabel(t) {
  const d = new Date(t * 1000);
  const yesterday = new Date(Date.now() - 864e5);
  if (d.toDateString() === new Date().toDateString()) return 'Today';
  if (d.toDateString() === yesterday.toDateString()) return 'Yesterday';
  return d.toLocaleDateString([], { weekday: 'long', day: 'numeric', month: 'long' });
}

export const short = (t) => {
  const s = Math.max(0, Date.now() / 1000 - t);
  return s < 60 ? 'now' : s < 3600 ? `${Math.floor(s / 60)}m` : s < 86400 ? `${Math.floor(s / 3600)}h` : `${Math.floor(s / 86400)}d`;
};

// A clock time today, a date before that.
// "at 10:28 PM" today, "on 18 Oct at 10:28 PM" another day.
export function at(t) {
  const d = new Date(t * 1000);
  if (d.toDateString() === new Date().toDateString()) return `at ${clock(t)}`;
  return `on ${d.toLocaleDateString([], { day: 'numeric', month: 'short' })} at ${clock(t)}`;
}

export function when(t) {
  const d = new Date(t * 1000);
  return d.toDateString() === new Date().toDateString() ? clock(t) : d.toLocaleDateString([], { day: 'numeric', month: 'short' });
}

// A session that ended (expired, logged out elsewhere, revoked on the server) sends the page back to login.
export const toLogin = () => location.reload();

// ---------- conversations kept in this browser ----------
// The chat and the agent pages last seen, so the next visit draws them at once and fetches only what is new. The
// login page clears them (logging out, or a session that ended, leads there).
const KEEP = 'seimei.keep.';
const PAGES = 20;
const saves = {};
export const keep = {
  read(key) {
    try { return JSON.parse(localStorage.getItem(KEEP + key)); } catch { return null; }
  },
  // Written a moment after the last change; a full storage drops the oldest pages first.
  later(key, data) {
    clearTimeout(saves[key]);
    saves[key] = setTimeout(() => keep.write(key, data()), 1200);
  },
  write(key, data) {
    const index = keep.read('index') || {};
    index[key] = Date.now();
    const pages = Object.keys(index).filter((k) => k !== 'chat').sort((a, b) => index[b] - index[a]);
    for (const k of pages.slice(PAGES)) keep.drop(k, index);
    for (;;) {
      try {
        localStorage.setItem(KEEP + key, JSON.stringify(data));
        localStorage.setItem(KEEP + 'index', JSON.stringify(index));
        return;
      } catch {
        const old = Object.keys(index).filter((k) => k !== key && k !== 'chat').sort((a, b) => index[a] - index[b])[0];
        if (!old) return;
        keep.drop(old, index);
      }
    }
  },
  drop(key, index = null) {
    try { localStorage.removeItem(KEEP + key); } catch {}
    if (index) delete index[key];
  },
};

export async function post(path, body) {
  const r = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (r.status === 401) toLogin();
  if (!r.ok) throw new Error(`${r.status}`);
  return r.json();
}

// A short note at the bottom for taps that could not do anything.
let snackTimer = 0;
// A short message at the bottom; action ({ label, run }) adds a button such as Undo.
// The snackbar sits above whatever holds the bottom edge, never over the field: an agent page's composer, or the
// composer and navigation bar (the rail layout has only the composer there).
function snackPlace() {
  const s = $('snack');
  const page = document.querySelector('#spage:not([hidden]) .scomposer');
  const bottom = document.querySelector('body > .bottom');
  const base = page || (getComputedStyle(bottom).display === 'contents' ? $('composer') : bottom);
  const top = base && base.offsetParent !== null || base === bottom ? base.getBoundingClientRect().top : innerHeight;
  // final/ 19 and 20: 44 over the composer, 32 over the navigation bar
  const gap = page || $('composer').offsetParent ? 44 : 32;
  s.style.bottom = `${Math.max(16, innerHeight - top + gap)}px`;
}
// The keyboard, the navigation bar stepping aside or a page opening move that edge while the bar shows
addEventListener('resize', () => { if (!$('snack').hidden) snackPlace(); });
new ResizeObserver(() => { if (!$('snack').hidden) snackPlace(); }).observe(document.querySelector('body > .bottom'));

function unsnack() {
  $('snack').hidden = true;
  document.body.classList.remove('snacked');
}

export function snack(text, action = null) {
  const s = $('snack');
  s.replaceChildren(el('span', null, text));
  if (action) {
    const b = el('button', 'sl', action.label);
    b.type = 'button';
    b.addEventListener('click', () => { unsnack(); action.run(); });
    s.append(b);
  }
  s.hidden = false;
  // The thread makes room, so the bar never hides the end of it (a card's buttons)
  const log = $('log');
  const end = log.scrollHeight - log.scrollTop - log.clientHeight < 80;
  document.body.classList.add('snacked');
  if (end) log.scrollTop = log.scrollHeight;
  snackPlace();
  clearTimeout(snackTimer);
  // One with an action stays until it's used or the next one replaces it (M3: actionable snackbars don't auto-dismiss)
  if (!action) snackTimer = setTimeout(unsnack, 4000);
}

// gone runs on a 404: the server no longer has the thing (not the same as offline).
export async function getJSON(path, gone) {
  const r = await fetch(path).catch(() => null);
  if (r?.status === 401) toLogin();
  if (r?.status === 404) gone?.();
  return r?.ok ? r.json() : null;
}

// ---------- full-screen pictures: the chat-sized one at once, the original once loaded ----------

const viewer = $('viewer');
const vimg = $('viewer-img');
const vsave = $('viewer-save');
let zoom = { s: 1, x: 0, y: 0 };
let token = 0;

function place() {
  vimg.style.transform = `translate(${zoom.x}px, ${zoom.y}px) scale(${zoom.s})`;
  viewer.classList.toggle('zoomed', zoom.s > 1);
}

// Zoom by f around the point (px, py) of the viewport, keeping that point still.
function zoomAt(f, px, py) {
  const s = Math.min(8, Math.max(1, zoom.s * f));
  const r = viewer.getBoundingClientRect();
  const cx = px - r.left - r.width / 2;
  const cy = py - r.top - r.height / 2;
  const k = s / zoom.s;
  zoom = s === 1 ? { s: 1, x: 0, y: 0 } : { s, x: cx - (cx - zoom.x) * k, y: cy - (cy - zoom.y) * k };
  place();
}

/** src: shown at once; full: the original, swapped in once loaded; save: its download link. */
export function view(src, { full = null, save = null } = {}) {
  const mine = ++token;
  zoom = { s: 1, x: 0, y: 0 };
  place();
  vimg.src = src;
  vsave.hidden = !save;
  if (save) vsave.href = save;
  // Its own step of history, so the phone's back button closes it instead of leaving the page.
  if (viewer.hidden) history.pushState({ viewer: true }, '');
  viewer.hidden = false;
  if (full && full !== src) {
    const big = new Image();
    big.onload = () => { if (mine === token && !viewer.hidden) vimg.src = full; };
    big.src = full;
  }
}

export const viewing = () => !viewer.hidden;

export function closeView(fromHistory = false) {
  if (viewer.hidden) return;
  viewer.hidden = true;
  token++;
  if (!fromHistory && history.state?.viewer) history.back();
}
const close = () => closeView();

const pts = new Map();
let pinch = null;
let moved = false;
vimg.addEventListener('pointerdown', (e) => {
  vimg.setPointerCapture(e.pointerId);
  pts.set(e.pointerId, { x: e.clientX, y: e.clientY });
  moved = false;
  if (pts.size === 2) {
    const [a, b] = [...pts.values()];
    pinch = { d: Math.hypot(a.x - b.x, a.y - b.y), s: zoom.s };
  }
});
vimg.addEventListener('pointermove', (e) => {
  const was = pts.get(e.pointerId);
  if (!was) return;
  const now = { x: e.clientX, y: e.clientY };
  pts.set(e.pointerId, now);
  if (Math.hypot(now.x - was.x, now.y - was.y) > 2) moved = true;
  if (pts.size === 2 && pinch) {
    const [a, b] = [...pts.values()];
    zoomAt(pinch.s * Math.hypot(a.x - b.x, a.y - b.y) / pinch.d / zoom.s, (a.x + b.x) / 2, (a.y + b.y) / 2);
  } else if (pts.size === 1 && zoom.s > 1) {
    zoom.x += now.x - was.x;
    zoom.y += now.y - was.y;
    place();
  }
});
const lift = (e) => {
  pts.delete(e.pointerId);
  if (pts.size < 2) pinch = null;
};
vimg.addEventListener('pointerup', lift);
vimg.addEventListener('pointercancel', lift);
vimg.addEventListener('dblclick', (e) => zoomAt(zoom.s > 1 ? 1 / zoom.s : 2.5, e.clientX, e.clientY));
viewer.addEventListener('wheel', (e) => {
  e.preventDefault();
  zoomAt(Math.exp(-e.deltaY / 300), e.clientX, e.clientY);
}, { passive: false });
// A tap on the backdrop closes, and so does one on the unzoomed picture; a drag never does.
viewer.addEventListener('click', (e) => {
  if (e.target.closest('.viewer-bar')) return;
  if (e.target === vimg && (zoom.s > 1 || moved)) return;
  close();
});
$('viewer-close').addEventListener('click', close);
document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && !viewer.hidden) close(); });

export const ICON = {
  check: 'M5 12.5l4.5 4.5L19 7.5',
  chev: 'M9 6l6 6-6 6',
  send: 'M12 19V5M5.5 11.5 12 5l6.5 6.5',
  laptop: 'M5 6.5A1.5 1.5 0 0 1 6.5 5h11A1.5 1.5 0 0 1 19 6.5V15H5zM3 18h18',
  list: 'M8 7h11M8 12h11M8 17h7M4.5 7h.01M4.5 12h.01M4.5 17h.01',
  save: 'M12 4v11M7 10.5l5 5 5-5M5 19.5h14',
  close: 'M6 6l12 12M18 6L6 18',
};

vsave.append(svg(ICON.save, 20));
$('viewer-close').append(svg(ICON.close, 20));
