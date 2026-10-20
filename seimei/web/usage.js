// The Usage tab: Claude rate-limit windows and today's tokens; its ring is the tab's icon.
import { $, S, hooks, el, clock } from './util.js';

const page = $('usage');
const WINDOWS = [['five_hour', '5-hour'], ['seven_day', 'Weekly']];

export const tone = (u) => (u >= 0.95 ? 'bad' : u >= 0.8 ? 'warn' : 'ok');
const tokens = (n) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : `${n}`);

function resetText(t) {
  const d = new Date(t * 1000);
  const mins = Math.max(0, Math.round((t * 1000 - Date.now()) / 60000));
  const left = mins < 60 ? `${mins} min` : mins < 2880 ? `${Math.floor(mins / 60)} h ${mins % 60} min` : `${Math.round(mins / 1440)} days`;
  const at = d.toDateString() === new Date().toDateString() ? clock(t)
    : d.toLocaleString([], { weekday: 'short', hour: '2-digit', minute: '2-digit' });
  return `resets ${at}, in ${left}`;
}

export function setUsage(u) {
  S.usage = u;
  hooks.chrome();
  if (S.tab === 'usage') renderUsage();
}

export function renderUsage() {
  const usage = S.usage;
  const body = [];
  for (const [key, label] of WINDOWS) {
    const w = usage?.windows?.[key];
    if (!w) continue;
    const pct = Math.round(w.utilization * 100);
    const meter = el('div', `meter ${tone(w.utilization)}`);
    const top = el('div', 'top');
    top.append(el('b', null, label), el('span', 'pct', `${pct}%`));
    const bar = el('div', 'bar-track');
    const fill = el('i');
    fill.style.width = `${Math.min(pct, 100)}%`;
    bar.append(fill);
    meter.append(top, bar, el('span', 'sub', w.resets_at ? resetText(w.resets_at) : ''));
    body.push(meter);
  }
  if (!body.length) body.push(el('p', 'usage-note', 'No usage reported yet. Claude Code reports it when a session takes a turn.'));
  else if (usage.updated) body.push(el('p', 'usage-note', `Updated ${clock(usage.updated)}. Claude Code reports usage when a session takes a turn.`));
  const rows = Object.entries(usage?.today?.sessions || {}).sort((a, b) => b[1].output - a[1].output);
  if (rows.length) {
    body.push(el('h3', 'usage-h', 'Today in seimei'));
    const list = el('div', 'ulist');
    for (const [name, t] of rows) {
      const row = el('div', 'urow');
      row.append(el('b', null, name), el('time', null, `${t.turns} ${t.turns === 1 ? 'turn' : 'turns'}`),
        el('span', null, `${tokens(t.output)} out · ${tokens(t.input + t.cache_read + t.cache_write)} in`));
      list.append(row);
    }
    body.push(list);
  }
  page.replaceChildren(...body);
}
