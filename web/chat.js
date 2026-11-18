// The conversation with seimei: rendering events, live text, approval cards, and the composer.
import { md } from './md.js';
import { $, S, hooks, el, svg, clock, dayKey, dayLabel, home, plural, post, view, snack, finePointer, toLogin, ICON, wearShape, say, enter, leave } from './util.js';
import { openWorker, openThread, openDraft } from './agents.js';
import { THR } from './threads.js';
import { sheet, showSheet, closeSheet } from './sheet.js';
import { renderTasks } from './tasks.js';
import { sentDictated } from './mic.js';

const log = $('log');
const thread = $('thread');
const traj = $('traj');
const older = $('older'), olderT = $('older-t'), working = $('working');
const input = $('input'), sendBtn = $('send'), stopBtn = $('stop'), talkBtn = $('talk');

let live = null;
let liveText = '';

export const nearBottom = () => log.scrollHeight - log.scrollTop - log.clientHeight < 120;
// Whether the reader sits at the bottom; content that grows later (pictures loading) keeps them there.
let pinned = true;
export const toBottom = () => {
  log.scrollTop = log.scrollHeight;
  pinned = true;
  jumpState();
};
export const following = () => pinned;
// Only a move up lets go: a scroll event can come late (a background tab fires it on return), after the thread grew below.
let lastTop = 0;
log.addEventListener('scroll', () => {
  pinned = nearBottom() || (pinned && log.scrollTop >= lastTop);
  lastTop = log.scrollTop;
  jumpState();
}, { passive: true });

// Jump to latest: while the reader is scrolled up, a button above the composer counts the rows that came in below.
// It goes to the newest unanswered card above the view, else to the end.
let missed = 0;
const jump = el('button', 'jump sl');
jump.type = 'button';
jump.hidden = true;
jump.append(svg('M12 5v14M6 13l6 6 6-6', 20), el('span', null, 'Latest'), el('b'));
document.body.append(jump);
function jumpState() {
  if (pinned) missed = 0;
  const show = !pinned && S.lane !== 'k' && log.scrollHeight - log.scrollTop - log.clientHeight > log.clientHeight / 2;
  jump.hidden = !show;
  if (!show) return;
  const b = jump.querySelector('b');
  b.hidden = !missed;
  b.textContent = String(missed);
  jump.setAttribute('aria-label', missed ? `Jump to latest, ${missed} new` : 'Jump to latest');
  const c = document.getElementById('composer')?.getBoundingClientRect();
  if (c?.height) jump.style.bottom = `${Math.round(innerHeight - c.top + 28)}px`;
}
jump.addEventListener('click', () => {
  const view = log.getBoundingClientRect();
  const open = [...(S.lane === 't' ? traj : thread).querySelectorAll('.wcard.approval:not(.settled), .wcard.asking')].pop();
  if (open && open.getBoundingClientRect().bottom < view.top) {
    open.scrollIntoView({ block: 'center', behavior: 'smooth' });
    return;
  }
  log.scrollTo({ top: log.scrollHeight, behavior: 'smooth' });
});
addEventListener('resize', jumpState);
// Pictures loading, fonts swapping in and card buttons all grow the thread after it is drawn.
const grow = new ResizeObserver(() => { if (pinned) log.scrollTop = log.scrollHeight; });
grow.observe(thread);
grow.observe(traj);

// ---------- tool activity ----------

const ICONS = {
  Bash: 'M4 17l5-5-5-5M11 19h9',
  Read: 'M6 3h9l4 4v14H6zM14 3v5h5',
  Edit: 'M4 20h4L19 9l-4-4L4 16zM13 7l4 4',
  Search: 'M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM16 16l4 4',
  Web: 'M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3 12h18M12 3c3 3.5 3 14.5 0 18M12 3c-3 3.5-3 14.5 0 18',
  Tool: 'M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2.4-2.4z',
};

// The built-in plugins' tools (mcp__browser__browser_tabs) read like seimei's own, as they were named before them.
const ours = name => name.replace(/^mcp__(desktop|browser|music|queue)__/, 'mcp__seimei__');

function phrase(name, n) {
  name = ours(name);
  if (name === 'Bash') return `ran ${plural(n, 'a command', 'commands')}`;
  if (name === 'Read') return `read ${plural(n, 'a file', 'files')}`;
  if (/^(Edit|Write|MultiEdit|NotebookEdit)$/.test(name)) return `edited ${plural(n, 'a file', 'files')}`;
  if (/^(Grep|Glob)$/.test(name)) return n === 1 ? 'searched the files' : `searched ${n} times`;
  if (name === 'WebSearch') return 'searched the web';
  if (name === 'WebFetch') return `opened ${plural(n, 'a page', 'pages')}`;
  if (/^(Agent|Task)$/.test(name)) return `started ${plural(n, 'a subagent', 'subagents')}`;
  if (/^(TodoWrite|TaskCreate|TaskUpdate|TaskList|TaskGet)$/.test(name)) return 'updated its steps';
  if (name === 'ToolSearch') return 'loaded tools';
  if (name === 'AskUserQuestion') return n === 1 ? 'asked you a question' : `asked you ${n} questions`;
  if (name === 'mcp__seimei__workers') return 'checked the shikigami';
  if (name === 'mcp__seimei__peek') return n === 1 ? 'looked in on a shikigami' : `looked in on shikigami ${n} times`;
  if (name === 'mcp__seimei__message') return n === 1 ? 'messaged a shikigami' : `sent ${n} shikigami messages`;
  if (name === 'mcp__seimei__stop') return 'stopped a shikigami';
  if (name === 'mcp__seimei__show_image') return n === 1 ? 'showed you an image' : `showed you ${n} images`;
  if (name === 'mcp__seimei__send_file') return n === 1 ? 'sent you a file' : `sent you ${n} files`;
  if (name === 'mcp__seimei__browser_tabs') return 'looked at your tabs';
  if (name === 'mcp__seimei__browser_read') return n === 1 ? 'read a page' : `read ${n} page parts`;
  if (name === 'mcp__seimei__browser_open') return n === 1 ? 'opened a tab' : `opened ${n} tabs`;
  if (name === 'mcp__seimei__browser_switch') return 'switched tabs';
  if (name === 'mcp__seimei__browser_close') return 'closed tabs';
  if (name === 'mcp__seimei__browser_download') return n === 1 ? 'started a download' : `started ${n} downloads`;
  if (name === 'mcp__seimei__browser_downloads') return 'checked the downloads';
  const short = name.startsWith('mcp__') ? name.split('__').pop().replace(/_/g, ' ') : name;
  return n === 1 ? `used ${short}` : `used ${short} ${n} times`;
}

// What a call that is still running does, for the live pill and the Agents list.
export function doing(name) {
  if (name === 'Bash') return 'running a command';
  if (name === 'Read') return 'reading a file';
  if (/^(Edit|Write|MultiEdit|NotebookEdit)$/.test(name)) return 'editing a file';
  if (/^(Grep|Glob)$/.test(name)) return 'searching the files';
  if (name === 'WebSearch') return 'searching the web';
  if (name === 'WebFetch') return 'opening a page';
  if (/^(Agent|Task)$/.test(name)) return 'running a subagent';
  const short = name.startsWith('mcp__') ? name.split('__').pop().replace(/_/g, ' ') : name;
  return `using ${short}`;
}

// How long something has run: 42s, 4m 12s, 1h 5m.
export function elapsed(since) {
  const s = Math.max(0, Math.floor(Date.now() / 1000 - since));
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m ${s % 60}s` : `${Math.floor(s / 3600)}h ${Math.floor(s % 3600 / 60)}m`;
}

function iconFor(name) {
  if (name === 'Bash') return ICONS.Bash;
  if (name === 'Read') return ICONS.Read;
  if (/^(Edit|Write|MultiEdit|NotebookEdit)$/.test(name)) return ICONS.Edit;
  if (/^(Grep|Glob)$/.test(name)) return ICONS.Search;
  if (/^Web/.test(name)) return ICONS.Web;
  return ICONS.Tool;
}

class Activity {
  constructor() {
    this.node = el('div', 'activity');
    this.chip = el('button', 'chip sl');
    this.chip.type = 'button';
    this.calls = el('div', 'calls');
    this.calls.hidden = true;
    this.node.append(this.chip, this.calls);
    this.counts = new Map();
    this.rows = new Map();
    this.running = 0;
    this.failed = 0;
    this.first = null;
    this.chip.addEventListener('click', () => {
      this.calls.hidden = !this.calls.hidden;
      this.node.classList.toggle('open', !this.calls.hidden);
    });
  }

  add(ev) {
    this.first ??= ev.name;
    this.counts.set(ev.name, (this.counts.get(ev.name) || 0) + 1);
    const row = el('button', 'call sl run');
    row.type = 'button';
    row.append(el('b', null, ev.name.startsWith('mcp__') ? ev.name.split('__').pop() : ev.name), el('span', null, ev.summary));
    const out = el('pre', 'out');
    out.hidden = true;
    row.addEventListener('click', () => { if (out.textContent) out.hidden = !out.hidden; });
    this.calls.append(row, out);
    this.rows.set(ev.tool_id, { row, out, name: ev.name });
    this.running++;
    this.label();
  }

  // The call still running, under the chip: its command and its last output lines, and how long it has run.
  live(id, info) {
    const r = this.rows.get(id);
    if (!r || !r.row.classList.contains('run')) return false;
    if (!this.box) {
      this.box = el('div', 'livebox');
      this.cmd = el('code', 'cmd');
      this.tail = el('pre', 'tail');
      this.box.append(this.cmd, this.tail);
      this.chip.after(this.box);
    }
    const changed = this.live_ !== id;
    this.live_ = id;
    this.liveName = info.tool;
    this.since = info.since;
    this.cmd.textContent = info.what || '';
    this.tail.textContent = (info.output || []).join('\n');
    this.tail.hidden = !info.output?.length;
    this.node.classList.add('running');
    if (changed) this.label();
    this.tick();
    return true;
  }

  tick() {
    if (this.clock && this.since) this.clock.textContent = elapsed(this.since);
  }

  unlive() {
    this.box?.remove();
    this.box = null;
    this.live_ = null;
    this.node.classList.remove('running');
    this.label();
  }

  result(ev) {
    const r = this.rows.get(ev.tool_id);
    if (ev.tool_id === this.live_) this.unlive();
    if (!r || !r.row.classList.contains('run')) return;
    r.row.classList.remove('run');
    r.row.classList.toggle('err', ev.error);
    r.out.textContent = ev.text || 'No output';
    this.running--;
    if (ev.error) this.failed++;
    this.label();
  }

  label() {
    // The call under way reads in the present, live box or not yet
    const now = this.live_ ? this.liveName : [...this.rows.values()].reverse().find((r) => r.row.classList.contains('run'))?.name;
    const counts = new Map(this.counts);
    if (now) counts.set(now, counts.get(now) - 1);
    const parts = [...counts].filter(([, n]) => n > 0).map(([name, n]) => phrase(name, n));
    if (now) parts.push(`${doing(now)}…`);
    const text = parts.join(', ') + (this.failed ? ` · ${this.failed} failed` : '');
    // A failure shows on the collapsed chip too, not only in its row.
    this.chip.classList.toggle('err', this.failed > 0);
    const chev = svg(ICON.chev);
    chev.classList.add('chev');
    this.clock = this.live_ ? el('span', 'clock') : null;
    this.chip.replaceChildren(
      this.running ? loader('bare small') : svg(iconFor(this.first)),
      el('span', null, text.charAt(0).toUpperCase() + text.slice(1)),
      ...(this.clock ? [this.clock] : []),
      chev,
    );
    this.tick();
  }
}

// ---------- cards ----------

export const loader = (cls = '') => {
  const l = el('span', `loader ${cls}`.trim());
  l.append(el('i'));
  return l;
};

// The question glyph of the Agents and Tasks ask tiles
const ASK = 'M4 5h16v11H9l-5 4zM10 9.2a2 2 0 1 1 2.6 1.9c-.4.2-.6.5-.6.9M12 13.6v.1';
const CHEV = 'M9.5 6l6 6-6 6';
// The model as its product name ("opus" as "Opus"), then the effort
const model = (ev) => [ev.model && cap(ev.model), ev.effort].filter(Boolean).join(' · ');
const cap = (t) => t.charAt(0).toUpperCase() + t.slice(1);
const oneSentence = (t) => (t || '').trim().split(/(?<=[.!?])\s/)[0].replace(/\.$/, '');

// A quiet line naming a shikigami, one tap from its page: what seimei's own sentence above didn't say.
function evline(ev, cls, ...parts) {
  const b = el('button', `evline ${cls}sl`);
  b.type = 'button';
  b.dataset.worker = ev.worker;
  const text = el('span');
  text.append(...parts);
  b.append(text, svg(CHEV, 20));
  return b;
}

// The side thread a message of the user's came from.
function threadLine(ev) {
  const b = el('button', 'evline sl');
  b.type = 'button';
  b.dataset.thread = ev.thread;
  const text = el('span');
  text.append(el('b', null, ev.thread_name || 'Side thread'), el('small', null, 'Side thread'));
  b.append(text, svg(CHEV, 20));
  return b;
}

// What a shikigami event shows in chat, or nothing: the user wrote their own reply on its page.
function workerCard(ev) {
  const name = el('b', null, ev.name);
  if (ev.event === 'replied') return null;
  if (ev.event === 'summoned') return evline(ev, '', name, el('small', null, model(ev)));
  if (ev.event === 'failed') return evline(ev, 'failed ', name, ' failed', el('small', null, oneSentence(firstLine(ev.text))));
  if (ev.event === 'done') {
    const report = el('div', 'report');
    report.dataset.worker = ev.worker;
    report.append(evline(ev, '', name, ' finished'));
    if (ev.text) {
      const body = el('div', 'rbody');
      body.innerHTML = md(ev.text);
      report.append(body);
      clipWatch.observe(body);
    }
    return report;
  }
  if (ev.event !== 'asking') return el('div', 'note', `${ev.name} ${ev.event === 'stopped' ? 'was stopped' : 'was interrupted by a restart'}`);
  // A decision: the warm card, the asker's name as its title, the question its body, the answer one tap away
  const card = el('div', 'wcard asking');
  card.dataset.worker = ev.worker;
  const badge = el('span', 'badge');
  wearShape(badge, ev.name);
  badge.append(svg(ASK, 22));
  const head = el('div', 'head');
  const title = el('div', 'title');
  title.append(name);
  head.append(badge, title);
  card.append(head);
  if (ev.text) {
    const body = el('div', 'body');
    body.innerHTML = md(ev.text);
    card.append(body);
    clipWatch.observe(body);
  }
  const foot = el('div', 'foot');
  const open = el('button', 'btn tonal sl morph', `Open ${ev.name}`);
  const answer = el('button', 'btn filled sl morph', 'Answer');
  open.type = answer.type = 'button';
  answer.dataset.answer = '1';
  foot.append(open, answer);
  card.append(foot);
  return card;
}

// Long card bodies get "Show more", right under the text (above a question's buttons).
// Watched while the card lives: a card drawn on a hidden tab has no size yet, and a new text size can clip a body or free it.
const clipWatch = new ResizeObserver((entries) => {
  for (const { target: body } of entries) {
    const card = body.parentElement;
    if (!body.clientHeight || card.classList.contains('open')) continue;
    const clipped = body.scrollHeight > body.clientHeight + 4;
    card.classList.toggle('clipped', clipped);
    let more = card.querySelector(':scope > .more');
    if (clipped && !more) {
      more = el('button', 'more sl', 'Show more');
      more.type = 'button';
      more.addEventListener('click', () => {
        card.classList.toggle('open');
        more.textContent = card.classList.contains('open') ? 'Show less' : 'Show more';
      });
      body.after(more);
    }
    if (more) more.hidden = !clipped;
  }
});

// Cards by approval id, and outcomes seen so far (an older page can load a card after its outcome).
const approvals = new Map();
const outcomes = new Map();
const LOCK = 'M7 11V8a5 5 0 0 1 10 0v3M6 11h12a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1z';
const PLAN = 'M8 6h12M8 12h12M8 18h12M4 6h.01M4 12h.01M4 18h.01';
const PEN = 'M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4';
const OUTCOME = { approved: 'Allowed', denied: 'Denied', restart: 'Canceled by a restart', cancelled: 'Canceled' };
const ALLOW = 'M5 12.5l4.5 4.5L19 7.5';
const ASKS = 'M4 5h16v11H9l-5 4zM10 9.2a2 2 0 1 1 2.6 1.9c-.4.2-.6.5-.6.9M12 13.6v.1';

const SUB = { desktop: 'a desktop action' };

function approvalSub(ev) {
  if (ev.options) return ev.sub || (ev.action === 'edit' ? home((ev.detail || '').split('\n')[0]).split('/').pop() : '');
  if (ev.action === 'edit') return home((ev.detail || '').split('\n')[0]);
  if (ev.action !== 'command') return SUB[ev.action] || '';
  const words = new Set((ev.detail || '').split(/[\s'"=]+/));
  const s = (S.sessions || []).find((x) => x.name && words.has(x.name));
  return s ? `session ${s.name}${s.project ? ` · ${s.project}` : ''}` : '';
}

// An edit's detail: the path, a blank line, then a diff (- and + lines) or the new file.
function diffBody(detail) {
  const body = el('div', 'body cmd');
  const [path, ...rest] = (detail || '').split('\n');
  body.append(el('span', 'path', home(path)), '\n');
  for (const line of rest.slice(rest[0] === '' ? 1 : 0)) {
    const cls = line.startsWith('-') ? 'del' : line.startsWith('+') ? 'add' : null;
    body.append(cls ? el('span', cls, line) : line, '\n');
  }
  return body;
}

// A rule option: "Always allow <code>rule</code> in this project", marked when the rule lets it run anything.
function choice(o) {
  const b = el('button', 'choice sl');
  b.type = 'button';
  if (o.kind === 'rule' && o.rule) {
    b.append(o.label.startsWith('Always') ? 'Always allow ' : 'Allow ', el('code', null, o.rule), ' ', el('small', null, o.where || ''));
    if (o.broad) b.append(' ', el('small', 'warn', '· runs any code'));
  } else if (o.where && o.label.endsWith(o.where)) {
    b.append(o.label.slice(0, -o.where.length).trim(), ' ', el('small', null, o.where));
  } else b.textContent = o.label;
  // A question's option may explain itself
  if (o.kind === 'choice' && o.sub) b.append(' ', el('small', null, o.sub));
  return b;
}

function approvalCard(ev) {
  const perm = Array.isArray(ev.options);
  // seimei's AskUserQuestion: the options are the answers, the note writes one, Skip leaves it
  const question = ev.action === 'question';
  const card = el('div', `wcard approval${perm ? ' perm' : ''}${ev.action === 'plan' ? ' plan' : ''}${question ? ' question' : ''}`);
  const badge = el('span', 'badge');
  badge.append(svg(ev.action === 'plan' ? PLAN : question ? ASKS : LOCK, 18));
  const head = el('div', 'head');
  const title = el('div', 'title');
  const who = ev.subagent ? `${ev.who}'s subagent` : ev.who;
  title.append(el('b', null, question ? `${who} asks you` : `${who} wants to ${ev.target}`));
  // The subtitle names what it touches, not the title again: the session a command names, the file an edit writes.
  const sub = approvalSub(ev);
  if (sub) title.append(el('span', 'sub', sub));
  head.append(badge, title);
  let body;
  if (perm && ev.action === 'edit') body = diffBody(ev.detail);
  else if (perm && ev.action === 'plan') {
    body = el('div', 'body plan');
    body.innerHTML = md(ev.detail || '');
  } else body = el('div', `body ${question ? 'task' : 'cmd'}`, ev.detail);
  card.append(head, body);

  const opts = perm ? ev.options : [{ id: 'deny', label: 'Deny' }, { id: 'once', label: 'Allow' }];
  const once = opts.find((o) => o.id === 'once');
  const denyOpt = opts.find((o) => o.id === 'deny');
  const buttons = [];
  const answer = async (decision, option, note, btn) => {
    // The others step back while the tapped one is on its way
    buttons.forEach((b) => { b.disabled = true; b.classList.toggle('held', b !== btn); });
    // Past 200 ms the tapped button shows the answer is on its way.
    const slow = setTimeout(() => {
      if (!btn) return;
      btn.classList.add('busy');
      btn.append(loader('bare'));
    }, 200);
    try {
      await post(`api/approvals/${ev.aid}`, { decision, ...(perm ? { option } : {}), ...(note ? { note } : {}) });
    } catch (e) {
      // 409: answered elsewhere, the event stream settles the card.
      if (e.message === '409' || outcomes.has(ev.aid)) return;
      buttons.forEach((b) => { b.disabled = false; b.classList.remove('held'); });
      snack("Couldn't send your answer", { label: 'Retry', run: () => answer(decision, option, note, btn) });
    } finally {
      clearTimeout(slow);
      btn?.classList.remove('busy');
      btn?.querySelector('.loader')?.remove();
    }
  };
  const extra = opts.filter((o) => o.id !== 'once' && o.id !== 'deny');
  if (extra.length) {
    const list = el('div', 'choices m3x-seg');
    for (const o of extra) {
      const b = choice(o);
      b.addEventListener('click', () => answer('approve', o.id, '', b));
      buttons.push(b);
      list.append(b);
    }
    card.append(list);
  }
  const foot = el('div', 'foot');
  const left = el('span', 'left');
  const deny = el('button', 'btn tonal sl morph', denyOpt?.label || 'Deny');
  const ok = el('button', 'btn filled sl morph');
  ok.append(svg(ALLOW, 20), once?.label || 'Allow');
  deny.type = ok.type = 'button';
  deny.addEventListener('click', () => answer('deny', 'deny', '', deny));
  ok.addEventListener('click', () => answer('approve', 'once', '', ok));
  buttons.push(deny, ok);
  foot.append(left);
  if (perm) {
    // The note goes to the agent with a deny, like the note the agent's own terminal prompt takes.
    const pen = el('button', 'btn icon sl morph');
    pen.type = 'button';
    pen.title = question ? 'Write an answer' : 'Add a note';
    pen.setAttribute('aria-label', pen.title);
    pen.append(svg(PEN, 18));
    buttons.push(pen);
    pen.addEventListener('click', () => {
      const field = el('textarea', 'note');
      field.rows = 2;
      field.placeholder = question ? `Answer ${ev.who}` : `Note for ${ev.who}`;
      const cancel = el('button', 'btn text sl', 'Cancel');
      const send = el('button', 'btn filled sl', question ? 'Answer' : ev.action === 'plan' ? 'Keep planning' : 'Deny with note');
      cancel.type = send.type = 'button';
      const noteFoot = el('div', 'foot');
      noteFoot.append(el('span', 'left'), cancel, send);
      buttons.push(cancel, send);
      cancel.addEventListener('click', () => { field.remove(); noteFoot.remove(); foot.hidden = false; });
      send.addEventListener('click', () => (question ? answer('approve', 'other', field.value.trim(), send)
        : answer('deny', 'deny', field.value.trim(), send)));
      foot.hidden = true;
      card.append(field, noteFoot);
      field.focus();
    });
    foot.append(pen);
  }
  foot.append(deny);
  if (!question) foot.append(ok);
  card.append(foot);
  // 0 for a shikigami's permission card, which waits.
  card.expires = ev.expires || 0;
  approvals.set(ev.aid, card);
  if (outcomes.has(ev.aid)) settle(outcomes.get(ev.aid));
  return card;
}

function settle(done) {
  outcomes.set(done.aid, done);
  const card = approvals.get(done.aid);
  if (!card) return;
  const outcome = done.outcome;
  card.classList.add('settled', outcome);
  card.querySelectorAll('.foot, .choices, textarea.note').forEach((n) => n !== card.querySelector('.foot') && n.remove());
  const foot = card.querySelector('.foot');
  foot.hidden = false;
  const had = card.contains(document.activeElement);
  foot.querySelectorAll('button').forEach((b) => b.remove());
  let text = OUTCOME[outcome] || outcome;
  if (outcome === 'expired') text = card.expires ? 'No answer in time, denied' : 'No answer, denied';
  if (outcome === 'approved' && card.classList.contains('plan')) text = 'Went ahead';
  if (outcome === 'approved' && done.chosen === 'Allow') text = 'Allowed once';
  else if (outcome === 'approved' && done.chosen) text = done.chosen.replace(/^Always allow/, 'Always allowed').replace(/^Allow /, 'Allowed ');
  if (outcome === 'denied' && done.by === 'chat') text = 'Denied: you wrote in the chat';
  else if (outcome === 'denied' && done.note) text = `Denied: “${done.note}”`;
  if (card.classList.contains('question')) {
    text = { approved: `Answered: ${done.chosen || `“${done.note || ''}”`}`, expired: 'No answer in time',
      denied: done.by === 'chat' ? 'You answered in the chat' : 'Skipped' }[outcome] || text;
    // Answered, it is the question and its answer: no header, no warm card
    card.querySelector('.head')?.remove();
  }
  const left = foot.querySelector('.left');
  left.textContent = text;
  left.tabIndex = -1;
  if (had) left.focus();
  render();
}

export const waitingApprovals = () => [...approvals.entries()].filter(([aid]) => !outcomes.has(aid));

function pic(name, size) {
  const img = el('img', 'pic');
  // The aspect ratio reserves the picture's space before it loads, so the thread does not jump.
  if (size?.[0] && size?.[1]) img.style.aspectRatio = `${size[0]} / ${size[1]}`;
  img.src = `api/files/${name}?w=thumb`;
  img.dataset.name = name;
  img.loading = 'lazy';
  img.alt = '';
  return img;
}

// ---------- files ----------

const FILE_ICON = {
  archive: 'M4 7.5 12 3.5l8 4v9l-8 4-8-4zM4 7.5l8 4 8-4M12 11.5v9',
  audio: 'M9 18V5.5l10-2v12.5M9 18a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0zM19 16a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0z',
  video: 'M4 6.5h11v11H4zM15 10.5l5-3v9l-5-3',
  image: 'M6.5 4.5h11a3 3 0 0 1 3 3v9a3 3 0 0 1-3 3h-11a3 3 0 0 1-3-3v-9a3 3 0 0 1 3-3zM20.5 15.5l-4.2-4.2a1.5 1.5 0 0 0-2.1 0L5.5 19.5',
  code: 'M9 8l-4 4 4 4M15 8l4 4-4 4',
  doc: 'M6 3h9l4 4v14H6zM14 3v5h5M9 13h6M9 17h6',
};
const fileIcon = (kind) => FILE_ICON[kind] || FILE_ICON.doc;
const size = (n) => (n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`);
const KIND_LABEL = { pdf: 'PDF', archive: 'Zip', audio: 'Audio', video: 'Video', image: 'Image', code: 'Code', text: 'Text', file: 'File' };

// The server deletes files after 30 days (uploads.rs KEEP_DAYS); older chips show as expired.
const KEEP = 30 * 86400;

// A file as a download chip; the server always serves it as an attachment, never rendered.
export function fileChip(f, t, from = null) {
  const expired = t && Date.now() / 1000 - t > KEEP;
  const a = el(expired ? 'div' : 'a', `fchip${expired ? ' expired' : ' sl'}`);
  if (!expired) {
    a.href = `api/uploads/${f.id}`;
    a.download = f.name;
  }
  const ic = el('span', 'ic');
  ic.append(svg(fileIcon(f.kind), 20));
  const txt = el('span', 'txt');
  const what = from ? `${from} · ${size(f.size)}` : `${size(f.size)} · ${KIND_LABEL[f.kind] || 'File'}`;
  txt.append(el('b', null, f.name), el('small', null, expired ? 'Expired' : what));
  a.append(ic, txt);
  if (!expired) a.append(svg('M12 4v11M7 10.5l5 5 5-5M5 20h14', 18));
  return a;
}

// Whose picture or file it is when not seimei's: a shikigami's line opens its page, a session's only names it.
function sharer(ev, what) {
  if (!ev.from) return [];
  if (ev.worker) return [evline(ev, '', el('b', null, ev.from), ` ${what}`)];
  return [el('small', 'from', `${ev.from} ${what}`)];
}

function sentFile(ev) {
  const fig = el('figure', 'shown file');
  fig.append(...sharer(ev, 'sent you a file'), fileChip({ ...ev, id: ev.file }, ev.t));
  if (ev.caption) fig.append(el('figcaption', null, ev.caption));
  return fig;
}

function message(ev) {
  const node = el('div', `msg ${ev.type}`);
  if (ev.type === 'assistant') node.innerHTML = md(ev.text);
  else {
    if (ev.quote) node.append(anchor(ev.quote, ev.quote_id, ev.quote_lane || 't'));
    if (ev.images?.length) {
      const pics = el('div', `pics n${Math.min(ev.images.length, 3)}${ev.text ? '' : ' only'}`);
      ev.images.forEach((n, i) => pics.append(pic(n, ev.sizes?.[i])));
      node.append(pics);
    }
    if (ev.files?.length) {
      const box = el('div', 'files');
      ev.files.forEach((f) => box.append(fileChip(f, ev.t)));
      node.append(box);
    }
    if (ev.text) node.append(document.createTextNode(ev.text));
  }
  node.dataset.t = ev.t;
  return node;
}

function shown(ev) {
  const fig = el('figure', 'shown');
  const img = pic(ev.file, ev.w && [ev.w, ev.h]);
  // As wide as it fits, never upscaled, and no taller than 60vh: the same box the loaded picture gets.
  if (ev.w && ev.h) img.style.width = `min(100%, ${ev.w}px, calc(60vh * ${(ev.w / ev.h).toFixed(4)}))`;
  fig.append(...sharer(ev, 'showed you an image'), img);
  if (ev.caption) fig.append(el('figcaption', null, ev.caption));
  return fig;
}

function limitText(ev) {
  const name = { five_hour: '5-hour', seven_day: 'Weekly' }[ev.window] || ev.window;
  // A warning comes when usage runs ahead of pace for the window, at any level, so it says how much is used.
  const used = typeof ev.utilization === 'number' ? `${Math.round(ev.utilization * 100)}% used, ` : '';
  const what =
    ev.status === 'rejected' ? 'limit reached' : ev.status === 'allowed' ? 'limit cleared' : `${used}ahead of pace`;
  return `${name} ${what}${ev.resets_at && ev.status !== 'allowed' ? `, resets ${resets(ev.resets_at)}` : ''}`;
}

// "16:00" today, "thu 16:00" on another day.
function resets(t) {
  const d = new Date(t * 1000);
  if (d.toDateString() === new Date().toDateString()) return clock(t);
  return `${d.toLocaleDateString('en', { weekday: 'short' }).toLowerCase()} ${clock(t)}`;
}

// A system line repeated within this many seconds counts up on the first one instead of showing again.
const REPEAT = 900;

// Events in lane "t" happened on their own (crates/core/src/events.rs): the trajectory renders them, the chat folds each run into one line.
const inTraj = (ev) => ev.lane === 't';

// Texts of the user's and seimei's messages by event id, for reply anchors and quotes.
const said = new Map();
const REPLY = 'M9 14 4 9l5-5M4 9h10.5a5.5 5.5 0 0 1 0 11H11';
const WAVE = 'M4 10v4M8 6v12M12 9v6M16 4v16M20 10v4';
const CHEVRON = 'M6 9l6 6 6-6';

// A voice call as one card: its length and summary, and a tap opens the transcript.
function callCard(t) {
  const node = el('div', 'vcall');
  const head = el('button', 'head sl');
  head.type = 'button';
  const badge = el('span', 'badge');
  badge.append(svg(WAVE, 18));
  const title = el('b', null, 'Voice call');
  const at = clock(t);
  const time = el('time', null, at);
  head.append(badge, title, time, svg(CHEVRON, 20));
  const sum = el('div', 'sum');
  const last = el('div', 'last');
  const turns = el('div', 'turns');
  sum.hidden = true;
  head.addEventListener('click', () => node.classList.toggle('open'));
  node.append(head, sum, last, turns);
  return { node, title, time, at, sum, last, turns, t0: t, t1: t, ended: false };
}

const minutes = (secs) => `${Math.max(1, Math.round(secs / 60))} min`;
const ENDED = { done: 'finished', failed: 'failed', stopped: 'was stopped', interrupted: 'was interrupted' };

// A server note's times in this device's clock: clock pairs each time with its text in the note.
function noteText(ev) {
  let text = ev.text;
  for (const [t, was] of ev.clock || []) text = text.replace(was, clock(t));
  return text;
}

function foldLabel(ev) {
  if (ev.type === 'worker') return `${ev.name} ${ENDED[ev.event] || ev.event}`;
  if (ev.type === 'system') return noteText(ev).replace(/\.$/, '');
  if (ev.type === 'rate_limit') return limitText(ev);
  // A reply of the user's turn that seimei marked trajectory:.
  if (ev.type === 'assistant' && ev.re) return 'a reply to you';
  return '';
}

// What the user can answer: seimei's messages and the report, question and approval cards.
// Each gets a reply button on hover; a long press opens its menu (Reply, Ask on the side, Copy).
// Event id -> the composer's quote and the lane it shows in.
const quotable = new Map();
const VERB = { done: 'finished' };

function replyable(node, ev, text) {
  quotable.set(ev.id, { text, lane: ev.lane === 't' ? 't' : 'c', type: ev.type });
  node.classList.add('replyable');
  node.dataset.rid = ev.id;
  const b = el('button', 'rbtn sl');
  b.type = 'button';
  b.title = 'Reply';
  b.setAttribute('aria-label', 'Reply');
  b.dataset.reply = ev.id;
  b.append(svg(REPLY, 16));
  node.append(b);
}

const firstLine = (t) => ((t || '').split('\n').find((x) => x.trim()) || '').replace(/^[#>*\-\s]+/, '').trim();

// A chip quoting the message this one answers; a tap goes to it (lane c: the chat, t: the trajectory).
function anchor(text, id, lane) {
  const b = el('button', 're sl');
  b.type = 'button';
  b.dataset.goto = id;
  b.dataset.lane = lane;
  const flat = text.replace(/\s+/g, ' ').trim();
  b.append(svg(REPLY, 14), el('span', null, flat.length > 60 ? `${flat.slice(0, 60)}…` : flat));
  return b;
}

// The card of the call that runs now, started at `since` (unix seconds): the open card when its last turn came since, or with make a new one.
export function callNow(since, make = false) {
  const v = main.vcall;
  if (v && !v.ended && v.t1 >= since) return v;
  return make ? main.call(Date.now() / 1000, true) : null;
}

// One instance appends live; a fresh one renders older pages for prepending.
// The chat and the trajectory share the open question cards (asks), so a report in one settles a card in the other.
// What the trajectory does within this long after a card settles is the card's own doing: its outcome line says it, no fold line.
const AFTER_CARD = 300;

export class Renderer {
  constructor(target, before = null, { lane = 'c', asks = new Map() } = {}) {
    this.target = target;
    this.before = before;
    this.lane = lane;
    this.day = null;
    this.activity = null;
    this.tools = new Map();
    this.notes = new Map();
    // Shikigami whose "asks" card is still open: any later event about them settles it.
    this.asks = asks;
    // Chat: the open fold line, the last message of the user's, and whether something sits after it.
    this.folded = null;
    this.lastUser = null;
    this.between = false;
    // Trajectory: the dispatcher turn being drawn.
    this.turn = null;
    this.cur = null;
    // Voice: the open call's card, and the partner's open asks to seimei by event id.
    this.vcall = null;
    this.asked = new Map();
  }

  put(node) {
    // The last thing drawn, when it is an approval card (add sets it after this)
    this.card = null;
    if (this.cur && !node.dataset.id) node.dataset.id = this.cur;
    this.target.insertBefore(node, this.before);
    // What the last drawn node says happened ("dev finished"): a trigger line naming it right after repeats it
    this.said = node.dataset.said || null;
    return node;
  }

  trig(ev) {
    if (ev.why && ev.why === this.said) return;
    this.put(el('div', 'trig', ev.why ? `after ${ev.why}` : 'on its own'));
  }

  daily(ev) {
    if (ev.t && dayKey(ev.t) !== this.day) {
      this.day = dayKey(ev.t);
      this.put(el('div', 'note day', dayLabel(ev.t)));
    }
  }

  // An event of the trajectory, seen from the chat: one line per run, naming what happened.
  // A run with nothing to name (seimei handling the user's sessions on its own, say) leaves no line: the trajectory has it.
  fold(ev) {
    if (ev.type === 'tool_result' || ev.type === 'approval_done') return;
    if (this.card?.done && (ev.t || 0) - this.card.done <= AFTER_CARD) return;
    const l = foldLabel(ev);
    if (!l && !this.folded?.node.isConnected) return;
    this.cur = ev.id;
    this.daily(ev);
    if (!this.folded?.node.isConnected) {
      const node = this.put(el('button', 'fold sl'));
      node.type = 'button';
      node.dataset.goto = ev.id;
      node.dataset.lane = 't';
      this.folded = { node, reports: [], notes: [] };
      this.activity = null;
      this.between = true;
    }
    // Shikigami reports are named first; notes only when nothing else happened.
    const f = this.folded;
    const list = ev.type === 'worker' ? f.reports : f.notes;
    if (l && !list.includes(l)) list.push(l);
    const all = [...f.reports, ...f.notes];
    const shown = all.slice(0, Math.max(1, Math.min(3, f.reports.length)));
    const what = shown.join(', ') + (all.length > shown.length ? `${shown.length > 1 ? ',' : ''} and ${all.length - shown.length} more` : '');
    f.node.replaceChildren(el('span', null, what), el('b', null, 'Trajectory'));
  }

  note(ev) {
    const seen = this.notes.get(ev.text);
    if (seen && seen.node.isConnected && (ev.t || 0) - seen.t < REPEAT) {
      seen.n++;
      seen.t = ev.t || seen.t;
      seen.node.dataset.n = `×${seen.n}`;
      return;
    }
    const node = this.put(el('div', `note ${ev.level || ''}`, noteText(ev)));
    this.notes.set(ev.text, { node, t: ev.t || 0, n: 1 });
  }

  // The call that runs now (a session's open tool call); false when it is not drawn here.
  live(info) {
    const a = info && this.tools.get(info.tool_id);
    if (this.liveAct && this.liveAct !== a) this.liveAct.unlive();
    this.liveAct = a && a.live(info.tool_id, info) ? a : null;
    return !!this.liveAct;
  }

  tick() {
    this.liveAct?.tick();
  }

  // seimei's offer of a side thread: a tonal button under the reply it is about.
  offer(ev) {
    const at = this.target.querySelector(`[data-id="${ev.re}"]`);
    if (!at || at.nextElementSibling?.dataset.ask) return;
    const b = el('button', 'btn tonal sl offerbtn');
    b.type = 'button';
    b.dataset.ask = ev.re;
    b.append(svg(THR, 20), el('span', null, 'Ask on the side'));
    at.after(b);
  }

  // A voice call is one card where it starts, with its turns inside; what the partner asked seimei stays in the chat, as the reply that answers it does.
  // Turns more than 5 minutes apart are another call; a voice_end closes the card with the call's length and summary.
  call(t, fresh = false) {
    if (fresh || !this.vcall || this.vcall.ended || t - this.vcall.t1 > 300) {
      this.vcall = callCard(t);
      this.put(this.vcall.node);
    }
    this.vcall.t1 = t;
    return this.vcall;
  }

  voice(ev) {
    if (ev.type === 'voice_end') {
      // It closes the open card, however long the call stayed silent; with no card open, its turns are on an older page.
      const c = this.vcall;
      if (!c || c.ended) return;
      c.ended = true;
      c.title.textContent = `Voice call · ${minutes(ev.secs)}`;
      c.sum.textContent = ev.summary || '';
      c.sum.hidden = !ev.summary;
      c.node.classList.remove('live');
      return;
    }
    const c = this.call(ev.t);
    this.between = false;
    if (!c.node.classList.contains('live')) c.title.textContent = `Voice call · ${minutes(c.t1 - c.t0)}`;
    if (ev.type === 'user') {
      // The partner asked seimei: the reply that follows answers it, with no chip.
      this.lastUser = ev.id;
      said.set(ev.id, ev.text);
      this.asked.set(ev.id, this.put(el('div', 'ask wait', `Asked seimei: ${ev.text}`)));
    } else if (ev.who === 'user') c.turns.append(el('div', 'msg user voiced', ev.text));
    else {
      c.turns.append(el('div', `vline${ev.cut ? ' cut' : ''}`, ev.text));
      c.last.textContent = ev.text;
    }
  }

  add(ev) {
    if (ev.type === 'approval_done') {
      if (this.card?.aid === ev.aid) this.card.done = ev.t || Date.now() / 1000;
      return settle(ev);
    }
    if (ev.type === 'offer') return this.offer(ev);
    this.cur = ev.id;
    if (ev.type !== 'tool_result') this.folded = null;
    this.daily(ev);
    // Each dispatcher turn in the trajectory opens with what started it.
    if (this.lane === 't' && ev.turn && ev.turn !== this.turn && ev.type !== 'tool_result') {
      this.turn = ev.turn;
      this.activity = null;
      this.trig(ev);
    } else if (this.lane === 't' && !ev.turn) this.turn = null;
    // The part of an event turn that came to the chat (a picture, a reply for the user) says what started it.
    if (this.lane === 'c' && ev.from && ev.from !== this.from && ev.type !== 'tool_result' && ev.type !== 'user') {
      this.from = ev.from;
      this.activity = null;
      this.trig(ev);
    } else if (this.lane === 'c' && !ev.from) this.from = null;
    // The summoned card already says it.
    if (ev.type === 'tool' && (ev.name === 'mcp__seimei__summon' || ev.name === 'AskUserQuestion')) return;
    if (ev.type === 'tool') {
      if (!this.activity) {
        this.activity = new Activity();
        this.put(this.activity.node);
      }
      this.activity.add(ev);
      this.tools.set(ev.tool_id, this.activity);
      return;
    }
    if (ev.type === 'tool_result') {
      this.tools.get(ev.tool_id)?.result(ev);
      return;
    }
    this.activity = null;
    if (ev.type === 'voice' || ev.type === 'voice_end' || (ev.type === 'user' && ev.voice)) return this.voice(ev);
    if (ev.type === 'user' || ev.type === 'assistant') {
      if (ev.text) said.set(ev.id, ev.text);
      // Sent from a side thread: the thread's name above the message, one tap from it
      if (ev.type === 'user' && ev.thread) this.put(threadLine(ev));
      // In a side thread: what seimei wrote to it, over its message
      if (ev.type === 'user' && ev.from) this.put(el('div', 'trig sender', ev.from));
      const node = this.put(message(ev));
      if (ev.type === 'user') {
        this.lastUser = ev.id;
        this.between = false;
      } else if (ev.re && (ev.re_pin || ev.re !== this.lastUser || this.between) && (said.has(ev.re) || ev.re_text)) {
        // A reply far from what it answers says which message that was.
        // re_pin: seimei named the message (re:), so the chip shows even right after it.
        node.prepend(anchor(said.get(ev.re) || ev.re_text, ev.re, 'c'));
        this.between = false;
      }
      if (ev.type === 'assistant') replyable(node, ev, ev.text || '');
      if (ev.type === 'assistant' && ev.re) this.asked.get(ev.re)?.classList.remove('wait');
      if (this.lane === 't' && ev.type === 'assistant') {
        const b = el('button', 'reply-btn sl', 'Reply in chat');
        b.type = 'button';
        b.dataset.reply = ev.id;
        node.append(b);
      }
      return;
    }
    // What the turn itself shows (lane c: a summoned card, a picture) does not come between.
    if (ev.lane !== 'c') this.between = true;
    if (ev.type === 'system') this.note(ev);
    else if (ev.type === 'worker') {
      const open = this.asks.get(ev.worker);
      if (open && ev.event !== 'summoned') {
        open.classList.replace('asking', 'asked');
        open.querySelector('.foot')?.remove();
        open.querySelector('.title').append(el('span', 'meta', ev.event === 'replied' || ev.event === 'done' ? 'Answered' : 'Closed'));
        this.asks.delete(ev.worker);
      }
      const node = workerCard(ev);
      if (!node) return;
      node.dataset.said = foldLabel(ev);
      const card = this.put(node);
      if (ev.event === 'asking') this.asks.set(ev.worker, card);
      if (VERB[ev.event]) replyable(card, ev, `${ev.name} ${VERB[ev.event]}: ${firstLine(ev.text)}`);
    }
    else if (ev.type === 'image') this.put(shown(ev));
    else if (ev.type === 'file') this.put(sentFile(ev));
    else if (ev.type === 'approval') {
      replyable(this.put(approvalCard(ev)), ev, `${ev.who} wants to ${ev.target}`);
      this.card = { aid: ev.aid, done: outcomes.get(ev.aid)?.t ?? null };
    }
    else if (ev.type === 'rate_limit') this.put(el('div', `note ${ev.status === 'rejected' ? 'error' : 'warn'}`, limitText(ev)));
  }
}

const asks = new Map();
export const main = new Renderer(thread, null, { asks });
const trail = new Renderer(traj, null, { lane: 't', asks });

// One event, drawn where it belongs: the trajectory gets it whole, the chat a fold line.
function route(ev, chat = main, t = trail) {
  if (inTraj(ev)) {
    t.add(ev);
    chat.fold(ev);
  } else chat.add(ev);
}

// ---------- live state ----------

export function render() {
  const s = !S.online ? (S.awaySince ? 'away' : 'connecting')
    : !S.session.connected ? 'starting'
    : waitingApprovals().length ? 'waiting'
    : S.session.busy ? 'working' : 'idle';
  const text = { away: '', connecting: 'Connecting…', starting: 'Session starting…', waiting: '', working: 'Working', idle: '' }[s];
  S.status = { s, text };
  document.body.classList.toggle('offline', !S.online && !!S.awaySince);
  const away = !!(S.awaySince && Date.now() - S.awaySince > 4000 && !S.online);
  if (away && $('away').hidden) requestAnimationFrame(toBottom);
  $('away').hidden = !away;
  if (S.awaySince) $('away-since').textContent = `Since ${clock(S.awaySince / 1000)}. Reconnects on its own.`;
  working.hidden = !(S.online && S.session.busy && !live) || s === 'waiting';
  const typing = input.value.trim() !== '' || attachments.length > 0;
  stopBtn.hidden = !(S.session.busy && !typing);
  // An empty composer offers a voice call in place of a send that could not send.
  talkBtn.hidden = !(S.voice && !typing && stopBtn.hidden);
  sendBtn.hidden = !stopBtn.hidden || !talkBtn.hidden;
  sendBtn.disabled = !typing;
  hooks.chrome();
}

export function clearLive() {
  live?.remove();
  live = null;
  liveText = '';
}

let frame = 0;
// Streaming text of the running turn, in the lane the turn belongs to.
export function onDelta(text, lane) {
  const stick = nearBottom();
  if (!live) {
    live = el('div', 'msg assistant live');
    (lane === 't' ? traj : thread).append(live);
    render();
  }
  liveText += text;
  if (!frame) {
    frame = requestAnimationFrame(() => {
      frame = 0;
      if (live) live.innerHTML = md(liveText);
      if (stick) toBottom();
    });
  }
}

// A logged event: draw it, keep the streaming reply last, and follow the bottom.
export function addEvent(ev) {
  // The user's own message always comes into view, even sent while scrolled up.
  const stick = nearBottom() || ev.type === 'user';
  if (ev.type === 'user' && ev.cid && pending.has(ev.cid)) {
    pending.get(ev.cid).remove();
    pending.delete(ev.cid);
  }
  if (ev.type === 'assistant') clearLive();
  route(ev);
  if (live) live.parentNode.append(live);
  hello();
  if (stick) toBottom();
  else if (/^(user|assistant|approval|worker|image)$/.test(ev.type) && inTraj(ev) === (S.lane === 't')) {
    missed++;
    jumpState();
  }
  say(spoken(ev));
}

// The line the live region reads for an event: a card that arrives, a reply, a card that settles
function spoken(ev) {
  if (ev.type === 'assistant') return 'seimei replied';
  if (ev.type === 'approval') return `${ev.who || 'seimei'} wants to ${ev.target || ev.action}`;
  if (ev.type === 'approval_done') return approvals.get(ev.aid)?.querySelector('.foot .left')?.textContent || '';
  if (ev.type === 'worker' && ev.event === 'asking') return `${ev.name} asks: ${oneSentence(ev.text || '')}`;
  if (ev.type === 'worker' && ev.event === 'done') return `${ev.name} finished`;
  if (ev.type === 'worker' && ev.event === 'failed') return `${ev.name} failed`;
  return '';
}

// An empty chat greets instead of showing a blank sheet; an empty trajectory says what will show.
function hello() {
  const empty = (t) => ![...t.children].some((c) => !c.matches('.older, .note.day, .traj-empty, [hidden]'));
  $('hello').hidden = S.lane !== 'c' || !empty(thread);
  $('traj-empty').hidden = !empty(traj);
}

// ---------- chat | trajectory ----------

S.lane = 'c';
// Where each lane was left; null: at the bottom.
const scrolls = { c: null, t: null, k: 0 };

const LANES = ['c', 't', 'k'];
// The lane being left, as a still copy over the log: the switch itself stays at once (callers append right after).
function slideOut(node, was, dir) {
  const frame = el('div', 'lane-ghost');
  frame.setAttribute('aria-hidden', 'true');
  frame.style.top = `${log.scrollTop}px`;
  frame.style.height = `${log.clientHeight}px`;
  const copy = node.cloneNode(true);
  copy.hidden = false;
  copy.removeAttribute('id');
  for (const n of copy.querySelectorAll('[id]')) n.removeAttribute('id');
  copy.style.top = `${was.top}px`;
  frame.append(copy);
  log.append(frame);
  leave(frame, `translateX(${-dir * 25}%)`).finished.finally(() => frame.remove());
  // Shared axis: the old lane is gone in the first third, the new one fades in after it, so the two never mix.
  frame.animate([{ opacity: 1 }, { opacity: 0, offset: 0.35 }, { opacity: 0 }], { duration: 200, fill: 'forwards' });
}

export function showLane(lane, goto = null) {
  if (lane !== S.lane) {
    // Lateral: the outgoing lane slides out to one side while the incoming one slides in from the other
    const dir = LANES.indexOf(lane) > LANES.indexOf(S.lane) ? 1 : -1;
    const out = { c: thread, t: traj, k: $('tasks') }[S.lane];
    const was = { top: out.offsetTop - log.scrollTop, height: out.offsetHeight };
    scrolls[S.lane] = pinned ? null : log.scrollTop;
    S.lane = lane;
    thread.hidden = lane !== 'c';
    traj.hidden = lane !== 't';
    $('tasks').hidden = lane !== 'k';
    document.body.dataset.lane = lane;
    for (const [id, l] of [['lt-chat', 'c'], ['lt-traj', 't'], ['lt-tasks', 'k']]) {
      $(id).classList.toggle('on', l === lane);
      $(id).setAttribute('aria-selected', String(l === lane));
    }
    log.setAttribute('aria-label', { c: 'Chat', t: 'Trajectory', k: 'Tasks' }[lane]);
    if (lane === 'k') renderTasks();
    if (live && live.parentNode !== (lane === 't' ? traj : thread)) live.hidden = true;
    else if (live) live.hidden = false;
    hello();
    if (scrolls[lane] === null) toBottom();
    else {
      log.scrollTop = scrolls[lane];
      pinned = nearBottom();
    }
    jumpState();
    slideOut(out, was, dir);
    const into = { c: thread, t: traj, k: $('tasks') }[lane];
    enter(into, `translateX(${dir * 25}%)`);
    into.animate([{ opacity: 0 }, { opacity: 0, offset: 0.25 }, { opacity: 1 }], { duration: 300 });
  }
  const node = goto && (lane === 't' ? traj : thread).querySelector(`[data-id="${goto}"]`);
  if (node) {
    node.scrollIntoView({ block: 'center' });
    pinned = nearBottom();
    node.classList.remove('flash');
    void node.offsetWidth;
    node.classList.add('flash');
  }
}
$('lt-chat').addEventListener('click', () => showLane('c'));
$('lt-traj').addEventListener('click', () => showLane('t'));
$('lt-tasks').addEventListener('click', () => showLane('k'));

// ---------- history ----------

let firstId = null;

export function loadHistory(r) {
  r.events.forEach((ev) => route(ev));
  firstId = r.events[0]?.id ?? null;
  older.hidden = olderT.hidden = !firstId || firstId <= 1;
  hello();
  toBottom();
}

async function loadOlder() {
  older.disabled = true;
  const r = await fetch(`api/history?before=${firstId}&limit=200`).then((x) => x.json()).catch(() => null);
  older.disabled = false;
  if (!r || !r.events.length) return;
  const height = log.scrollHeight;
  const anchor = older.nextSibling;
  const anchorT = traj.querySelector(':scope > :not(.older, .traj-empty)');
  const page = new Renderer(thread, anchor, { asks });
  const pageT = new Renderer(traj, anchorT, { lane: 't', asks });
  r.events.forEach((ev) => route(ev, page, pageT));
  // Drop the day chip that now repeats right after the inserted page.
  for (const a of [anchor, anchorT]) {
    if (a?.classList?.contains('day') && a.textContent === dayLabel(r.events.at(-1).t)) a.remove();
  }
  firstId = r.events[0].id;
  older.hidden = olderT.hidden = firstId <= 1;
  log.scrollTop += log.scrollHeight - height;
}
older.addEventListener('click', loadOlder);
olderT.addEventListener('click', loadOlder);

// ---------- composer ----------

export function autosize() {
  input.style.height = 'auto';
  input.style.height = `${Math.min(input.scrollHeight, 168)}px`;
}

// The draft survives reloads; it lives only in this browser.
const DRAFT = 'seimei.draft';
function saveDraft() {
  try {
    if (input.value) localStorage.setItem(DRAFT, input.value);
    else localStorage.removeItem(DRAFT);
  } catch {}
}
try { input.value = localStorage.getItem(DRAFT) || ''; } catch {}

$('composer').addEventListener('submit', async (e) => {
  e.preventDefault();
  const text = input.value.trim();
  const sending = attachments.splice(0);
  if ((!text && !sending.length) || !S.online) {
    attachments.push(...sending);
    if (!S.online) snack('Offline. Your message stays here.');
    return;
  }
  sentDictated(text);
  input.value = '';
  autosize();
  renderAttachments();
  saveDraft();
  const reply = replyTo;
  quoteOff();
  deliver(text, sending, reply);
});

// Reply (hover button, long press, or "Reply in chat" in the trajectory): the next message answers that message or card.
let replyTo = null;
function replyIn(id) {
  replyTo = { id, ...(quotable.get(id) || { text: said.get(id) || '', lane: 't' }) };
  $('quote-text').textContent = `Replying to: ${replyTo.text.replace(/\s+/g, ' ').trim()}`;
  $('quote').hidden = false;
  showLane('c');
  input.focus();
}
function quoteOff() {
  replyTo = null;
  $('quote').hidden = true;
}
$('quote-x').addEventListener('click', quoteOff);

// The message shows at once, dimmed, until the server echoes it back; a failed one offers a retry.
const pending = new Map();

function pendingBubble(cid, text, sending, reply) {
  const node = el('div', 'msg user pending');
  if (reply) node.append(anchor(reply.text, reply.id, reply.lane || 't'));
  const imgs = sending.filter((a) => a.kind === 'image');
  if (imgs.length) {
    const pics = el('div', `pics n${Math.min(imgs.length, 3)}${text ? '' : ' only'}`);
    imgs.forEach((a) => { const i = el('img', 'pic'); i.src = a.url; pics.append(i); });
    node.append(pics);
  }
  const docs = sending.filter((a) => a.kind === 'file');
  if (docs.length) {
    const box = el('div', 'files');
    docs.forEach((a) => box.append(el('div', 'fchip', a.name)));
    node.append(box);
  }
  if (text) node.append(document.createTextNode(text));
  node.dataset.cid = cid;
  return node;
}

async function deliver(text, sending, reply = null) {
  const cid = Math.random().toString(36).slice(2, 12);
  const node = pendingBubble(cid, text, sending, reply);
  pending.set(cid, node);
  if (S.lane !== 'c') showLane('c');
  thread.append(node);
  if (live?.parentNode === thread) thread.append(live);
  hello();
  toBottom();
  try {
    // Files upload as soon as they are picked; wait for any still on the way.
    await Promise.all(sending.map((a) => a.upload).filter(Boolean));
    if (sending.some((a) => a.kind === 'file' && !a.id)) throw new Error('upload');
    await post('api/send', { text, cid, reply: reply?.id, images: sending.filter((a) => a.kind === 'image').map((a) => ({ data: a.data })),
      files: sending.filter((a) => a.kind === 'file').map((a) => a.id) });
  } catch {
    if (!pending.has(cid)) return;
    node.classList.add('failed');
    const again = el('button', 'retry sl', 'Not sent. Tap to retry.');
    again.type = 'button';
    again.addEventListener('click', () => {
      pending.delete(cid);
      node.remove();
      // A file whose upload failed is gone; the rest go again.
      deliver(text, sending.filter((a) => a.kind !== 'file' || a.id), reply);
    });
    node.append(again);
  }
}

input.addEventListener('input', () => { autosize(); render(); saveDraft(); });
input.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && finePointer) {
    e.preventDefault();
    $('composer').requestSubmit();
  }
});
stopBtn.addEventListener('click', () => post('api/interrupt', {}).catch(() => {}));

// ---------- images ----------

// The model reads images up to 1568 px on the long side; anything bigger only costs upload time.
const MAX_SIDE = 1568;
const attachments = [];

const toBase64 = (blob) => new Promise((resolve, reject) => {
  const r = new FileReader();
  r.onload = () => resolve(String(r.result).split(',')[1]);
  r.onerror = reject;
  r.readAsDataURL(blob);
});

async function prepare(file) {
  const bmp = await createImageBitmap(file);
  const scale = Math.min(1, MAX_SIDE / Math.max(bmp.width, bmp.height));
  let blob = file;
  const lossless = /^image\/(png|gif|webp)$/.test(file.type);
  if (scale < 1 || !lossless || file.size > 3.5e6) {
    const c = document.createElement('canvas');
    c.width = Math.round(bmp.width * scale);
    c.height = Math.round(bmp.height * scale);
    const ctx = c.getContext('2d');
    ctx.fillStyle = '#fff';
    ctx.fillRect(0, 0, c.width, c.height);
    ctx.drawImage(bmp, 0, 0, c.width, c.height);
    blob = await new Promise((resolve) => c.toBlob(resolve, 'image/jpeg', 0.85));
  }
  bmp.close?.();
  return { kind: 'image', data: await toBase64(blob), url: URL.createObjectURL(blob) };
}

const MAX_FILE = 50 << 20;
const PICTURE = /^image\/(jpeg|png|webp|gif)$/;

// Any other file goes up to the server right away; the message then names it by id.
function uploadFile(f) {
  const a = { kind: 'file', name: f.name || 'file', size: f.size, id: null };
  a.upload = fetch(`api/upload?name=${encodeURIComponent(a.name)}`, {
    method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: f,
  }).then(async (r) => {
    if (r.status === 401) toLogin();
    const body = await r.json().catch(() => null);
    if (!r.ok || !body?.id) throw new Error(body?.error || 'upload failed');
    Object.assign(a, { id: body.id, kind: 'file', fkind: body.kind });
  }).catch((e) => {
    attachments.splice(attachments.indexOf(a) >>> 0, attachments.includes(a) ? 1 : 0);
    main.add({ type: 'system', level: 'error', text: `Couldn't upload ${a.name}: ${e.message}` });
  }).finally(() => { a.upload = null; renderAttachments(); });
  return a;
}

async function addFiles(list) {
  for (const f of list) {
    const pics = attachments.filter((a) => a.kind === 'image').length;
    const files = attachments.length - pics;
    try {
      if (PICTURE.test(f.type) && pics < 6) attachments.push(await prepare(f));
      else if (f.size > MAX_FILE) main.add({ type: 'system', level: 'error', text: `${f.name} is larger than 50 MB.` });
      else if (files < 10) attachments.push(uploadFile(f));
    } catch {
      main.add({ type: 'system', level: 'error', text: `Couldn't read ${f.name || 'that file'}` });
    }
  }
  renderAttachments();
}

function renderAttachments() {
  const box = $('attachments');
  box.replaceChildren();
  attachments.forEach((a, i) => {
    const item = el('div', `att${a.kind === 'file' ? ' file' : ''}${a.upload ? ' busy' : ''}`);
    if (a.kind === 'file') {
      item.append(svg(fileIcon(a.fkind), 20), el('span', 'fname', a.name));
    } else {
      const img = el('img');
      img.src = a.url;
      img.alt = '';
      item.append(img);
    }
    const x = el('button', 'remove sl');
    x.type = 'button';
    x.setAttribute('aria-label', 'Remove');
    x.append(svg('M6 6l12 12M18 6 6 18', 14));
    x.addEventListener('click', () => {
      if (a.url) URL.revokeObjectURL(a.url);
      attachments.splice(i, 1);
      renderAttachments();
    });
    item.append(x);
    box.append(item);
  });
  box.hidden = attachments.length === 0;
  render();
}

$('attach').addEventListener('click', () => $('file').click());
$('file').addEventListener('change', (e) => {
  addFiles([...e.target.files]);
  e.target.value = '';
});
input.addEventListener('paste', (e) => {
  const imgs = [...(e.clipboardData?.files || [])].filter((f) => f.type.startsWith('image/'));
  if (imgs.length) {
    e.preventDefault();
    addFiles(imgs);
  }
});
$('composer').addEventListener('dragover', (e) => e.preventDefault());
$('composer').addEventListener('drop', (e) => {
  e.preventDefault();
  addFiles([...(e.dataTransfer?.files || [])]);
});

// A tap on a message shows when it was sent; on a shikigami card opens it; on a picture zooms; on a fold line or an anchor goes to what it names.
function onTap(e) {
  if (press?.fired) return;
  const goto = e.target.closest('[data-goto]');
  if (goto) return showLane(goto.dataset.lane, +goto.dataset.goto);
  const reply = e.target.closest('.reply-btn, .rbtn');
  if (reply) return replyIn(+reply.dataset.reply);
  const side = e.target.closest('[data-thread]');
  if (side) return openThread(side.dataset.thread);
  const offer = e.target.closest('[data-ask]');
  if (offer) return askOnSide(+offer.dataset.ask);
  if (e.target.matches('img.pic')) {
    const n = e.target.dataset.name;
    return view(e.target.src, n ? { full: `api/files/${n}`, save: `api/files/${n}?download=1` } : {});
  }
  const card = e.target.closest('.wcard, .report, .evline');
  if (card?.dataset.worker && !e.target.closest('a, .more') && !getSelection().toString()) {
    // Answer opens the page with its answer field ready
    const go = openWorker(card.dataset.worker);
    if (e.target.closest('[data-answer]')) go.then(() => $('spage').querySelector('textarea')?.focus());
    return go;
  }
  const msg = e.target.closest('.msg');
  if (!msg || e.target.closest('a') || getSelection().toString()) return;
  const t = msg.querySelector(':scope > .time');
  if (t) t.remove();
  else if (msg.dataset.t) msg.append(el('span', 'time', clock(+msg.dataset.t)));
}
thread.addEventListener('click', onTap);
traj.addEventListener('click', onTap);

// Ask on the side: a new side thread about this message of seimei's.
function askOnSide(id) {
  const text = said.get(id) || quotable.get(id)?.text || '';
  openDraft({ id, text });
}

// A long press (or a right click) on something answerable: Reply, Ask on the side for seimei's own words, Copy.
const COPY = 'M10 8h8.5A1.5 1.5 0 0 1 20 9.5v9a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 8 18.5V10a2 2 0 0 1 2-2zM16 8V5.5A1.5 1.5 0 0 0 14.5 4h-9A1.5 1.5 0 0 0 4 5.5v9A1.5 1.5 0 0 0 5.5 16H8';
function msgMenu(id) {
  const q = quotable.get(id) || { text: said.get(id) || '' };
  const opt = (icon, label, run) => {
    const b = el('button', 'mopt sl');
    b.type = 'button';
    b.append(svg(icon, 22), el('span', null, label));
    b.addEventListener('click', () => { closeSheet(); run(); });
    return b;
  };
  const flat = q.text.replace(/\s+/g, ' ').trim();
  const opts = [opt(REPLY, 'Reply', () => replyIn(id))];
  if (q.type === 'assistant') opts.push(opt(THR, 'Ask on the side', () => askOnSide(id)));
  opts.push(opt(COPY, 'Copy', () => navigator.clipboard?.writeText(q.text).then(() => snack('Copied'), () => null)));
  sheet.body.replaceChildren(el('div', 'mwho', flat.length > 90 ? `${flat.slice(0, 90)}…` : flat), ...opts);
  showSheet('rowmenu');
}
const PRESS = 450;
let press = null;
function pressStart(e) {
  const node = e.target.closest('.replyable');
  if (e.pointerType !== 'touch' || !node || e.target.closest('a, button, input, textarea, pre')) return;
  const p = { x: e.clientX, y: e.clientY, fired: false };
  p.timer = setTimeout(() => {
    p.fired = true;
    navigator.vibrate?.(12);
    msgMenu(+node.dataset.rid);
  }, PRESS);
  press = p;
}
function pressMove(e) {
  if (press && !press.fired && Math.hypot(e.clientX - press.x, e.clientY - press.y) > 10) pressEnd();
}
function pressEnd() {
  if (!press) return;
  clearTimeout(press.timer);
  // A fired press swallows the click that may follow it, and no later tap.
  if (press.fired) setTimeout(() => { press = null; }, 400);
  else press = null;
}
for (const box of [thread, traj]) {
  box.addEventListener('pointerdown', pressStart);
  box.addEventListener('pointermove', pressMove);
  box.addEventListener('pointerup', pressEnd);
  box.addEventListener('pointercancel', pressEnd);
  box.addEventListener('contextmenu', (e) => {
    if (press?.fired) return e.preventDefault();
    // A right click with the mouse: the same menu, unless text is selected (the browser's own menu copies it)
    const node = e.target.closest('.replyable');
    if (!node || e.target.closest('a, pre') || getSelection().toString()) return;
    e.preventDefault();
    msgMenu(+node.dataset.rid);
  });
}
