// The Usage tab: Claude rate-limit windows, today's tokens, and the API-price equivalent; its ring is the tab's icon.
import { $, S, hooks, el, svg, clock, getJSON, ICON } from './util.js';

const page = $('usage');
const WINDOWS = [['five_hour', '5-hour'], ['seven_day', 'Weekly']];

export const tone = (u) => (u >= 0.95 ? 'bad' : u >= 0.8 ? 'warn' : 'ok');
const tokens = (n) => (n >= 1e9 ? `${(n / 1e9).toFixed(1)}B` : n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : `${n}`);

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
  body.push(costSection());
  page.replaceChildren(...body);
  loadCost();
}

// ---------- what the tokens would cost at API prices ----------

let cost = null;
let costAt = 0;
const open = new Set();
// Cents only below $100: past that they are noise (QA D13).
const usd = (n) => `$${n >= 100 ? Math.round(n).toLocaleString('en-US') : n.toFixed(2)}`;
const MODEL = (k) => k.replace(/^claude-/, '').replace(/-(\d+)-(\d+)$/, ' $1.$2').replace(/-(\d+)$/, ' $1')
  .replace(/^./, (c) => c.toUpperCase());

async function loadCost() {
  if (Date.now() - costAt < 15000) return;
  costAt = Date.now();
  const r = await getJSON('api/cost');
  if (!r?.all) return;
  cost = r;
  if (S.tab === 'usage') page.querySelector('.cost')?.replaceWith(costSection());
}

function costSection() {
  const box = el('div', 'cost');
  box.append(el('h3', 'usage-h', 'At API prices'));
  if (!cost) {
    box.append(el('p', 'usage-note', 'Adding up…'));
    return box;
  }
  const since = new Date(cost.week_start * 1000).toLocaleString([], { weekday: 'short', hour: '2-digit', minute: '2-digit' });
  const list = el('div', 'ulist');
  for (const [key, label, sub] of [['day', 'Today', ''], ['week', 'This week', `since ${since}`], ['all', 'All time', '']]) {
    const p = cost[key];
    const row = el('button', `crow sl${open.has(key) ? ' open' : ''}`);
    row.type = 'button';
    const chev = svg(ICON.chev, 18);
    chev.classList.add('chev');
    row.append(el('b', null, label), el('span', 'amt', usd(p.usd)), chev,
      el('small', null, [sub, `${p.messages.toLocaleString('en-US')} API calls`].filter(Boolean).join(' · ')));
    const models = el('div', 'cmodels');
    models.hidden = !open.has(key);
    for (const [m, v] of Object.entries(p.models).sort((a, b) => b[1].usd - a[1].usd)) {
      const r = el('div', 'cmodel');
      r.append(el('span', null, MODEL(m)), el('span', 'amt', usd(v.usd)),
        el('small', null, `${tokens(v.output)} out · ${tokens(v.input)} in · ${tokens(v.cache_read)} cache read · ${tokens(v.cache_write)} cache write`));
      models.append(r);
    }
    if (p.unpriced.length) models.append(el('p', 'usage-note', `No price for ${p.unpriced.join(', ')}; counted as $0.`));
    row.addEventListener('click', () => {
      if (open.has(key)) open.delete(key);
      else open.add(key);
      row.classList.toggle('open');
      models.hidden = !models.hidden;
    });
    list.append(row, models);
  }
  box.append(list, el('p', 'usage-note',
    `What this laptop's Claude Code use would cost on pay-as-you-go API prices (checked ${cost.prices_checked}). Other machines on the account are not included.`));
  return box;
}
