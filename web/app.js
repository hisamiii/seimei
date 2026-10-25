// The page: the stream from the server, the three tabs, and the header and navigation bar around them.
import { $, S, hooks, clock, finePointer, viewing, closeView, keep, reduced } from './util.js';
import { render, onDelta, clearLive, addEvent, loadHistory, autosize, waitingApprovals, toBottom, showLane, following, nearBottom } from './chat.js';
import { setWorkers, setSessions, sheetEvent, agentCounts, showAgents, pageOpen, closePage, archOpen, closeArch } from './agents.js';
import { setUsage, renderUsage, tone } from './usage.js';
import { setTasks, loadTasks, tasksArchOpen, closeTasksArch } from './tasks.js';
import { account } from './account.js';
import { sheet, closeSheet } from './sheet.js';
import { init as initTheme, phone as phoneTheme } from './theme.js';

let lastId = 0;
// The chat's events as they came (the last KEEP_CHAT), kept in this browser for the next visit.
let raw = [];
const KEEP_CHAT = 300;
// A kept chat further behind than this is fetched anew instead of replayed through the stream.
const REPLAY_MAX = 1000;
// An older copy is not drawn at all: a night brings about a thousand events, and drawing a copy only to reload the
// page would flash.
const KEPT_FOR = 4 * 3600e3;
const saveChat = () => keep.later('chat', () => ({ events: raw.slice(-KEEP_CHAT), at: Date.now() }));

// ---------- tabs ----------

const TABS = ['chat', 'agents', 'usage'];
const TITLES = { chat: 'seimei', agents: 'Agents', usage: 'Usage' };
const pages = { chat: $('log'), agents: $('agents'), usage: $('usage') };

function show(tab) {
  if (!TABS.includes(tab)) tab = 'chat';
  const was = S.tab;
  S.tab = tab;
  for (const t of TABS) pages[t].hidden = t !== tab;
  document.body.dataset.tab = tab;
  // Top level (BEHAVIOR.md 1): the new destination fades in; no slide between nav destinations
  if (was && was !== tab && !reduced()) pages[tab].animate([{ opacity: 0 }, { opacity: 1 }], { duration: 200, easing: 'cubic-bezier(.2, 0, 0, 1)' });
  for (const b of document.querySelectorAll('.nav-item')) {
    const on = b.dataset.tab === tab;
    b.classList.toggle('on', on);
    if (on) b.setAttribute('aria-current', 'page');
    else b.removeAttribute('aria-current');
  }
  if (tab === 'chat') {
    S.unread = false;
    // Back at the bottom only if the reader was following when they left
    if (was !== 'chat' && following()) requestAnimationFrame(toBottom);
    if (S.lane === 't') showLane('t');
  } else if (tab === 'agents') showAgents();
  else if (tab === 'usage') renderUsage();
  chrome();
  lift();
}

// The top bar fills with surface-container once its screen has moved off its rest: the chat off its end, Agents,
// Usage and Tasks off their top (M3 app bars on scroll)
function lift() {
  const sc = pages[S.tab];
  const off = S.tab === 'chat' && S.lane !== 'k' ? !nearBottom() : sc.scrollTop > 0;
  document.body.classList.toggle('lifted', off);
}
for (const t of TABS) pages[t].addEventListener('scroll', lift, { passive: true });

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
  if (viewing()) return closeView(true);
  if (!sheet.wrap.hidden) return closeSheet(true);
  // Closing a sheet over the page goes back to the page's own entry: the page stays.
  if (pageOpen() && !history.state?.spage) return closePage();
  if (pageOpen()) return;
  if (archOpen() && !history.state?.arch) return closeArch();
  if (tasksArchOpen() && !history.state?.tarch) return closeTasksArch();
  show(location.hash.slice(1) || 'chat');
});

// ---------- header and navigation bar ----------

function chrome() {
  const { working, asks } = agentCounts();
  let s = 'idle', text = '';
  if (S.tab === 'chat') ({ s, text } = S.status);
  else if (S.tab === 'agents') {
    // The sections and the nav badge say how many work and need you
    s = 'idle';
  }
  // Away: the other tabs show the last known state, and say so (QA U2).
  if (S.tab !== 'chat' && !S.online && S.awaySince) {
    s = 'away';
    text = `Offline since ${clock(S.awaySince / 1000)}`;
  }
  $('title').textContent = TITLES[S.tab];
  $('status').dataset.s = s;
  $('status-text').textContent = text;
  // Idle and working go without saying in the chat (its working line shows a turn); only problems and asks show.
  $('status').hidden = !text || (S.tab === 'chat' && (s === 'idle' || s === 'working'));

  // What needs the user counts first, in orange; otherwise how many work (QA U4).
  const count = $('nav-count');
  const n = asks || working;
  count.hidden = !n;
  count.textContent = n > 99 ? '99+' : n;
  count.classList.toggle('attn', asks > 0);
  $('nav-chat-dot').hidden = !(S.tab !== 'chat' && (S.unread || waitingApprovals().length));
  const label = [working && `${working} working`, asks && `${asks} ${asks === 1 ? 'needs' : 'need'} you`].filter(Boolean).join(', ');
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
    raw.push(ev);
    if (raw.length > 2 * KEEP_CHAT) raw = raw.slice(-KEEP_CHAT);
    saveChat();
    // The chat dot is for the conversation; the trajectory keeps its own count.
    if (S.tab !== 'chat' && ev.lane !== 't' && /^(assistant|approval|worker|image)$/.test(ev.type)) S.unread = true;
    render();
  } else if (ev.type === 'state') {
    S.session = ev;
    if (!ev.busy) clearLive();
    render();
  } else if (ev.type === 'delta') {
    onDelta(ev.text, ev.lane);
  } else if (ev.type === 'workers') {
    setWorkers(ev);
  } else if (ev.type === 'w') {
    sheetEvent(ev);
  } else if (ev.type === 'usage') {
    setUsage(ev);
  } else if (ev.type === 'sessions') {
    setSessions(ev.list);
  } else if (ev.type === 'tasks') {
    setTasks(ev);
  } else if (ev.type === 'theme') {
    phoneTheme(ev);
  }
}

// ---------- connection ----------

let source = null;
let awayTimer = 0;
let lastSeen = 0;
const SILENT = 40000;
const GRACE = 8000;
let quietUntil = 0;

// A phone pauses a background tab and its stream drops; that is not the server sleeping.
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
    // The board's changes while away went by on the stream: fetch it whole.
    loadTasks();
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

// The server pings every 15 s. A suspended server never closes the connection, so silence is the signal.
function watchdog() {
  if (document.hidden) return;
  if (Date.now() - lastSeen > SILENT) {
    lost();
    connect();
  }
}
setInterval(watchdog, 5000);

// Back in front: reconnect quietly, and show the away card only if the server does not answer.
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
  // The chat as last seen draws at once; the server then sends only what came since (the stream replays it).
  const copy = keep.read('chat');
  const kept = copy && Date.now() - (copy.at || 0) < KEPT_FOR ? copy.events : null;
  if (kept?.length) {
    raw = kept;
    loadHistory({ events: kept });
    lastId = kept.at(-1).id;
    show(location.hash.slice(1) || 'chat');
    render();
  }
  const res = await fetch(kept?.length ? 'api/history?limit=0' : 'api/history?limit=200').catch(() => null);
  if (res?.status === 401) return location.reload();
  const r = res?.ok ? await res.json() : null;
  if (r && kept?.length && (r.last < lastId || r.last - lastId > REPLAY_MAX)) {
    // Too far behind (or another server's log): start from the server's copy.
    keep.drop('chat');
    return location.reload();
  }
  if (r) {
    if (!kept?.length) {
      loadHistory(r);
      lastId = r.last;
      raw = r.events;
      saveChat();
    }
    S.session = r.state;
    setWorkers({ list: r.workers, total: r.workers.length });
  }
  autosize();
  if (!kept?.length) show(location.hash.slice(1) || 'chat');
  render();
  connect();
  account();
}

initTheme();
start();
