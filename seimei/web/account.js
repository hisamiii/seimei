// The account menu: fingerprint login, the activity log, and logging out.
import { deviceName, register, supported } from './passkey.js';
import { $, el, clock, dayLabel, post, getJSON, toLogin } from './util.js';
import { main, toBottom } from './chat.js';
import { sheet, showSheet } from './sheet.js';

const menu = $('menu'), card = $('fp-card');

export async function account() {
  const r = await fetch('api/auth/me').then((x) => x.json()).catch(() => null);
  $('menu-btn').hidden = false;
  if (!r?.auth) {
    $('add-passkey').hidden = $('logout').hidden = true;
    return;
  }
  $('add-passkey').hidden = !supported();
  let dismissed = false;
  try { dismissed = localStorage.getItem('seimei.fp-card') === 'no'; } catch {}
  if (supported() && r.passkeys === 0 && !dismissed) {
    card.hidden = false;
    requestAnimationFrame(toBottom);
  }
}

async function addPasskey() {
  menu.hidden = true;
  const r = await register(deviceName()).catch(() => ({ ok: false }));
  if (r.status === 401) return toLogin();
  if (r.ok) card.hidden = true;
  const text = r.ok ? 'Fingerprint login is set up on this device.'
    : r.exists ? 'This device already has a passkey for seimei.'
    : r.cancelled ? null : 'Could not set up the fingerprint login.';
  if (text) {
    main.add({ type: 'system', level: r.ok || r.exists ? '' : 'error', text });
    toBottom();
  }
}

$('menu-btn').addEventListener('click', (e) => {
  e.stopPropagation();
  menu.hidden = !menu.hidden;
});
document.addEventListener('click', (e) => { if (!menu.contains(e.target)) menu.hidden = true; });
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') menu.hidden = true; });
$('add-passkey').addEventListener('click', addPasskey);
$('fp-yes').addEventListener('click', addPasskey);
$('fp-no').addEventListener('click', () => {
  card.hidden = true;
  try { localStorage.setItem('seimei.fp-card', 'no'); } catch {}
});
$('logout').addEventListener('click', () => post('api/auth/logout', {}).finally(toLogin));
$('audit').addEventListener('click', () => {
  menu.hidden = true;
  openAudit();
});

// ---------- activity log ----------

const AUDIT_TONE = { blocked: 'bad', denied: 'bad', expired: 'bad', approved: 'ok' };

function auditWhat(call) {
  let inp = {};
  try { inp = JSON.parse(call.input); } catch { return call.input || ''; }
  for (const k of ['command', 'file_path', 'path', 'pattern', 'url', 'query', 'description', 'prompt', 'to', 'task', 'worker', 'session']) {
    if (typeof inp[k] === 'string' && inp[k].trim()) return inp[k].trim().split('\n')[0];
  }
  return call.input === '{}' ? '' : call.input;
}

async function openAudit() {
  const r = await getJSON('api/audit?limit=300');
  if (!r) return;
  const list = el('div', 'alist');
  let day = null;
  for (const c of r.calls) {
    const d = new Date(c.t * 1000).toDateString();
    if (d !== day) {
      day = d;
      list.append(el('div', 'aday', dayLabel(c.t)));
    }
    const row = el('div', `arow ${AUDIT_TONE[c.decision] || ''}`);
    const top = el('div', 'atop');
    top.append(el('time', null, clock(c.t)), el('b', null, c.who), el('span', 'tool', c.tool.replace(/^mcp__seimei__/, '')));
    if (c.decision !== 'allowed') top.append(el('span', 'tag', c.why ? `${c.decision}: ${c.why}` : c.decision));
    if (c.failed) top.append(el('span', 'tag', 'failed'));
    row.append(top, el('code', null, auditWhat(c)));
    list.append(row);
  }
  if (!r.calls.length) list.append(el('p', 'usage-note', 'Nothing yet.'));
  sheet.body.replaceChildren(el('h2', 'list', 'Activity log'),
    el('p', 'usage-note', 'Every tool call by seimei and the shikigami, newest first.'), list);
  showSheet('audit');
}
