// Shikigami and the user's own Claude sessions: the Agents tab and their sheets.
import { $, S, hooks, el, svg, clock, home, short, when, post, getJSON, finePointer, ICON } from './util.js';
import { Renderer, message, phrase, main } from './chat.js';
import { sheet, showSheet } from './sheet.js';

const page = $('agents');

export const TONES = {
  starting: ['working', 'run'], running: ['working', 'run'], asking: ['needs you', 'ask'], done: ['done', 'idle'],
  failed: ['failed', 'bad'], stopped: ['stopped', 'idle'], interrupted: ['interrupted', 'idle'], laptop: ['on laptop', 'idle'],
};
export const ACTIVE = new Set(['starting', 'running', 'asking']);

function stateText(w) {
  const at = clock(w.updated);
  return {
    starting: 'Starting', running: `Working since ${at}`, asking: 'Waiting for your answer', done: `Finished at ${at}`,
    failed: `Failed at ${at}`, stopped: `Stopped at ${at}`, interrupted: `Interrupted at ${at}`,
    laptop: `Open on the laptop since ${at}`,
  }[w.state] || w.state;
}

// What a running shikigami is up to: its latest step.
function doing(w) {
  const d = w.doing;
  if (!d) return w.state === 'starting' ? 'Starting' : 'Working';
  if (d.text) return d.text;
  const what = phrase(d.name, 1);
  return d.summary ? `${what.charAt(0).toUpperCase() + what.slice(1)}: ${d.summary}` : what;
}

// ---------- the user's own sessions ----------

const SEEN = 'seimei.seen';
let seen = {};
try { seen = JSON.parse(localStorage.getItem(SEEN) || '{}'); } catch {}
const saveSeen = () => { try { localStorage.setItem(SEEN, JSON.stringify(seen)); } catch {} };

// What a session needs from the user: an open question first, then a finished turn not yet looked at.
function look(s) {
  if (s.question) return { label: 'question', tone: 'ask', rank: 0, ask: true };
  if (s.status === 'waiting') return { label: s.waiting_for || 'waiting', tone: 'ask', rank: 0, ask: true };
  if (s.status === 'busy') return { label: 'working', tone: 'run', rank: 2 };
  if (s.status === 'idle' && s.active > (seen[s.session_id] || 0) + 1) return { label: 'new reply', tone: 'idle', rank: 1, fresh: true };
  return { label: s.status, tone: 'idle', rank: 3 };
}

const sortedSessions = () => [...S.sessions].sort((a, b) => look(a).rank - look(b).rank || b.active - a.active);

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
  if (sheet.view === 'session') {
    const s = list.find((x) => x.pid === sheet.pid);
    if (s && s.status + s.question + s.active !== sheet.sig) openSession(s.pid);
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
  if (sheet.worker) {
    const w = S.workers.find((x) => x.id === sheet.worker.id);
    if (w) updateHead({ ...sheet.worker, ...w });
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

// The navigation badge: working shikigami, and whether anything waits on the user.
export function agentCounts() {
  const working = S.workers.filter((w) => ACTIVE.has(w.state)).length;
  const asks = S.workers.filter((w) => w.state === 'asking').length + S.sessions.filter((s) => look(s).ask).length;
  return { working, asks };
}

function changed() {
  hooks.chrome();
  if (S.tab === 'agents') renderAgents();
}
setInterval(() => { if (S.tab === 'agents') renderAgents(); }, 30000);

// ---------- the Agents tab ----------

const SPIN = 'spinner';

function spinner() {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  s.setAttribute('class', SPIN);
  s.setAttribute('viewBox', '0 0 24 24');
  s.setAttribute('aria-label', 'working');
  s.innerHTML = '<circle class="t" cx="12" cy="12" r="9"/><circle class="f" cx="12" cy="12" r="9" pathLength="100" stroke-dasharray="30 70"/>';
  return s;
}

function row({ av, avCls, name, sup, trail, cls = '', onClick }) {
  const b = el('button', `li sl ${cls}`);
  b.type = 'button';
  const a = el('span', `av ${avCls}`);
  if (typeof av === 'string') a.textContent = av;
  else a.append(av);
  const txt = el('span', 'txt');
  txt.append(el('span', 'hl', name), el('span', 'sup', sup));
  b.append(a, txt);
  if (trail) b.append(trail);
  b.addEventListener('click', onClick);
  return b;
}

const letter = (name) => (name || '?').trim().charAt(0).toLowerCase();
const answer = () => el('span', 'btn tonal small', 'Answer');
const trailTime = (t) => el('span', 'trail', t);

function section(title, note, rows) {
  const head = el('div', 'sec');
  head.append(el('span', null, title));
  if (note) head.append(el('small', null, note));
  const group = el('div', 'group');
  group.append(...rows);
  return [head, group];
}

function fade(t) {
  const age = Date.now() / 1000 - t;
  return age < 3600 ? 'fade1' : age < 86400 ? 'fade2' : 'fade3';
}

export function renderAgents() {
  const parts = [];
  const live = S.workers.filter((w) => ACTIVE.has(w.state))
    .sort((a, b) => (a.state === 'asking' ? 0 : 1) - (b.state === 'asking' ? 0 : 1) || b.updated - a.updated);
  if (live.length) {
    const asks = live.filter((w) => w.state === 'asking').length;
    parts.push(...section('Shikigami', asks ? `${live.length} working · ${asks} needs you` : `${live.length} working`,
      live.map((w) => row({
        av: letter(w.name), avCls: w.state === 'asking' ? 'ask' : '', name: w.name,
        sup: w.state === 'asking' ? `Asks: ${w.question || 'a question'}` : doing(w),
        trail: w.state === 'asking' ? answer() : spinner(), onClick: () => openWorker(w.id),
      }))));
  }
  if (S.sessions.length) {
    const sessions = sortedSessions();
    const asks = sessions.filter((s) => look(s).ask).length;
    parts.push(...section('Your sessions', asks ? `${asks} needs you` : '', sessions.map((s) => {
      const l = look(s);
      const sup = l.ask ? (s.question ? `Asks: ${s.question.split('\n')[0]}` : `Waiting for ${l.label}`)
        : `${s.status === 'busy' ? 'Working' : l.fresh ? 'New reply' : 'Idle'} · ${home(s.cwd)}`;
      const trail = l.ask ? answer() : s.status === 'busy' ? spinner() : trailTime(short(s.since));
      if (l.fresh) trail.classList.add('fresh');
      return row({ av: letter(s.name), avCls: `user ${l.ask ? 'ask' : ''}`, name: s.name, sup, trail, onClick: () => openSession(s.pid) });
    })));
  }
  const done = (all || S.workers).filter((w) => !ACTIVE.has(w.state)).sort((a, b) => b.updated - a.updated);
  if (done.length) {
    const shown = showAll ? done : done.slice(0, 15);
    const rows = shown.map((w) => row({
      av: svg(w.state === 'failed' ? 'M12 7v6M12 17v.01' : w.state === 'done' ? ICON.check : 'M7 12h10', 20),
      avCls: `done ${w.state === 'failed' ? 'bad' : ''}`, name: w.name, sup: w.summary || stateText(w),
      trail: trailTime(when(w.updated)), cls: fade(w.updated), onClick: () => openWorker(w.id),
    }));
    parts.push(...section('Finished', '', rows));
    if (done.length > shown.length) {
      const more = el('button', 'older sl', `Show all ${done.length}`);
      more.type = 'button';
      more.addEventListener('click', () => { showAll = true; renderAgents(); });
      parts.push(more);
    }
  }
  if (!parts.length) {
    const empty = el('div', 'empty');
    empty.append(el('b', null, 'No shikigami yet'), el('span', null, 'Ask seimei to summon one, and it shows up here while it works.'));
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

// ---------- the shikigami sheet ----------

function updateHead(w) {
  sheet.worker = w;
  const [, tone] = TONES[w.state] || [w.state, 'idle'];
  const h = sheet.head;
  h.badge.className = `wbadge ${tone}`;
  h.sub.textContent = stateText(w);
  h.stop.hidden = !ACTIVE.has(w.state);
  h.input.placeholder = w.state === 'asking' ? `Answer ${w.name}` : `Message ${w.name}`;
  h.question.hidden = !(w.state === 'asking' && w.question);
  if (w.question) h.question.textContent = w.question;
  h.laptop.hidden = w.state === 'laptop' || w.started === false;
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
    const r = await fetch(`api/workers/${encodeURIComponent(w.id)}/laptop`, { method: 'POST',
      headers: { 'Content-Type': 'application/json' }, body: '{}' }).catch(() => null);
    const body = await r?.json().catch(() => null);
    b.disabled = false;
    armed = false;
    if (r?.ok) {
      b.hidden = true;
    } else {
      label.textContent = 'On laptop';
      main.add({ type: 'system', level: 'error', text: `Could not open ${w.name} on the laptop: ${body?.error || 'no answer'}` });
    }
  });
  return b;
}

// The task as one tonal row with a preview; tapping it unfolds the whole text.
function taskRow(text) {
  const box = el('div', 'taskd');
  const head = el('button', 'taskd-head sl');
  head.type = 'button';
  const ic = el('span', 'ic');
  ic.append(svg(ICON.list, 18));
  const txt = el('span', 'txt');
  txt.append(el('b', null, 'Task'), el('span', 'preview', text));
  const chev = svg(ICON.chev, 20);
  chev.classList.add('chev');
  head.append(ic, txt, chev);
  const full = el('div', 'full', text);
  full.hidden = true;
  head.addEventListener('click', () => {
    full.hidden = !full.hidden;
    box.classList.toggle('open', !full.hidden);
  });
  box.append(head, full);
  return box;
}

function composer(onSend, extra = []) {
  const form = el('form', 'sheet-actions');
  const field = el('label', 'field');
  const input = el('textarea');
  input.rows = 1;
  input.setAttribute('enterkeyhint', 'send');
  field.append(input);
  const send = el('button', 'fab sl');
  send.type = 'submit';
  send.setAttribute('aria-label', 'Send');
  send.append(svg(ICON.send, 20));
  form.append(field, send, ...extra);
  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    const text = input.value.trim();
    if (!text) return;
    input.value = '';
    try {
      await onSend(text);
    } catch {
      input.value = text;
    }
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && finePointer) {
      e.preventDefault();
      form.requestSubmit();
    }
  });
  return { form, input };
}

export async function openWorker(id) {
  const r = await getJSON(`api/workers/${encodeURIComponent(id)}`);
  if (!r) return;
  const w = r.worker;

  const badge = el('span', 'wbadge');
  badge.append(el('span'));
  const titles = el('div');
  const sub = el('div', 'sub');
  titles.append(el('h2', null, w.name), sub);
  const head = el('div', 'whead');
  head.append(badge, titles);

  const facts = el('div', 'facts');
  const laptop = laptopButton(w);
  facts.append(el('span', 'mono', home(w.cwd)), el('span', null, w.model), el('span', null, `started ${clock(w.created)}`), laptop);

  const question = el('div', 'question');
  const log = el('div', 'wlog');
  sheet.renderer = new Renderer(log);
  r.events.forEach((ev) => sheet.renderer.add(ev));
  sheet.log = log;

  const stop = el('button', 'round-stop sl');
  stop.type = 'button';
  stop.setAttribute('aria-label', 'Stop');
  stop.append(el('span'));
  stop.addEventListener('click', () => post(`api/workers/${encodeURIComponent(w.id)}/stop`, {}).catch(() => {}));
  const { form, input } = composer((text) => post(`api/workers/${encodeURIComponent(w.id)}/send`, { text }), [stop]);

  sheet.body.replaceChildren(head, facts, taskRow(w.task), question, log, form);
  showSheet('worker');
  sheet.head = { badge, sub, stop, input, question, laptop };
  updateHead(w);
  log.scrollTop = log.scrollHeight;
}

export function sheetEvent(ev) {
  if (!sheet.renderer || sheet.worker?.id !== ev.worker) return;
  const log = sheet.log;
  const stick = log.scrollHeight - log.scrollTop - log.clientHeight < 80;
  sheet.renderer.add(ev.ev);
  if (stick) log.scrollTop = log.scrollHeight;
}

// ---------- a session the user started ----------

export async function openSession(pid) {
  const r = await getJSON(`api/sessions/${pid}`);
  if (!r) return;
  const s = r.session;
  if (s.session_id) markSeen(s);
  changed();

  const l = s.session_id ? look(s) : { label: s.status, tone: 'idle' };
  const badge = el('span', `wbadge ${l.tone}`);
  badge.append(el('span'));
  const titles = el('div');
  titles.append(el('h2', null, s.name), el('div', 'sub', `${l.label === 'new reply' ? 'idle' : l.label} for ${short(s.since).replace('now', 'a moment')}`));
  const head = el('div', 'whead');
  head.append(badge, titles);
  const facts = el('div', 'facts');
  facts.append(el('span', 'mono', home(s.cwd)), el('span', null, s.kind), el('span', null, `last active ${clock(s.active)}`));
  const parts = [head, facts];

  for (const q of r.questions) {
    const box = el('div', 'squestion');
    box.append(el('b', null, 'Waiting on a question in the terminal'));
    for (const item of q.questions) {
      box.append(el('p', null, item.question + (item.multiSelect ? ' (pick any)' : '')));
      const ol = el('ol');
      for (const o of item.options || []) {
        const li = el('li');
        li.append(el('span', null, o.label));
        if (o.description) li.append(el('small', null, o.description));
        ol.append(li);
      }
      box.append(ol);
    }
    parts.push(box);
  }
  for (const c of r.open_calls) {
    const running = el('div', 'running');
    running.append(el('span', null, `${c.tool} since ${clock(Date.parse(c.since) / 1000)}`), el('code', null, c.what));
    parts.push(running);
  }

  const log = el('div', 'wlog');
  const msgs = [...r.user.map(([t, text]) => ({ t, type: 'user', text })), ...r.assistant.map(([t, text]) => ({ t, type: 'assistant', text }))]
    .sort((a, b) => a.t.localeCompare(b.t));
  for (const m of msgs) log.append(message({ ...m, t: Date.parse(m.t) / 1000 }));
  if (!msgs.length) log.append(el('div', 'note', 'No messages yet.'));
  parts.push(log, el('p', 'usage-note', 'Read-only. Ask seimei to message this session.'));
  sheet.body.replaceChildren(...parts);
  showSheet('session');
  sheet.pid = s.pid;
  sheet.sig = s.status + s.question + s.active;
  sheet.body.scrollTop = sheet.body.scrollHeight;
}
