// The page: the stream from the laptop, the four tabs, and the header and navigation bar around them.
import { $, S, hooks, finePointer } from './util.js';
import { render, onDelta, clearLive, addEvent, loadHistory, autosize, waitingApprovals, toBottom } from './chat.js';
import { setWorkers, setSessions, sheetEvent, agentCounts, showAgents, pageOpen, closePage } from './agents.js';
import { showScreen, hideScreen } from './screen.js';
import { setUsage, renderUsage, tone } from './usage.js';
import { account } from './account.js';
import { sheet, closeSheet } from './sheet.js';

let lastId = 0;

// ---------- tabs ----------

const TABS = ['chat', 'agents', 'screen', 'usage'];
const TITLES = { chat: 'seimei', agents: 'Agents', screen: 'Screen', usage: 'Usage' };
const pages = { chat: $('log'), agents: $('agents'), screen: $('screen'), usage: $('usage') };

function show(tab) {
  if (!TABS.includes(tab)) tab = 'chat';
  const was = S.tab;
  S.tab = tab;
  for (const t of TABS) pages[t].hidden = t !== tab;
  document.body.dataset.tab = tab;
  for (const b of document.querySelectorAll('.nav-item')) {
    const on = b.dataset.tab === tab;
    b.classList.toggle('on', on);
    if (on) b.setAttribute('aria-current', 'page');
    else b.removeAttribute('aria-current');
  }
  if (was === 'screen' && tab !== 'screen') hideScreen();
  if (tab === 'chat') {
    S.unread = false;
    if (was !== 'chat') requestAnimationFrame(toBottom);
  } else if (tab === 'agents') showAgents();
  else if (tab === 'screen') showScreen();
  else if (tab === 'usage') renderUsage();
  chrome();
}

// Chat is home: other tabs sit one step of history above it, so the phone's back button returns to the chat.
function go(tab) {
  if (tab === S.tab) {
    pages[tab].scrollTo({ top: tab === 'chat' ? pages[tab].scrollHeight : 0, behavior: 'smooth' });
    return;
  }
  if (tab === 'chat') {
    if (history.state?.tab) history.back();
    else {
      history.replaceState(null, '', location.pathname);
      show('chat');
    }
    return;
  }
  if (S.tab === 'chat') history.pushState({ tab }, '', `#${tab}`);
  else history.replaceState({ tab }, '', `#${tab}`);
  show(tab);
}

for (const b of document.querySelectorAll('.nav-item')) b.addEventListener('click', () => go(b.dataset.tab));

window.addEventListener('popstate', () => {
  if (!sheet.wrap.hidden) return closeSheet(true);
  if (pageOpen()) return closePage();
  show(location.hash.slice(1) || 'chat');
});

// ---------- header and navigation bar ----------

function chrome() {
  const { working, asks } = agentCounts();
  let s = 'idle', text = '';
  if (S.tab === 'chat') ({ s, text } = S.status);
  else if (S.tab === 'agents') {
    s = asks ? 'waiting' : working ? 'working' : 'idle';
    text = [working && `${working} working`, asks && `${asks} ${asks === 1 ? 'needs' : 'need'} you`].filter(Boolean).join(' · ') || 'nothing running';
  }
  $('title').textContent = TITLES[S.tab];
  $('status').dataset.s = s;
  $('status-text').textContent = text;
  // Idle and working go without saying in the chat (its working line shows a turn); only problems and asks show.
  $('status').hidden = !text || (S.tab === 'chat' && (s === 'idle' || s === 'working'));

  const count = $('nav-count');
  count.hidden = !working;
  count.textContent = working > 99 ? '99+' : working;
  count.classList.toggle('attn', asks > 0);
  $('nav-agents-dot').hidden = !(asks && !working);
  $('nav-chat-dot').hidden = !(S.tab !== 'chat' && (S.unread || waitingApprovals().length));
  const label = [working && `${working} working`, asks && `${asks} waiting for you`].filter(Boolean).join(', ');
  $('nav-agents').setAttribute('aria-label', label ? `Agents, ${label}` : 'Agents');

  const w = S.usage?.windows?.five_hour;
  const ring = $('nav-ring');
  ring.setAttribute('stroke-dasharray', `${w ? Math.round(w.utilization * 100) : 0} 100`);
  $('nav-usage').dataset.tone = !w ? '' : S.usage.status === 'rejected' ? 'bad' : tone(w.utilization);
  $('nav-usage').setAttribute('aria-label', w ? `Usage, 5-hour window ${Math.round(w.utilization * 100)}% used` : 'Usage');
}
hooks.chrome = chrome;

// The keyboard covers the bottom of a phone screen: while typing, the navigation bar steps aside.
if (!finePointer) {
  const composer = $('composer');
  composer.addEventListener('focusin', () => document.body.classList.add('typing'));
  composer.addEventListener('focusout', () => document.body.classList.remove('typing'));
}

// ---------- stream ----------

function handle(ev) {
  if (ev.id) {
    if (ev.id <= lastId) return;
    lastId = ev.id;
    addEvent(ev);
    if (S.tab !== 'chat' && /^(assistant|approval|worker|image)$/.test(ev.type)) S.unread = true;
    render();
  } else if (ev.type === 'state') {
    S.session = ev;
    if (!ev.busy) clearLive();
    render();
  } else if (ev.type === 'delta') {
    onDelta(ev.text);
  } else if (ev.type === 'workers') {
    setWorkers(ev);
  } else if (ev.type === 'w') {
    sheetEvent(ev);
  } else if (ev.type === 'usage') {
    setUsage(ev);
  } else if (ev.type === 'sessions') {
    setSessions(ev.list);
  }
}

// ---------- connection ----------

let source = null;
let awayTimer = 0;
let lastSeen = 0;
const SILENT = 40000;
const GRACE = 8000;
let quietUntil = 0;

// A phone pauses a background tab and its stream drops; that is not the laptop sleeping.
// Losses while hidden or during a quiet reconnect only count once that reconnect fails.
function lost() {
  if (S.online && !document.hidden && Date.now() > quietUntil) S.awaySince = Date.now();
  S.online = false;
  render();
  clearTimeout(awayTimer);
  awayTimer = setTimeout(render, 4500);
}

function connect() {
  source?.close();
  lastSeen = Date.now();
  source = new EventSource(`api/stream?after=${lastId}`);
  source.onopen = () => {
    S.online = true;
    S.awaySince = null;
    lastSeen = Date.now();
    render();
  };
  source.onmessage = (e) => {
    lastSeen = Date.now();
    handle(JSON.parse(e.data));
  };
  source.onerror = () => {
    lost();
    // The browser retries on its own unless the server refused the request outright,
    // and EventSource cannot see why, so ask whether the session is still valid.
    if (source.readyState === EventSource.CLOSED) {
      fetch('api/auth/me')
        .then((r) => (r.status === 401 ? location.reload() : setTimeout(connect, 10000)))
        .catch(() => setTimeout(connect, 10000));
    }
  };
}

// The server pings every 15 s. A suspended laptop never closes the connection, so silence is the signal.
function watchdog() {
  if (document.hidden) return;
  if (Date.now() - lastSeen > SILENT) {
    lost();
    connect();
  }
}
setInterval(watchdog, 5000);

// Back in front: reconnect quietly, and show the away card only if the laptop does not answer.
function resume() {
  if (S.online && Date.now() - lastSeen < SILENT) return;
  const since = Date.now();
  quietUntil = since + GRACE;
  S.awaySince = null;
  S.online = false;
  render();
  connect();
  setTimeout(() => {
    if (!S.online && !S.awaySince) {
      S.awaySince = since;
      render();
    }
  }, GRACE);
}
document.addEventListener('visibilitychange', () => { if (!document.hidden) resume(); });
window.addEventListener('pageshow', (e) => { if (e.persisted) resume(); });

async function start() {
  const res = await fetch('api/history?limit=200').catch(() => null);
  if (res?.status === 401) return location.reload();
  const r = res?.ok ? await res.json() : null;
  if (r) {
    loadHistory(r);
    lastId = r.last;
    S.session = r.state;
    setWorkers({ list: r.workers, total: r.workers.length });
  }
  autosize();
  show(location.hash.slice(1) || 'chat');
  render();
  connect();
  account();
}

start();
