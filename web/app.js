// The page: the stream from the server, the three tabs, and the header and navigation bar around them.
import { $, S, hooks, clock, finePointer, viewing, closeView, keep, reduced } from './util.js';
import { render, onDelta, clearLive, addEvent, loadHistory, autosize, waitingApprovals, toBottom, showLane, following, nearBottom } from './chat.js';
import { setWorkers, setSessions, sheetEvent, sessionEvent, agentCounts, showAgents, pageOpen, closePage, archOpen, closeArch, refreshPage, openPage, openThread, openDraft } from './agents.js';
import { setThreads, showThreads, renderThreads, threadsArchOpen, closeThreadsArch, openThreadsArch } from './threads.js';
import { showWatches, showFiles } from './pages.js';
import { nav, renderDrawer, drawerOpen, drawerGone } from './drawer.js';
import { setUsage, renderUsage } from './usage.js';
import { setTasks, loadTasks, tasksArchOpen, closeTasksArch } from './tasks.js';
import { account } from './account.js';
import { sheet, closeSheet } from './sheet.js';
import { init as initTheme, phone as phoneTheme } from './theme.js';

let lastId = 0;
// The chat's events as they came (the last KEEP_CHAT), kept in this browser for the next visit.
let raw = [];
const KEEP_CHAT = 300;
// An older copy is not drawn: a night brings about a thousand events, and drawing a copy only to reload it would flash.
const KEPT_FOR = 4 * 3600e3;
const saveChat = () => keep.later('chat', () => ({ events: raw.slice(-KEEP_CHAT), at: Date.now() }));

// ---------- pages ----------

// The drawer's destinations; chat is home.
const TABS = ['chat', 'threads', 'agents', 'watches', 'files', 'usage', 'settings'];
const TITLES = { chat: 'seimei', threads: 'Side threads', agents: 'Agents', watches: 'Watches', files: 'Files', usage: 'Usage', settings: 'Settings' };
const pages = Object.fromEntries(TABS.map((t) => [t, $(t === 'chat' ? 'log' : t)]));
const SHOW = { threads: showThreads, agents: showAgents, watches: showWatches, files: showFiles, usage: renderUsage };

function show(tab) {
  if (!TABS.includes(tab)) tab = 'chat';
  const was = S.tab;
  S.tab = tab;
  for (const t of TABS) pages[t].hidden = t !== tab;
  document.body.dataset.tab = tab;
  // Top level: the new destination fades in; no slide between nav destinations
  if (was && was !== tab && !reduced()) pages[tab].animate([{ opacity: 0 }, { opacity: 1 }], { duration: 200, easing: 'cubic-bezier(.2, 0, 0, 1)' });
  if (tab === 'chat') {
    // Back at the bottom only if the reader was following when they left
    if (was !== 'chat' && following()) requestAnimationFrame(toBottom);
    if (S.lane === 't') showLane('t');
  } else SHOW[tab]?.();
  chrome();
  lift();
}

// The top bar fills with surface-container once its screen has moved off its rest (M3 app bars on scroll).
// The chat rests at its end; the other pages at their top.
function lift() {
  const sc = pages[S.tab];
  const off = S.tab === 'chat' && S.lane !== 'k' ? !nearBottom() : sc.scrollTop > 0;
  document.body.classList.toggle('lifted', off);
}
for (const t of TABS) pages[t].addEventListener('scroll', lift, { passive: true });

// Chat is home: other pages sit one step of history above it, so the phone's back button returns to the chat.
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

// What runs once the history entry being left is gone (a page or the drawer closing, then where the user goes).
let afterPop = null;
function thenGo(run) {
  afterPop = run;
  history.back();
}

// Where the drawer's picks go.
nav.go = go;
nav.thread = openThread;
nav.agent = openPage;
nav.threadsArchive = () => {
  go('threads');
  openThreadsArch();
};
// The send sheet's Show: from a thread's page back to the chat.
hooks.home = () => (pageOpen() ? thenGo(() => go('chat')) : go('chat'));
$('new-thread').addEventListener('click', () => openDraft());

window.addEventListener('popstate', () => {
  const run = afterPop;
  afterPop = null;
  if (viewing()) closeView(true);
  else if (!sheet.wrap.hidden) closeSheet(true);
  else if (drawerOpen() && !history.state?.drawer) drawerGone();
  // Closing a sheet over the page goes back to the page's own entry: the page stays.
  else if (pageOpen() && !history.state?.spage) closePage();
  else if (pageOpen()) {}
  else if (archOpen() && !history.state?.arch) closeArch();
  else if (tasksArchOpen() && !history.state?.tarch) closeTasksArch();
  else if (threadsArchOpen() && !history.state?.tarchive) closeThreadsArch();
  else show(location.hash.slice(1) || 'chat');
  chrome();
  run?.();
});

// ---------- header and drawer ----------

// News for the user in seimei's chat while it is out of view: the drawer's seimei count, and the dot on a thread page's back button.
const chatInView = () => S.tab === 'chat' && !pageOpen() && !document.hidden;
hooks.seimeiNews = () => !chatInView() && (S.unread > 0 || waitingApprovals().length > 0);

function chrome() {
  const { asks } = agentCounts();
  let s = 'idle', text = '';
  if (S.tab === 'chat') ({ s, text } = S.status);
  // Away: the other pages show the last known state, and say so.
  if (S.tab !== 'chat' && !S.online && S.awaySince) {
    s = 'away';
    text = `Offline since ${clock(S.awaySince / 1000)}`;
  }
  if (chatInView()) S.unread = 0;
  $('title').textContent = TITLES[S.tab];
  // The dispatcher on an ACP agent or Codex says which; the default harness goes without saying.
  const harness = S.session.harness || '';
  $('harness').hidden = S.tab !== 'chat' || !harness.startsWith('acp:');
  $('harness').textContent = harness.replace(/^acp:/, 'on ');
  $('status').dataset.s = s;
  $('status-text').textContent = text;
  // Idle and working go without saying in the chat (its working line shows a turn); only problems and asks show.
  $('status').hidden = !text || (S.tab === 'chat' && (s === 'idle' || s === 'working'));

  // The menu icon's dot: a thread answered, an agent asks, or seimei said something while the chat is out of view.
  const news = S.threads.some((t) => t.unread > 0) || asks > 0 || (S.tab !== 'chat' && hooks.seimeiNews());
  $('drawer-dot').hidden = !news;
  $('drawer-btn').setAttribute('aria-label', news ? 'Open the menu, news' : 'Open the menu');
  if (S.tab === 'threads') renderThreads();
  renderDrawer();
}
hooks.chrome = chrome;

// The keyboard covers the bottom of a phone screen: short landscape hides the header while typing.
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
    // seimei's count is for the conversation; the trajectory keeps its own.
    if (!chatInView() && ev.lane !== 't' && /^(assistant|approval|worker|image|file)$/.test(ev.type)) {
      S.unread = (S.unread || 0) + 1;
      chrome();
    }
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
  } else if (ev.type === 's') {
    sessionEvent(ev);
  } else if (ev.type === 'usage') {
    setUsage(ev);
  } else if (ev.type === 'sessions') {
    setSessions(ev.list);
  } else if (ev.type === 'tasks') {
    setTasks(ev);
  } else if (ev.type === 'threads') {
    setThreads(ev);
  } else if (ev.type === 'theme') {
    phoneTheme(ev);
  }
}

// ---------- connection ----------

let source = null;
// Set once the page has its history (or the kept chat): before that, a stream from id 0 would replay the whole log.
let ready = false;
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
  if (!ready) return;
  source?.close();
  lastSeen = Date.now();
  source = new EventSource(`api/stream?after=${lastId}`);
  source.onopen = () => {
    // The board's changes while away went by on the stream: fetch it whole.
    loadTasks();
    refreshPage();
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
    // The browser retries unless the server refused outright, and EventSource cannot see why: ask whether the session is still valid.
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
document.addEventListener('visibilitychange', () => {
  // A hidden page is away: this browser's notifications (if on) take over until it shows again.
  fetch('api/presence', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ view: document.hidden ? 'hidden' : 'chat' }) }).catch(() => null);
  if (document.hidden) return;
  resume();
  navigator.serviceWorker?.getRegistration().then((r) => r?.getNotifications()).then((ns) => ns?.forEach((n) => n.close())).catch(() => null);
});
window.addEventListener('pageshow', (e) => { if (e.persisted) resume(); });
// A tap on a thread's notification while the page is open (sw.js).
navigator.serviceWorker?.addEventListener('message', (e) => { if (e.data?.thread) openThread(e.data.thread); });
// Try now, on the away block: reconnect at once instead of waiting for the next retry.
$('retry').addEventListener('click', connect);

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
  const url = kept?.length ? 'api/history?limit=0' : 'api/history?limit=200';
  let res = await fetch(url).catch(() => null);
  // Nothing kept and no history yet (the server asleep, the phone offline): ask again until it comes.
  while (!kept?.length && !res?.ok && res?.status !== 401) {
    lost();
    await new Promise((ok) => setTimeout(ok, 5000));
    res = await fetch(url).catch(() => null);
  }
  if (res?.status === 401) return location.reload();
  const r = res?.ok ? await res.json() : null;
  if (r && kept?.length && (r.last < lastId || r.last - lastId > KEEP_CHAT)) {
    // Further behind than the chat keeps (or another server's log): the replay would replace it all, so start from the server's copy. The app does the same.
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
    if (r.threads) setThreads(r.threads);
  }
  autosize();
  // A thread's notification opens its page (sw.js).
  const thread = location.hash.match(/^#thread=(.+)$/)?.[1];
  if (thread) history.replaceState(null, '', location.pathname);
  if (!kept?.length || thread) show(thread ? 'chat' : location.hash.slice(1) || 'chat');
  if (thread) openThread(decodeURIComponent(thread));
  render();
  ready = true;
  connect();
  account();
}

initTheme();
start();
