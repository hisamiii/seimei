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
export function when(t) {
  const d = new Date(t * 1000);
  return d.toDateString() === new Date().toDateString() ? clock(t) : d.toLocaleDateString([], { day: 'numeric', month: 'short' });
}

// A session that ended (expired, logged out elsewhere, revoked on the laptop) sends the page back to login.
export const toLogin = () => location.reload();

export async function post(path, body) {
  const r = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (r.status === 401) toLogin();
  if (!r.ok) throw new Error(`${r.status}`);
  return r.json();
}

export async function getJSON(path) {
  const r = await fetch(path).catch(() => null);
  if (r?.status === 401) toLogin();
  return r?.ok ? r.json() : null;
}

const viewer = $('viewer');
export function view(src) {
  $('viewer-img').src = src;
  viewer.hidden = false;
}
viewer.addEventListener('click', () => { viewer.hidden = true; });
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') viewer.hidden = true; });

export const ICON = {
  star: 'M12 2 17.88 20.09 2.49 8.91h19.02L6.12 20.09Z',
  check: 'M5 12.5l4.5 4.5L19 7.5',
  chev: 'M9 6l6 6-6 6',
  send: 'M12 19V5M5.5 11.5 12 5l6.5 6.5',
  laptop: 'M5 6.5A1.5 1.5 0 0 1 6.5 5h11A1.5 1.5 0 0 1 19 6.5V15H5zM3 18h18',
  list: 'M8 7h11M8 12h11M8 17h7M4.5 7h.01M4.5 12h.01M4.5 17h.01',
  full: 'M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5',
};
