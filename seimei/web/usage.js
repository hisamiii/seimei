// The Usage tab: Claude rate-limit windows, today's tokens, and the API-price equivalent; its ring is the tab's icon.
import { $, S, hooks, el, svg, clock, getJSON, home, ICON } from './util.js';

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
  body.push(costSection(), metricsSection());
  page.replaceChildren(...body);
  loadCost();
  loadMetrics();
}

// ---------- each shikigami: what it was given, what it cost, how it went ----------

let metrics = null;
let metricsAt = 0;
let allShown = false;
const mopen = new Set();
const SHOWN = 8;

async function loadMetrics() {
  if (Date.now() - metricsAt < 15000) return;
  metricsAt = Date.now();
  const r = await getJSON('api/metrics');
  if (!r?.workers) return;
  metrics = r.workers;
  if (S.tab === 'usage') page.querySelector('.metrics')?.replaceWith(metricsSection());
}

export const span = (sec) => {
  const m = Math.round(sec / 60);
  return sec < 60 ? `${Math.round(sec)} s` : m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${String(m % 60).padStart(2, '0')} min`;
};

// The facts of one shikigami's run, for this list and its details sheet.
export function metricFacts(m) {
  const t = m.tokens;
  const by = [m.followups_by.seimei && `${m.followups_by.seimei} from seimei`, m.followups_by.user && `${m.followups_by.user} from you`];
  const rounds = [m.asks && `${m.asks} ${m.asks === 1 ? 'ask' : 'asks'}`,
    m.followups && `${m.followups} ${m.followups === 1 ? 'follow-up' : 'follow-ups'} (${by.filter(Boolean).join(', ')})`,
    m.reopened && 'reopened'].filter(Boolean).join(' · ');
  return [
    ['Cost', `${usd(m.usd)} at API prices${m.adopted ? ', with its time as your session' : ''}`],
    ['Tokens', `${tokens(t.input)} in · ${tokens(t.cache_read)} cache read · ${tokens(t.cache_write)} cache write · ${tokens(t.output)} out`],
    ['Time', `${span(m.wall)}, busy ${span(m.busy)}`],
    // Asks while it worked, messages after its first done, and whether one asked for more work.
    ['Rounds', rounds || 'one go'],
    // Effort, model or permissions changed on the way (#74), oldest first.
    ...(m.tunes?.length ? [['Changes', m.tunes.map((x) => `${x.what} at ${new Date(x.t * 1000).toTimeString().slice(0, 5)}`).join(' · ')]] : []),
  ];
}

function metricsSection() {
  const box = el('div', 'metrics');
  box.append(el('h3', 'usage-h', 'Shikigami'));
  if (!metrics) {
    box.append(el('p', 'usage-note', 'Adding up…'));
    return box;
  }
  if (!metrics.length) {
    box.append(el('p', 'usage-note', 'No shikigami yet.'));
    return box;
  }
  const finished = metrics.filter((m) => m.first_done);
  const total = metrics.reduce((a, m) => a + m.usd, 0);
  box.append(el('p', 'usage-note', `${metrics.length} shikigami, ${usd(total)} at API prices. `
    + `${finished.filter((m) => m.reopened).length} of ${finished.length} that finished got more work after.`));
  const list = el('div', 'ulist');
  for (const m of allShown ? metrics : metrics.slice(0, SHOWN)) {
    const row = el('button', `crow sl${mopen.has(m.id) ? ' open' : ''}`);
    row.type = 'button';
    const chev = svg(ICON.chev, 18);
    chev.classList.add('chev');
    const how = [MODEL(m.model || ''), m.effort, m.context, span(m.wall), m.state, m.reopened ? 'reopened' : '']
      .filter(Boolean).join(' · ');
    row.append(el('b', null, m.name), el('span', 'amt', usd(m.usd)), chev, el('small', null, how));
    const facts = el('dl', 'kv mfacts');
    facts.hidden = !mopen.has(m.id);
    for (const [k, v] of [['Folder', home(m.cwd)], ...metricFacts(m).slice(1)]) facts.append(el('dt', null, k), el('dd', null, v));
    row.addEventListener('click', () => {
      if (mopen.has(m.id)) mopen.delete(m.id);
      else mopen.add(m.id);
      row.classList.toggle('open');
      facts.hidden = !facts.hidden;
    });
    list.append(row, facts);
  }
  box.append(list);
  if (metrics.length > SHOWN && !allShown) {
    const more = el('button', 'btn text sl', `Show all ${metrics.length}`);
    more.type = 'button';
    more.addEventListener('click', () => { allShown = true; box.replaceWith(metricsSection()); });
    box.append(more);
  }
  return box;
}

// ---------- what the tokens would cost at API prices ----------

let cost = null;
let costAt = 0;
const open = new Set();
// Cents only below $100: past that they are noise (QA D13).
export const usd = (n) => `$${n >= 100 ? Math.round(n).toLocaleString('en-US') : n.toFixed(2)}`;
export const MODEL = (k) => k.replace(/^claude-/, '').replace(/-(\d+)-(\d+)$/, ' $1.$2').replace(/-(\d+)$/, ' $1')
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
