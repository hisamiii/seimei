// Shikigami and the user's own Claude sessions: the Agents list, and the full-screen page of one of them.
import { $, S, hooks, el, svg, clock, home, short, when, getJSON, finePointer, toLogin, ICON } from './util.js';
import { Renderer, doing as doingPhrase, elapsed } from './chat.js';
import { sheet, showSheet, closeSheet } from './sheet.js';

const page = $('agents');
const sp = $('spage');

export const ACTIVE = new Set(['starting', 'running', 'asking']);

function stateText(w) {
  const at = clock(w.updated);
  return {
    starting: 'Starting', running: `Working since ${at}`, asking: 'Waiting for your answer', done: `Finished at ${at}`,
    failed: `Failed at ${at}`, stopped: `Stopped at ${at}`, interrupted: `Interrupted at ${at}`,
    laptop: `Open on the laptop since ${at}`,
  }[w.state] || w.state;
}

const cap = (t) => t.charAt(0).toUpperCase() + t.slice(1);

// What a shikigami is up to: its latest step.
function workerLine(w) {
  const d = w.doing;
  if (!d) return w.state === 'starting' ? 'Starting' : 'Working';
  if (d.text) return d.text;
  return d.summary ? `${cap(doingPhrase(d.name))}: ${d.summary}` : cap(doingPhrase(d.name));
}

// What a session is up to: its open call and how long it has run.
function sessionLine(d) {
  if (!d) return 'Working';
  const what = d.tool === 'Bash' ? `running ${d.what}` : `${doingPhrase(d.tool)}: ${d.what}`;
  return `${cap(what)}${d.since ? ` · ${elapsed(d.since).split(' ')[0]}` : ''}`;
}

// ---------- the user's own sessions: which turns are new to them ----------

const SEEN = 'seimei.seen';
let seen = {};
try { seen = JSON.parse(localStorage.getItem(SEEN) || '{}'); } catch {}
const saveSeen = () => { try { localStorage.setItem(SEEN, JSON.stringify(seen)); } catch {} };

export function setSessions(list) {
  // A session seen for the first time starts read; only turns that end after that get marked.
  let fresh = false;
  for (const s of list) {
    if (!(s.session_id in seen)) {
      seen[s.session_id] = s.status === 'busy' ? 0 : s.active;
      fresh = true;
    }
  }
  if (fresh) saveSeen();
  S.sessions = list;
  changed();
  if (P?.kind === 'session') {
    const s = list.find((x) => x.pid === P.id);
    if (s) {
      P.row = s;
      head();
    }
  }
}

function markSeen(s) {
  seen[s.session_id] = s.active;
  saveSeen();
}

// ---------- shikigami ----------

let all = null;
let allFetch = 0;
let showAll = false;

export function setWorkers(ev) {
  S.workers = ev.list;
  S.workersTotal = ev.total ?? S.workersTotal;
  if (P?.kind === 'worker') {
    const w = S.workers.find((x) => x.id === P.id);
    if (w) {
      P.row = { ...P.row, ...w };
      head();
    }
  }
  // The full list (finished ones from days ago too) is fetched only while the tab shows.
  if (S.tab === 'agents') refreshAll();
  changed();
}

async function refreshAll() {
  const now = Date.now();
  if (now - allFetch < 2000) return;
  allFetch = now;
  const r = await getJSON('api/workers');
  if (r) {
    all = r.workers;
    renderAgents();
  }
}

// ---------- one model for both kinds ----------

// Catppuccin accents; a project keeps its color everywhere.
const TINTS = ['#89b4fa', '#a6e3a1', '#94e2d5', '#cba6f7', '#f5c2e7', '#b4befe', '#f9e2af', '#74c7ec', '#eba0ac', '#fab387'];
function tint(project) {
  if (!project) return '#a6adc8';
  let h = 0;
  for (const c of project) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return TINTS[h % TINTS.length];
}

function fromWorker(w) {
  const state = w.state === 'asking' ? 'ask' : ACTIVE.has(w.state) ? 'work' : 'done';
  return {
    kind: 'worker', id: w.id, name: w.name, project: w.project, context: w.context, state, since: w.updated,
    line: state === 'ask' ? (w.question || 'Asks a question') : state === 'work' ? workerLine(w) : (w.summary || stateText(w)),
    bad: w.state === 'failed',
  };
}

function fromSession(s) {
  const asking = s.question || s.status === 'waiting';
  const state = asking ? 'ask' : s.status === 'busy' ? 'work' : 'idle';
  return {
    kind: 'session', id: s.pid, name: s.name, project: s.project, context: s.context, state, since: s.since,
    line: s.question ? (s.ask || 'Asks a question') : asking ? `Waiting for ${s.waiting_for || 'you'} on the laptop`
      : state === 'work' ? sessionLine(s.doing) : (s.line || 'Idle'),
    fresh: state === 'idle' && s.active > (seen[s.session_id] || 0) + 1,
  };
}

// The navigation badge: working agents, and whether anything waits on the user.
export function agentCounts() {
  const items = [...S.workers.map(fromWorker), ...S.sessions.map(fromSession)];
  return { working: items.filter((x) => x.state === 'work').length, asks: items.filter((x) => x.state === 'ask').length };
}

function changed() {
  hooks.chrome();
  if (S.tab === 'agents') renderAgents();
}
setInterval(() => { if (S.tab === 'agents') renderAgents(); }, 30000);

// ---------- the list ----------

const Q_ICON = 'M4 5h16v11H9l-5 4zM10 9.2a2 2 0 1 1 2.6 1.9c-.4.2-.6.5-.6.9M12 13.6v.1';
const ARC = 'M12 3a9 9 0 1 1-9 9';

function tile(x) {
  const t = el('span', `tile ${x.state}${x.bad ? ' bad' : ''}`);
  t.style.setProperty('--c', tint(x.project));
  const icon = x.state === 'work' ? ARC : x.state === 'ask' ? Q_ICON : x.bad ? 'M12 7v6M12 17v.01' : ICON.check;
  t.append(svg(icon, 20));
  return t;
}

function row(x) {
  const b = el('button', `agent sl ${x.state}`);
  b.type = 'button';
  const top = el('span', 'top');
  top.append(el('b', null, x.name));
  if (x.kind === 'worker') top.append(el('span', 'tag', 'shikigami'));
  const t = el('time', x.fresh ? 'fresh' : null, x.state === 'done' ? when(x.since) : short(x.since));
  top.append(t);
  const sub = el('span', 'sub');
  if (x.project) {
    const p = el('span', 'proj', x.project);
    p.style.color = tint(x.project);
    sub.append(p, ' · ');
  }
  sub.append(el('span', 'line', x.line));
  const txt = el('span', 'txt');
  txt.append(top, sub);
  b.append(tile(x), txt);
  b.addEventListener('click', () => openPage(x.kind, x.id));
  return b;
}

function group(title, cls, items) {
  const sec = el('section', `agroup ${cls}`);
  sec.append(el('h2', null, title), ...items.map(row));
  return sec;
}

export function renderAgents() {
  const items = [...S.workers.filter((w) => ACTIVE.has(w.state)).map(fromWorker), ...S.sessions.map(fromSession)];
  const by = (st) => items.filter((x) => x.state === st).sort((a, b) => b.since - a.since);
  const ask = by('ask'), work = by('work'), idle = by('idle');
  const parts = [];
  const counts = el('div', 'acounts');
  for (const [n, label, cls] of [[ask.length, 'needs you', 'ask'], [work.length, 'working', 'work'], [idle.length, 'idle', 'idle']]) {
    if (n) {
      const c = el('span', cls);
      c.append(el('i'), `${n} ${label}`);
      counts.append(c);
    }
  }
  if (counts.childElementCount) parts.push(counts);
  if (ask.length) parts.push(group('Needs you', 'ask', ask));
  if (work.length) parts.push(group('Working', 'work', work));
  if (idle.length) parts.push(group('Idle', 'idle', idle));
  const done = (all || S.workers).filter((w) => !ACTIVE.has(w.state)).sort((a, b) => b.updated - a.updated);
  if (done.length) {
    const shown = showAll ? done : done.slice(0, 15);
    parts.push(group('Finished shikigami', 'done', shown.map(fromWorker)));
    if (done.length > shown.length) {
      const more = el('button', 'older sl', `Show all ${done.length}`);
      more.type = 'button';
      more.addEventListener('click', () => { showAll = true; renderAgents(); });
      parts.push(more);
    }
  }
  if (!parts.length) {
    const empty = el('div', 'empty');
    empty.append(el('b', null, 'Nothing running'), el('span', null, 'Your Claude sessions on the laptop and the shikigami seimei summons show up here.'));
    parts.push(empty);
  }
  const top = page.scrollTop;
  page.replaceChildren(...parts);
  page.scrollTop = top;
}

export function showAgents() {
  renderAgents();
  allFetch = 0;
  refreshAll();
}

// ---------- the page of one agent ----------

// The open page: kind 'worker' or 'session', its id, its latest row, and the parts that update live.
let P = null;

function ring(pct) {
  const b = el('button', 'ring sl');
  b.type = 'button';
  const n = Math.max(0, Math.min(100, pct ?? 0));
  b.setAttribute('aria-label', pct == null ? 'Session details' : `Context ${n}% used. Session details`);
  b.innerHTML = `<svg width="34" height="34" viewBox="0 0 36 36" aria-hidden="true"><circle class="t" cx="18" cy="18" r="15"/>`
    + `<circle class="f" cx="18" cy="18" r="15" pathLength="100" stroke-dasharray="${n} 100" transform="rotate(-90 18 18)"/></svg>`;
  b.append(el('span', null, pct == null ? '' : String(n)));
  b.classList.toggle('hot', n >= 80);
  return b;
}

function head() {
  if (!P) return;
  const x = P.kind === 'worker' ? fromWorker(P.row) : fromSession(P.row);
  P.title.textContent = x.name;
  const sub = P.sub;
  sub.replaceChildren();
  const dot = el('i', x.state);
  sub.append(dot);
  if (x.project) {
    const p = el('b', null, x.project);
    p.style.color = tint(x.project);
    sub.append(p, ' · ');
  }
  const word = { ask: 'Needs you', work: 'Working', idle: 'Idle', done: P.row.state ? stateText(P.row) : 'Done' }[x.state];
  sub.append(el('span', x.state, x.state === 'done' ? word : `${word} ${short(x.since).replace('now', '')}`.trim()));
  const r = ring(x.context);
  r.addEventListener('click', details);
  P.ring.replaceWith(r);
  P.ring = r;
  if (P.kind === 'worker') {
    P.stop.hidden = !ACTIVE.has(P.row.state);
    question(P.row.state === 'asking' ? { text: P.row.question } : null);
  }
  composerState();
}

// The question card at the end of the timeline: options to tap when the laptop can be reached.
function question(ask) {
  const key = ask ? ask.tool_id || ask.text : null;
  if (key === P.askKey) return;
  P.askKey = key;
  P.ask = ask;
  P.qbox.replaceChildren();
  P.qbox.hidden = !ask;
  if (!ask) return composerState();
  const top = el('div', 'qhead');
  const ic = el('span', 'qic');
  ic.append(svg(Q_ICON, 18));
  const t = el('span', 'qt');
  const q = ask.questions?.[0];
  t.append(el('b', null, q?.question || ask.text || 'A question'));
  if (q?.header) t.append(el('small', null, q.header));
  top.append(ic, t);
  P.qbox.append(top);
  if (q) {
    (q.options || []).forEach((o, i) => {
      const b = el('button', 'qopt sl');
      b.type = 'button';
      b.append(el('b', null, o.label));
      if (o.description) b.append(el('small', null, o.description));
      b.disabled = !ask.answerable || !!P.row.reach;
      b.addEventListener('click', () => answer({ option: i + 1 }, b));
      P.qbox.append(b);
    });
    const many = (ask.questions?.length || 0) > 1;
    P.qbox.append(el('p', 'qnote', !ask.answerable ? `Answer ${many ? 'these' : 'this one'} on the laptop: ${many ? 'several questions at once' : 'pick any'} works there only.`
      : 'Something else? Answer in your own words below.'));
  } else {
    P.qbox.append(el('p', 'qnote', 'Answer below.'));
  }
  composerState();
  stick(true);
}

async function answer(body, btn) {
  for (const b of P.qbox.querySelectorAll('button')) b.disabled = true;
  btn?.classList.add('picked');
  const r = await postJSON(`api/sessions/${P.id}/answer`, { tool_id: P.ask.tool_id, ...body });
  if (!r.ok) {
    btn?.classList.remove('picked');
    for (const b of P.qbox.querySelectorAll('button')) b.disabled = false;
    note(r.error);
    return false;
  }
  note(null);
  poll(true);
  return true;
}

async function postJSON(path, body) {
  const r = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }).catch(() => null);
  if (r?.status === 401) toLogin();
  const j = await r?.json().catch(() => null);
  return r?.ok ? { ok: true } : { ok: false, error: j?.error || (r ? `The laptop said no (${r.status}).` : 'The laptop did not answer.') };
}

function note(text, cls = 'error') {
  P.note.textContent = text || '';
  P.note.className = `snote ${cls}`;
  P.note.hidden = !text;
}

// Why the composer cannot type into this session, if it cannot.
function blocked() {
  if (P.kind === 'worker') return null;
  const s = P.row;
  if (s.reach) return s.reach;
  if (P.ask && !P.ask.answerable) return 'Answer on the laptop.';
  if (s.status === 'waiting' && !s.question) return `Waiting on the laptop (${s.waiting_for || 'a prompt'}).`;
  return null;
}

function composerState() {
  if (!P) return;
  const why = blocked();
  P.input.disabled = !!why;
  P.attach.disabled = !!why;
  P.input.placeholder = why ? 'Can\'t reply from here right now' : P.ask ? 'Your answer…' : `Message ${P.row.name.length > 22 ? `${P.row.name.slice(0, 21).trim()}…` : P.row.name}`;
  if (P.kind === 'session') {
    if (why) note(why, 'info');
    else if (P.note.classList.contains('info')) note(null);
  }
  sendState();
}

// ---------- attachments: everything uploads to the laptop at once, and the message names the files ----------

const MAX_FILE = 50 << 20;

function addFiles(list) {
  for (const f of list) {
    if (P.files.length >= 10) break;
    if (f.size > MAX_FILE) {
      note(`${f.name} is over 50 MB.`);
      continue;
    }
    const a = { name: f.name || 'file', id: null, url: f.type.startsWith('image/') ? URL.createObjectURL(f) : null };
    a.upload = fetch(`api/upload?name=${encodeURIComponent(a.name)}`, {
      method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: f,
    }).then(async (r) => {
      if (r.status === 401) toLogin();
      const body = await r.json().catch(() => null);
      if (!r.ok || !body?.id) throw new Error(body?.error || 'upload failed');
      a.id = body.id;
    }).catch((e) => {
      P?.files.splice(P.files.indexOf(a) >>> 0, P.files.includes(a) ? 1 : 0);
      note(`Could not upload ${a.name}: ${e.message}`);
    }).finally(() => { a.upload = null; tray(); });
    P.files.push(a);
  }
  tray();
}

function tray() {
  if (!P) return;
  P.tray.replaceChildren();
  P.files.forEach((a, i) => {
    const item = el('div', `att${a.url ? '' : ' file'}${a.upload ? ' busy' : ''}`);
    if (a.url) {
      const img = el('img');
      img.src = a.url;
      img.alt = a.name;
      item.append(img);
    } else {
      item.append(svg('M6 3h8l4 4v14H6zM14 3v4h4', 20), el('span', 'fname', a.name));
    }
    const x = el('button', 'remove sl');
    x.type = 'button';
    x.setAttribute('aria-label', `Remove ${a.name}`);
    x.append(svg('M6 6l12 12M18 6 6 18', 14));
    x.addEventListener('click', () => {
      if (a.url) URL.revokeObjectURL(a.url);
      P.files.splice(i, 1);
      tray();
    });
    item.append(x);
    P.tray.append(item);
  });
  P.tray.hidden = !P.files.length;
  sendState();
}

function sendState() {
  if (!P) return;
  const typing = P.input.value.trim() !== '' || P.files.length > 0;
  P.send.disabled = P.input.disabled || !typing;
  if (P.kind === 'worker') {
    P.stop.hidden = !ACTIVE.has(P.row.state) || typing;
    P.send.hidden = !P.stop.hidden;
  }
}

async function submit(e) {
  e.preventDefault();
  const text = P.input.value.trim();
  const files = P.files.splice(0);
  if (!text && !files.length) return;
  P.input.value = '';
  autosize();
  tray();
  await Promise.all(files.map((a) => a.upload).filter(Boolean));
  const ids = files.filter((a) => a.id).map((a) => a.id);
  let r;
  if (P.kind === 'worker') r = await postJSON(`api/workers/${encodeURIComponent(P.id)}/send`, { text, files: ids });
  else if (P.ask?.answerable) r = await postJSON(`api/sessions/${P.id}/answer`, { tool_id: P.ask.tool_id, text, files: ids });
  else r = await postJSON(`api/sessions/${P.id}/send`, { text, files: ids });
  if (!r.ok) {
    P.input.value = text;
    P.files.unshift(...files.filter((a) => a.id));
    tray();
    autosize();
    note(r.error);
    return;
  }
  files.forEach((a) => a.url && URL.revokeObjectURL(a.url));
  note(null);
  composerState();
  stick(true);
  if (P.kind === 'session') poll(true);
}

function autosize() {
  P.input.style.height = 'auto';
  P.input.style.height = `${Math.min(P.input.scrollHeight, 160)}px`;
}

// ---------- the timeline ----------

// Like a chat: new events keep the reader at the bottom unless they scrolled up.
const nearBottom = () => P.main.scrollHeight - P.main.scrollTop - P.main.clientHeight < 120;
function stick(force = false) {
  if (P && (force || P.pinned)) P.main.scrollTop = P.main.scrollHeight;
}

function add(events) {
  for (const ev of events) P.renderer.add(ev);
  if (events.length) P.empty?.remove();
}

let pollTimer = 0;
async function poll(soon = false) {
  clearTimeout(pollTimer);
  if (!P || P.kind !== 'session') return;
  const p = P;
  if (soon) await new Promise((r) => setTimeout(r, 600));
  const r = await getJSON(`api/sessions/${p.id}${p.next != null ? `?from=${p.next}` : ''}`);
  if (P !== p) return;
  if (r) {
    if (p.next != null && r.next < p.next) {
      // The transcript was replaced (compacted or continued): draw it again.
      return openPage('session', p.id);
    }
    p.next = r.next;
    p.row = { ...p.row, ...r.session, reach: r.reach };
    add(r.events);
    p.renderer.live(r.doing && { ...r.doing, output: r.live?.output });
    question(r.ask);
    head();
    if (r.session?.session_id) markSeen(r.session);
    stick();
  }
  pollTimer = setTimeout(poll, document.hidden ? 8000 : 2000);
}

setInterval(() => {
  if (!P) return;
  P.renderer.tick();
}, 1000);

export function sheetEvent(ev) {
  if (P?.kind !== 'worker' || P.id !== ev.worker) return;
  add([ev.ev]);
  stick();
}

function details() {
  const r = P.row;
  const body = [];
  const titles = el('div', 'dhead');
  titles.append(el('h2', null, r.name), el('div', 'sub', P.kind === 'worker' ? 'Shikigami' : `Session ${r.kind || ''}`.trim()));
  body.push(titles);
  const facts = el('div', 'facts');
  facts.append(el('span', 'mono', home(r.cwd)));
  if (r.model) facts.append(el('span', null, r.model));
  const effort = r.effort || r.effort_now;
  if (effort) facts.append(el('span', null, `effort ${effort}${r.effort || P.kind !== 'worker' ? '' : ' (default)'}`));
  if (r.context_cap) facts.append(el('span', null, `compacts at ${r.context_cap}`));
  if (r.subagents) facts.append(el('span', null, `subagents on ${r.subagents}`));
  if (r.context != null) facts.append(el('span', null, `context ${r.context}% used`));
  facts.append(el('span', null, P.kind === 'worker' ? `started ${clock(r.created)}` : `last active ${clock(r.active)}`));
  if (P.kind === 'worker' && r.state !== 'laptop' && r.started !== false) facts.append(laptopButton(r));
  body.push(facts);
  if (P.kind === 'worker' && r.task) body.push(taskRow(r.task));
  if (P.kind === 'session') body.push(el('p', 'usage-note', r.reach ? r.reach : 'Messages you send here are typed into this session\'s window on the laptop.'));
  sheet.body.replaceChildren(...body);
  showSheet('details');
}

function laptopButton(w) {
  const b = el('button', 'pill-btn sl');
  b.type = 'button';
  const label = el('span', null, 'On laptop');
  b.append(svg(ICON.laptop, 16), label);
  let armed = false;
  b.addEventListener('click', async () => {
    const cur = S.workers.find((x) => x.id === w.id) || w;
    // A running worker gets stopped first, so ask for a second tap.
    if (ACTIVE.has(cur.state) && !armed) {
      armed = true;
      label.textContent = 'Stop it and open on laptop';
      return;
    }
    b.disabled = true;
    const r = await postJSON(`api/workers/${encodeURIComponent(w.id)}/laptop`, {});
    b.disabled = false;
    armed = false;
    if (r.ok) {
      b.hidden = true;
    } else {
      label.textContent = 'On laptop';
      sheet.body.append(el('p', 'snote error', `Could not open ${w.name} on the laptop: ${r.error}`));
    }
  });
  return b;
}

// The task as one tonal row with a preview; tapping it unfolds the whole text.
function taskRow(text) {
  const box = el('div', 'taskd');
  const headBtn = el('button', 'taskd-head sl');
  headBtn.type = 'button';
  const ic = el('span', 'ic');
  ic.append(svg(ICON.list, 18));
  const txt = el('span', 'txt');
  txt.append(el('b', null, 'Task'), el('span', 'preview', text));
  const chev = svg(ICON.chev, 20);
  chev.classList.add('chev');
  headBtn.append(ic, txt, chev);
  const full = el('div', 'full', text);
  full.hidden = true;
  headBtn.addEventListener('click', () => {
    full.hidden = !full.hidden;
    box.classList.toggle('open', !full.hidden);
  });
  box.append(headBtn, full);
  return box;
}

function build(kind, id, row) {
  const bar = el('header', 'sbar');
  const back = el('button', 'back sl');
  back.type = 'button';
  back.setAttribute('aria-label', 'Back to agents');
  back.append(svg('M15 5l-7 7 7 7', 22));
  back.addEventListener('click', () => history.back());
  const titles = el('div', 'titles');
  const title = el('h1');
  const sub = el('div', 'sub');
  titles.append(title, sub);
  const r = el('span');
  bar.append(back, titles, r);

  const main = el('main', 'stl');
  const thread = el('div', 'sthread');
  const empty = el('div', 'note', 'Nothing here yet.');
  thread.append(empty);
  const qbox = el('section', 'qcard');
  qbox.hidden = true;
  qbox.setAttribute('aria-label', 'Question');
  main.append(thread, qbox);

  const noteEl = el('p', 'snote');
  noteEl.hidden = true;
  const trayEl = el('div', 'attachments');
  trayEl.hidden = true;
  const form = el('form', 'scomposer');
  const field = el('div', 'field');
  const attach = el('button', 'attach sl');
  attach.type = 'button';
  attach.setAttribute('aria-label', 'Attach photos or files');
  attach.append(svg('M20 11.5l-7.8 7.8a5 5 0 0 1-7-7l8.5-8.5a3.3 3.3 0 0 1 4.7 4.7l-8.5 8.5a1.7 1.7 0 0 1-2.4-2.4l7.8-7.8', 22));
  const input = el('textarea');
  input.rows = 1;
  input.setAttribute('enterkeyhint', 'send');
  input.setAttribute('aria-label', 'Message');
  const file = el('input');
  file.type = 'file';
  file.multiple = true;
  file.hidden = true;
  field.append(attach, input);
  const send = el('button', 'fab sl');
  send.type = 'submit';
  send.setAttribute('aria-label', 'Send');
  send.append(svg(ICON.send, 20));
  const stop = el('button', 'fab tonal sl');
  stop.type = 'button';
  stop.setAttribute('aria-label', 'Stop');
  stop.append(el('span', 'square'));
  stop.hidden = true;
  form.append(field, file, send, stop);
  const foot = el('div', 'sfoot');
  foot.append(noteEl, trayEl, form);
  sp.replaceChildren(bar, main, foot);

  P = { kind, id, row, title, sub, ring: r, main, thread, empty, qbox, note: noteEl, tray: trayEl, input, attach, send, stop,
    files: [], next: null, ask: null, askKey: undefined, pinned: true, renderer: new Renderer(thread) };

  main.addEventListener('scroll', () => { if (P) P.pinned = nearBottom(); }, { passive: true });
  new ResizeObserver(() => stick()).observe(thread);
  attach.addEventListener('click', () => file.click());
  file.addEventListener('change', (e) => { addFiles([...e.target.files]); e.target.value = ''; });
  input.addEventListener('input', () => { autosize(); sendState(); });
  input.addEventListener('paste', (e) => {
    const fs = [...(e.clipboardData?.files || [])];
    if (fs.length) {
      e.preventDefault();
      addFiles(fs);
    }
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && finePointer) {
      e.preventDefault();
      form.requestSubmit();
    }
  });
  form.addEventListener('submit', submit);
  stop.addEventListener('click', () => postJSON(`api/workers/${encodeURIComponent(id)}/stop`, {}));
}

export async function openPage(kind, id) {
  const r = kind === 'worker' ? await getJSON(`api/workers/${encodeURIComponent(id)}`) : await getJSON(`api/sessions/${id}`);
  if (!r) return;
  const reopen = P && P.kind === kind && P.id === id;
  if (kind === 'worker') {
    const live = S.workers.find((x) => x.id === id);
    build(kind, id, { ...r.worker, ...live });
    add(r.events);
  } else {
    build(kind, id, { ...r.session, reach: r.reach });
    P.next = r.next;
    add(r.events);
    P.renderer.live(r.doing && { ...r.doing, output: r.live?.output });
    question(r.ask);
    if (r.session.session_id) markSeen(r.session);
    changed();
    pollTimer = setTimeout(poll, 2000);
  }
  head();
  sp.hidden = false;
  document.body.classList.add('spage-open');
  if (!reopen && !history.state?.spage) history.pushState({ spage: true }, '');
  requestAnimationFrame(() => stick(true));
}

export const openWorker = (id) => openPage('worker', id);
export const openSession = (pid) => openPage('session', pid);
export const pageOpen = () => !sp.hidden;

export function closePage() {
  clearTimeout(pollTimer);
  closeSheet(true);
  P?.files.forEach((a) => a.url && URL.revokeObjectURL(a.url));
  P = null;
  sp.hidden = true;
  sp.replaceChildren();
  document.body.classList.remove('spage-open');
}
