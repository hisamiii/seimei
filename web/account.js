// The account menu: fingerprint login, the phone app and its pairing, this browser's notifications, the theme, the activity log, and logging out.
import { deviceName, register, supported } from './passkey.js';
import { $, el, clock, dayLabel, when, post, getJSON, toLogin, snack, ask } from './util.js';
import { main, toBottom } from './chat.js';
import { sheet, showSheet } from './sheet.js';
import { openThemes } from './themepick.js';

const menu = $('menu'), card = $('fp-card');

export async function account() {
  const r = await fetch('api/auth/me').then((x) => x.json()).catch(() => null);
  $('menu-btn').hidden = false;
  if (!r?.auth) {
    $('add-passkey').hidden = $('logout').hidden = true;
    return;
  }
  $('add-passkey').hidden = !supported();
  $('phones').hidden = false;
  $('notify').hidden = !canPush();
  let dismissed = false;
  try { dismissed = localStorage.getItem('seimei.fp-card') === 'no'; } catch {}
  if (supported() && r.passkeys === 0 && !dismissed) {
    card.hidden = false;
    requestAnimationFrame(toBottom);
  }
}

// Adding a passkey or a phone needs a login from the last 10 minutes.
const FRESH = 'Log out and in again first';

async function addPasskey() {
  menu.hidden = true;
  const r = await register(deviceName()).catch(() => ({ ok: false }));
  if (r.status === 401) return toLogin();
  if (r.ok) card.hidden = true;
  const text = r.ok ? 'Fingerprint login is on'
    : r.status === 403 ? FRESH
    : r.exists ? 'Fingerprint login is already on for this device'
    : r.cancelled ? null : "Couldn't set up the fingerprint login";
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
$('phones').addEventListener('click', () => {
  menu.hidden = true;
  openPhones();
});
$('notify').addEventListener('click', () => {
  menu.hidden = true;
  openNotifications();
});
$('theme').addEventListener('click', () => {
  menu.hidden = true;
  openThemes();
});
$('audit').addEventListener('click', () => {
  menu.hidden = true;
  openAudit();
});

// ---------- this browser's notifications (Web Push; the phone app has its own) ----------

const canPush = () => 'serviceWorker' in navigator && 'PushManager' in window && 'Notification' in window;

function b64url(s) {
  const raw = atob(s.replace(/-/g, '+').replace(/_/g, '/'));
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}

async function openNotifications() {
  const info = await getJSON('api/push/web');
  if (!info) return;
  const body = [el('h2', 'list', 'Notifications')];
  const block = el('div', 'pblock');
  block.append(el('p', 'usage-note', info.on
    ? 'This browser gets a notification when a card waits or seimei needs you, while this page is closed or hidden'
    : 'Get a notification in this browser when a card waits or seimei needs you, while this page is closed or hidden'));
  const ios = /iPhone|iPad/.test(navigator.userAgent) && !navigator.standalone;
  if (Notification.permission === 'denied') {
    block.append(el('p', 'usage-note', "Notifications are blocked for this site in the browser's settings"));
  } else if (ios && !info.on) {
    block.append(el('p', 'usage-note', 'On iPhone, add this page to the Home Screen first (Share, Add to Home Screen), then turn them on there'));
  } else {
    const b = el('button', info.on ? 'btn text' : 'btn filled', info.on ? 'Turn off' : 'Turn on');
    b.type = 'button';
    b.addEventListener('click', async () => {
      b.disabled = true;
      const done = await (info.on ? pushOff() : pushOn(info.vapid)).catch(() => false);
      b.disabled = false;
      if (done === false) return snack(info.on ? "Couldn't turn them off" : "Couldn't turn them on");
      if (!info.on) snack('A test notification is on its way');
      openNotifications();
    });
    block.append(b);
  }
  body.push(block);
  sheet.body.replaceChildren(...body);
  showSheet('notify');
}

async function pushOn(vapid) {
  if (await Notification.requestPermission() !== 'granted') return false;
  const reg = await navigator.serviceWorker.register('sw.js', { scope: './' });
  await navigator.serviceWorker.ready;
  const sub = await reg.pushManager.getSubscription()
    || await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: b64url(vapid) });
  await post('api/push/web', sub.toJSON());
  return true;
}

async function pushOff() {
  const reg = await navigator.serviceWorker.getRegistration();
  await (await reg?.pushManager.getSubscription())?.unsubscribe();
  await post('api/push/web', { off: true });
  return true;
}

// ---------- activity log ----------

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
    const row = el('div', 'arow');
    const top = el('div', 'atop');
    top.append(el('time', null, clock(c.t)), el('b', null, c.who), el('span', 'tool', c.tool.replace(/^mcp__.+?__/, '')));
    if (c.failed) top.append(el('span', 'tag', 'Failed'));
    row.append(top, el('code', null, auditWhat(c)));
    list.append(row);
  }
  if (!r.calls.length) list.append(el('p', 'usage-note', 'Nothing yet'));
  sheet.body.replaceChildren(el('h2', 'list', 'Activity log'), list);
  showSheet('audit');
}

// ---------- phone app ----------

let pairTimer = 0;

function mb(bytes) {
  return `${(bytes / 1048576).toFixed(1)} MB`;
}

async function openPhones() {
  const [app, devs] = await Promise.all([getJSON('api/app'), getJSON('api/devices')]);
  if (!app || !devs) return;
  clearInterval(pairTimer);
  const body = [el('h2', 'list', 'Phone app')];

  const get = el('div', 'pblock');
  if (app.available) {
    const a = el('a', 'btn tonal', 'Download the app');
    a.href = 'api/app/seimei.apk';
    a.setAttribute('download', 'seimei.apk');
    get.append(el('p', 'usage-note', `Android ${app.version} · ${mb(app.size)}`), a);
  } else {
    get.append(el('p', 'usage-note', 'Not built yet'));
  }
  body.push(get);

  const pair = el('div', 'pblock');
  const pairBtn = el('button', 'btn filled', 'Pair a phone');
  pairBtn.type = 'button';
  const codeBox = el('div', 'pcode');
  codeBox.hidden = true;
  pairBtn.addEventListener('click', async () => {
    const r = await post('api/devices/pair', {}).catch((e) => {
      if (e.message === '403') snack(FRESH);
      return null;
    });
    if (!r) return;
    pairBtn.hidden = true;
    const left = el('p', 'usage-note');
    // The address to type in the app's first step: this page's own, without https:// (the app adds it).
    const addr = el('div', 'paddr');
    const copy = el('button', 'btn text', 'Copy');
    copy.type = 'button';
    copy.addEventListener('click', () => navigator.clipboard?.writeText(location.host).then(() => snack('Copied'), () => null));
    addr.append(el('code', null, location.host), copy);
    codeBox.replaceChildren(el('p', 'usage-note', 'In the app, connect to'), addr,
      el('p', 'usage-note', 'then type this code:'), el('b', null, r.code), left);
    codeBox.hidden = false;
    const tick = () => {
      const s = Math.round(r.expires - Date.now() / 1000);
      if (s <= 0 || sheet.wrap.hidden) {
        clearInterval(pairTimer);
        codeBox.hidden = true;
        pairBtn.hidden = false;
        return;
      }
      left.textContent = `Works once, for ${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
    };
    tick();
    pairTimer = setInterval(tick, 1000);
  });
  pair.append(pairBtn, codeBox);
  body.push(pair);

  const list = el('div', 'alist');
  for (const d of devs.devices) {
    const row = el('div', 'prow');
    const text = el('div', 'ptext');
    text.append(el('b', null, d.name),
      el('span', null, d.this ? `This phone · paired ${when(d.created)}` : d.used ? `Opened ${when(d.used)} · paired ${when(d.created)}` : `Paired ${when(d.created)}`));
    const un = el('button', 'btn text', 'Unpair');
    un.type = 'button';
    un.addEventListener('click', async () => {
      if (!await ask({ headline: `Unpair ${d.name}?`, body: 'It needs a new pairing code to come back', confirm: 'Unpair' })) return;
      await post(`api/devices/${encodeURIComponent(d.id)}/revoke`, {}).catch(() => null);
      openPhones();
    });
    row.append(text, un);
    list.append(row);
  }
  body.push(el('h3', 'psec', 'Paired phones'),
    devs.devices.length ? list : el('p', 'usage-note', 'None yet'));
  sheet.body.replaceChildren(...body);
  showSheet('phones');
}
