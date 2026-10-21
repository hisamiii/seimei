// Helpers and the state every part of the page shares.

export const $ = (id) => document.getElementById(id);
export const finePointer = matchMedia('(pointer: fine)').matches;

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

export const clock = (t) => new Date(t * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
export const dayKey = (t) => new Date(t * 1000).toDateString();
export const home = (p) => (p || '').replace(/^\/home\/[^/]+/, '~');
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
// "at 10:28 PM" today, "on 18 Oct at 10:28 PM" another day (QA U8).
export function at(t) {
  const d = new Date(t * 1000);
  if (d.toDateString() === new Date().toDateString()) return `at ${clock(t)}`;
  return `on ${d.toLocaleDateString([], { day: 'numeric', month: 'short' })} at ${clock(t)}`;
}

export function when(t) {
  const d = new Date(t * 1000);
  return d.toDateString() === new Date().toDateString() ? clock(t) : d.toLocaleDateString([], { day: 'numeric', month: 'short' });
}

// A session that ended (expired, logged out elsewhere, revoked on the laptop) sends the page back to login.
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

// A short note at the bottom for taps that could not do anything (QA U15).
let snackTimer = 0;
// A short message at the bottom; action ({ label, run }) adds a button such as Undo.
export function snack(text, action = null) {
  const s = $('snack');
  s.replaceChildren(el('span', null, text));
  if (action) {
    const b = el('button', 'sl', action.label);
    b.type = 'button';
    b.addEventListener('click', () => { s.hidden = true; action.run(); });
    s.append(b);
  }
  s.hidden = false;
  clearTimeout(snackTimer);
  snackTimer = setTimeout(() => { s.hidden = true; }, action ? 6000 : 4000);
}

export async function getJSON(path) {
  const r = await fetch(path).catch(() => null);
  if (r?.status === 401) toLogin();
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
  star: 'M12 2 17.88 20.09 2.49 8.91h19.02L6.12 20.09Z',
  check: 'M5 12.5l4.5 4.5L19 7.5',
  chev: 'M9 6l6 6-6 6',
  send: 'M12 19V5M5.5 11.5 12 5l6.5 6.5',
  laptop: 'M5 6.5A1.5 1.5 0 0 1 6.5 5h11A1.5 1.5 0 0 1 19 6.5V15H5zM3 18h18',
  list: 'M8 7h11M8 12h11M8 17h7M4.5 7h.01M4.5 12h.01M4.5 17h.01',
  full: 'M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5',
  save: 'M12 4v11M7 10.5l5 5 5-5M5 19.5h14',
  close: 'M6 6l12 12M18 6L6 18',
};

vsave.append(svg(ICON.save, 20));
$('viewer-close').append(svg(ICON.close, 20));
