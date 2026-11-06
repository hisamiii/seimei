// Side threads: their list and archive, and the sheets of a thread's page (details, its model, send to seimei, make it a shikigami).
// The page itself is the agent page's (agents.js, kind 'thread').
import { $, S, hooks, el, svg, at, home, getJSON, snack } from './util.js';
import { sheet, showSheet, closeSheet } from './sheet.js';
import { row, fromThread, postJSON, ARCH, openWorker, setArchived, threadRows } from './agents.js';

// Two speech bubbles: a side conversation.
export const THR = 'M4.5 4.5h9A1.5 1.5 0 0 1 15 6v5.5A1.5 1.5 0 0 1 13.5 13H8.5L5 16v-3h-.5A1.5 1.5 0 0 1 3 11.5V6a1.5 1.5 0 0 1 1.5-1.5zM18 8.5h1.5A1.5 1.5 0 0 1 21 10v5.5a1.5 1.5 0 0 1-1.5 1.5H19v3l-3.5-3h-4A1.5 1.5 0 0 1 10 15.5V16';
const AGENTS = 'M6 4h3.5A1.5 1.5 0 0 1 11 5.5V9a1.5 1.5 0 0 1-1.5 1.5H6A1.5 1.5 0 0 1 4.5 9V5.5A1.5 1.5 0 0 1 6 4zM14.5 4H18a1.5 1.5 0 0 1 1.5 1.5V9A1.5 1.5 0 0 1 18 10.5h-3.5A1.5 1.5 0 0 1 13 9V5.5A1.5 1.5 0 0 1 14.5 4zM6 13.5h3.5A1.5 1.5 0 0 1 11 15v3.5A1.5 1.5 0 0 1 9.5 20H6a1.5 1.5 0 0 1-1.5-1.5V15A1.5 1.5 0 0 1 6 13.5zM14.5 13.5H18a1.5 1.5 0 0 1 1.5 1.5v3.5A1.5 1.5 0 0 1 18 20h-3.5a1.5 1.5 0 0 1-1.5-1.5V15a1.5 1.5 0 0 1 1.5-1.5z';
const CHEV = 'M9 6l6 6-6 6';
const MODELS = ['opus', 'sonnet', 'haiku'];
const EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max'];
const cap = (t) => t.charAt(0).toUpperCase() + t.slice(1);

const page = $('threads');

// The open threads come on the stream (crates/core/src/workers/thread.rs thread_listing); all of them, archived too, from api/threads.
S.threadsArchived = 0;
S.threadsAll = null;
// The folder seimei last summoned in: where a shikigami made from a thread starts.
let cwd = '';

export function setThreads(ev) {
  S.threads = ev.list || [];
  S.threadsArchived = ev.archived || 0;
  threadRows(S.threads);
  if (S.tab === 'threads') refreshAll();
  hooks.chrome();
}

let fetched = 0;
async function refreshAll(force = false) {
  if (!force && Date.now() - fetched < 2000) return;
  fetched = Date.now();
  const r = await getJSON('api/threads');
  if (!r) return;
  S.threadsAll = r.threads;
  cwd = r.cwd || cwd;
  if (S.tab === 'threads') renderThreads();
}

export function threadSeen(id, last) {
  postJSON(`api/threads/${id}/seen`, { last });
}

// ---------- the list, and its archive ----------

let archView = false;
let archQuery = '';
export const threadsArchOpen = () => archView;
export function closeThreadsArch() {
  archView = false;
  archQuery = '';
  renderThreads();
}
export function openThreadsArch() {
  archView = true;
  history.pushState({ tarchive: true }, '');
  renderThreads();
  page.scrollTop = 0;
  refreshAll(true);
}

export function showThreads() {
  if (archView && !history.state?.tarchive) {
    archView = false;
    archQuery = '';
  }
  renderThreads();
  refreshAll(true);
}
hooks.threads = () => { if (S.tab === 'threads') refreshAll(true); };

function group(title, cls, items, q) {
  const sec = el('section', `agroup ${cls}`);
  const h = el('h2', null, title);
  h.hidden = !title;
  sec.append(h, ...items.map((x) => row(x, q)));
  return sec;
}

function archRow(n) {
  const b = el('button', 'archrow sl');
  b.type = 'button';
  const t = el('span', 'txt');
  t.append(el('b', null, 'Archived'), el('span', null, `${n} thread${n === 1 ? '' : 's'}`));
  const tl = el('span', 'tile done');
  tl.append(svg(ARCH, 20));
  b.append(tl, t, svg(CHEV, 20));
  b.addEventListener('click', openThreadsArch);
  return b;
}

function archPage() {
  const list = (S.threadsAll || []).filter((t) => t.archived).map(fromThread);
  const q = archQuery.trim().toLowerCase();
  const hits = q ? list.filter((x) => x.find.some((f) => f && f.toLowerCase().includes(q))) : list;
  const head = el('div', 'ahead');
  const back = el('button', 'back sl');
  back.type = 'button';
  back.setAttribute('aria-label', 'Back to side threads');
  back.append(svg('M15 5l-7 7 7 7', 22));
  back.addEventListener('click', () => history.back());
  head.append(back, el('h2', null, 'Archived'), el('span', 'count', q ? `${hits.length} of ${list.length}` : `${list.length}`));
  const search = el('label', 'asearch');
  const input = el('input');
  input.type = 'search';
  input.placeholder = 'Search';
  input.setAttribute('aria-label', 'Search the archived threads');
  input.value = archQuery;
  input.addEventListener('input', () => {
    archQuery = input.value;
    const pos = input.selectionStart;
    renderThreads();
    const again = page.querySelector('.asearch input');
    again.focus();
    again.setSelectionRange(pos, pos);
  });
  search.append(svg('M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM20 20l-3.5-3.5', 20), input);
  const parts = [head, search];
  if (hits.length) parts.push(group(q ? 'Matches' : '', 'done', hits, q));
  else parts.push(el('div', 'empty', q ? 'No matches' : 'Nothing archived. Quiet ones move here after a day.'));
  return parts;
}

export function renderThreads() {
  const top = page.scrollTop;
  if (archView) {
    page.replaceChildren(...archPage());
    page.scrollTop = top;
    return;
  }
  const open = S.threads.map(fromThread);
  const work = open.filter((x) => x.state === 'work');
  const idle = open.filter((x) => x.state !== 'work');
  const parts = [];
  if (work.length) parts.push(group('Working', 'work', work));
  if (idle.length) parts.push(group('Idle', 'idle', idle));
  if (S.threadsArchived) parts.push(archRow(S.threadsArchived));
  if (!parts.length) {
    const empty = el('div', 'empty');
    empty.append(el('b', null, 'No side threads yet'), el('span', null, "Start one with the pen above, or hold a message of seimei's"));
    parts.push(empty);
  }
  page.replaceChildren(...parts);
  page.scrollTop = top;
}
setInterval(() => { if (S.tab === 'threads' && !archView) renderThreads(); }, 30000);

// ---------- a thread's sheets ----------

function buttons(cancelLabel, okLabel, onCancel, onOk) {
  const cancel = el('button', 'btn text sl', cancelLabel);
  const ok = el('button', 'btn filled sl', okLabel);
  cancel.type = ok.type = 'button';
  cancel.addEventListener('click', onCancel);
  ok.addEventListener('click', onOk);
  const btns = el('div', 'dbtns');
  btns.append(cancel, ok);
  return { btns, ok };
}

function errNote() {
  const err = el('p', 'snote error');
  err.hidden = true;
  return err;
}

const fail = (err, text) => {
  err.textContent = text;
  err.hidden = false;
};

// Details: its model (tunable), context and start; then Make this a shikigami and Archive.
export function threadDetails(r, changed) {
  const titles = el('div', 'dhead');
  titles.append(el('h2', null, r.name), el('div', 'sub', 'Side thread'));
  const facts = el('dl', 'kv');
  const fact = (k, v) => v && facts.append(el('dt', null, k), el('dd', null, v));
  if (r.shikigami) fact('Model', r.model);
  else {
    const dd = el('dd', 'tune sl');
    dd.append(el('span', null, r.model || 'default'), svg(CHEV, 16));
    dd.tabIndex = 0;
    dd.addEventListener('click', () => modelSheet(r, changed));
    facts.append(el('dt', 'tune', 'Model'), dd);
  }
  fact('Context', r.context != null && `${r.context}% used`);
  fact('Started', cap(at(r.created).replace(/^at /, 'today at ')));
  const actions = el('div', 'dactions');
  if (!r.shikigami) {
    const make = el('button', 'dbtn sl');
    make.type = 'button';
    make.append(svg(AGENTS, 18), el('span', null, 'Make this a shikigami'));
    make.addEventListener('click', () => shikigamiSheet(r, changed));
    actions.append(make);
  }
  const arch = el('button', 'dbtn tonal sl');
  arch.type = 'button';
  arch.append(svg(ARCH, 18), el('span', null, r.archived ? 'Unarchive' : 'Archive'));
  arch.addEventListener('click', () => { closeSheet(); setArchived(fromThread(r), !r.archived); });
  actions.append(arch);
  sheet.body.replaceChildren(titles, facts, actions);
  showSheet('details');
}

function seg(values, pick, set) {
  const box = el('div', 'seg');
  for (const v of values) {
    const b = el('button', `sl${pick === v ? ' on' : ''}`, v);
    b.type = 'button';
    b.addEventListener('click', () => { set(v); box.querySelectorAll('button').forEach((x) => x.classList.toggle('on', x === b)); });
    box.append(b);
  }
  return box;
}

function modelSheet(r, changed) {
  let model = r.model;
  const err = errNote();
  const { btns, ok } = buttons('Cancel', 'Apply', () => threadDetails(r, changed), async () => {
    if (model === r.model) return threadDetails(r, changed);
    ok.disabled = true;
    const res = await postJSON(`api/threads/${r.id}/tune`, { model });
    ok.disabled = false;
    if (!res.ok) return fail(err, res.error || "Couldn't change it");
    const next = { ...r, ...res };
    changed(next);
    threadDetails(next, changed);
  });
  sheet.body.replaceChildren(el('h2', 'change-h', `Change ${r.name}`), el('p', 'lab', 'Model'), seg(MODELS, model, (v) => { model = v; }),
    el('p', 'hint', 'From its next message. A new model rebuilds the cache once.'), err, btns);
  showSheet('change');
}

// Send to seimei: the thread writes a short summary, the user edits it, and it goes into the chat as their message.
export async function sendSheet(r) {
  const box = el('textarea', 'sfield');
  box.placeholder = 'Writing a summary…';
  box.disabled = true;
  box.setAttribute('aria-label', 'What goes to seimei');
  const err = errNote();
  const { btns, ok } = buttons('Cancel', 'Send', () => closeSheet(), async () => {
    const text = box.value.trim();
    if (!text) return;
    ok.disabled = true;
    const res = await postJSON(`api/threads/${r.id}/post`, { text });
    ok.disabled = false;
    if (!res.ok) return fail(err, res.error);
    closeSheet();
    snack('Sent to seimei', { label: 'Show', run: () => hooks.home?.() });
  });
  ok.disabled = true;
  box.addEventListener('input', () => { ok.disabled = !box.value.trim(); });
  sheet.body.replaceChildren(el('h2', 'change-h', 'Send to seimei'), box, err, btns);
  showSheet('tsend');
  const res = await postJSON(`api/threads/${r.id}/summary`, {});
  if (sheet.view !== 'tsend' || !box.isConnected) return;
  box.disabled = false;
  box.placeholder = 'What seimei should know';
  if (res.ok) box.value = res.text || '';
  else fail(err, `${res.error}. Write it yourself.`);
  ok.disabled = !box.value.trim();
  box.focus();
}

// Make this a shikigami: a summon with the thread's text as context; the thread is read-only after.
async function shikigamiSheet(r, changed) {
  if (!cwd) await refreshAll(true);
  const pick = { model: r.model || 'opus', effort: 'high' };
  const field = (tag, cls, value, label) => {
    const f = el(tag, cls);
    if (tag === 'input') f.type = 'text';
    else f.rows = 3;
    f.value = value;
    f.setAttribute('aria-label', label);
    return f;
  };
  const name = field('input', 'sfield', r.name, 'Name');
  const folder = field('input', 'sfield mono', home(cwd) || '~', 'Folder');
  folder.spellcheck = false;
  const task = field('textarea', 'sfield', '', 'Task');
  task.placeholder = 'What it should do';
  const err = errNote();
  const { btns, ok } = buttons('Cancel', 'Summon', () => threadDetails(r, changed), async () => {
    ok.disabled = true;
    const res = await postJSON(`api/threads/${r.id}/shikigami`, {
      name: name.value.trim(), cwd: folder.value.trim(), model: pick.model, effort: pick.effort, task: task.value.trim(),
    });
    ok.disabled = false;
    if (!res.ok) return fail(err, res.error);
    closeSheet();
    changed(res.thread);
    snack(`Summoned ${res.name}`, { label: 'Open', run: () => openWorker(res.worker) });
  });
  ok.disabled = true;
  task.addEventListener('input', () => { ok.disabled = !task.value.trim(); });
  sheet.body.replaceChildren(el('h2', 'change-h', 'Make this a shikigami'),
    el('p', 'lab', 'Name'), name, el('p', 'lab', 'Folder'), folder,
    el('p', 'lab', 'Model'), seg(MODELS, pick.model, (v) => { pick.model = v; }),
    el('p', 'lab', 'Effort'), seg(EFFORTS, pick.effort, (v) => { pick.effort = v; }),
    el('p', 'lab', 'Task'), task, err, btns);
  showSheet('tshiki');
}
