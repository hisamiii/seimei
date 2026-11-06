// The drawer's quieter pages: the armed watch rules, and every picture and file shown or sent in the chat.
import { $, S, hooks, el, svg, clock, short, dayKey, dayLabel, getJSON, snack, view } from './util.js';
import { sheet, showSheet, closeSheet } from './sheet.js';
import { fileChip, loader } from './chat.js';
import { postJSON } from './agents.js';

// ---------- watches (crates/core/src/watches.rs rows) ----------

export const WATCH_ICON = {
  time: 'M10 2.5h4M12 14.5V10M12 6.5a7.5 7.5 0 1 1 0 15 7.5 7.5 0 0 1 0-15z',
  exits: 'M4 5h16v14H4zM7.5 9.5 10 12l-2.5 2.5M12.5 15H16',
  reboot: 'M12 3v8M7 6.5a7 7 0 1 0 10 0',
  ping: 'M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15L6 16zM10 20.5a2 2 0 0 0 4 0',
};
const GROUPS = [['time', 'Timers'], ['exits', 'When it exits'], ['reboot', 'After reboot'], ['ping', 'Pings']];

S.watches = [];
let watchFetch = 0;
export async function loadWatches(force = false) {
  if (!force && Date.now() - watchFetch < 3000) return S.watches;
  watchFetch = Date.now();
  const r = await getJSON('api/watches');
  if (r) S.watches = r.watches;
  if (S.tab === 'watches') renderWatches();
  hooks.drawer?.();
  return S.watches;
}

// A timer's time: its clock today, the weekday and clock after.
export function due(t) {
  if (!t) return '';
  const d = new Date(t * 1000);
  if (d.toDateString() === new Date().toDateString()) return clock(t);
  return `${d.toLocaleDateString([], { weekday: 'short' })} ${clock(t)}`;
}

// "in 28m" for a time to come.
function ahead(t) {
  const s = t - Date.now() / 1000;
  if (s <= 0) return 'due';
  return `in ${s < 3600 ? `${Math.max(1, Math.round(s / 60))}m` : s < 86400 ? `${Math.round(s / 3600)}h` : `${Math.round(s / 86400)}d`}`;
}

function watchRow(w) {
  const b = el('button', `agent sl ${w.running ? 'work' : 'done'}`);
  b.type = 'button';
  const t = el('span', w.running ? 'tile sh work' : 'tile done');
  if (w.running) {
    t.style.setProperty('--me', 'var(--shape-cookie9)');
    t.append(loader('bare small'));
  } else t.append(svg(WATCH_ICON[w.group] || WATCH_ICON.ping, 20));
  const top = el('span', 'top');
  // A timer shows when it rings; a command and a reboot note how long ago they were set; a ping has no time of its own
  const when = w.group === 'time' ? due(w.at) : w.at && w.group !== 'ping' ? short(w.at) : '';
  top.append(el('b', null, w.title), el('time', null, when));
  const subText = w.group === 'time' ? [w.sub, w.at && ahead(w.at), w.every && `every ${w.every}`].filter(Boolean).join(' · ') : w.sub;
  const sub = el('span', 'sub');
  sub.append(el('span', 'line', subText || ''));
  const txt = el('span', 'txt');
  txt.append(top, sub);
  b.append(t, txt);
  b.addEventListener('click', () => watchMenu(w));
  return b;
}

function watchMenu(w) {
  const b = el('button', 'mopt sl');
  b.type = 'button';
  const t = el('span', null, 'Remove');
  t.append(el('small', null, 'seimei hears that you removed it'));
  b.append(svg('M5 7h14M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3', 22), t);
  b.addEventListener('click', async () => {
    closeSheet();
    const r = await postJSON(`api/watches/${encodeURIComponent(w.id)}/remove`, {});
    if (!r.ok) return snack(r.error);
    S.watches = S.watches.filter((x) => x.id !== w.id);
    renderWatches();
    hooks.drawer?.();
    snack('Removed');
  });
  sheet.body.replaceChildren(el('div', 'mwho', w.title), b);
  showSheet('rowmenu');
}

export function renderWatches() {
  const page = $('watches');
  const parts = [];
  for (const [g, title] of GROUPS) {
    const rows = S.watches.filter((w) => w.group === g);
    if (!rows.length) continue;
    const sec = el('section', 'agroup');
    sec.append(el('h2', null, title), ...rows.map(watchRow));
    parts.push(sec);
  }
  if (!parts.length) {
    const empty = el('div', 'empty');
    empty.append(el('b', null, 'No watches'), el('span', null, 'Timers and what seimei waits for show here'));
    parts.push(empty);
  }
  page.replaceChildren(...parts);
}

export function showWatches() {
  renderWatches();
  loadWatches(true);
}

// ---------- files (api/gallery) ----------

let gallery = null;

export async function showFiles() {
  if (gallery) renderFiles();
  const r = await getJSON('api/gallery');
  if (!r) return;
  gallery = r.items;
  if (S.tab === 'files') renderFiles();
}

function renderFiles() {
  const page = $('files');
  const parts = [];
  // Each day: its pictures in a grid, then its files
  const days = new Map();
  for (const it of gallery) {
    const k = dayKey(it.t);
    if (!days.has(k)) days.set(k, { t: it.t, images: [], files: [] });
    days.get(k)[it.type === 'image' ? 'images' : 'files'].push(it);
  }
  for (const d of days.values()) {
    parts.push(el('h2', 'fday', dayLabel(d.t)));
    if (d.images.length) {
      const grid = el('div', 'fgrid');
      for (const it of d.images) {
        const img = el('img');
        img.src = `api/files/${it.file}?w=thumb`;
        img.loading = 'lazy';
        img.alt = it.caption || '';
        img.addEventListener('click', () => view(img.src, { full: `api/files/${it.file}`, save: `api/files/${it.file}?download=1` }));
        grid.append(img);
      }
      parts.push(grid);
    }
    if (d.files.length) {
      const list = el('div', 'flist');
      for (const it of d.files) list.append(fileChip({ id: it.file, name: it.name, size: it.bytes, kind: it.kind }, it.t, it.from));
      parts.push(list);
    }
  }
  if (!parts.length) {
    const empty = el('div', 'empty');
    empty.append(el('b', null, 'No files yet'), el('span', null, 'Pictures and files from the chat show here'));
    parts.push(empty);
  }
  page.replaceChildren(...parts);
}
