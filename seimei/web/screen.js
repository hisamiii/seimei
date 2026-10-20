// The Screen tab: the laptop screen or one window, about once a second, and the windows by workspace.
import { $, S, el, svg, getJSON, toLogin, view, ICON } from './util.js';

const page = $('screen');
const APP_TONES = ['#cba6f7', '#89b4fa', '#a6e3a1', '#f9e2af', '#fab387', '#f5c2e7', '#94e2d5', '#b4befe'];
const screen = { mode: 'screen', window: null, paused: false, timer: null, url: null, state: null, gen: 0, listTimer: 0 };

function appTone(app) {
  let h = 0;
  for (const ch of app || '?') h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return APP_TONES[h % APP_TONES.length];
}

function segButton(label, on, onClick) {
  const b = el('button', `seg-btn sl${on ? ' on' : ''}`);
  b.type = 'button';
  if (on) b.append(svg(ICON.check, 16));
  b.append(label);
  b.addEventListener('click', onClick);
  return b;
}

function screenTarget() {
  if (screen.mode === 'screen') return null;
  const wins = (screen.state?.workspaces || []).flatMap((w) => w.windows);
  return wins.find((w) => w.id === screen.window) || wins.find((w) => w.focused) || null;
}

// "Laptop screen" and "Portrait, 1600 × 2560" instead of an output name and a mode.
function outputLabel(o) {
  const name = /^eDP/.test(o.name) ? 'Laptop screen' : `Screen ${o.name}`;
  if (!o.width) return [name, ''];
  const turned = /90|270/.test(o.transform || '');
  const [w, h] = turned ? [o.height, o.width] : [o.width, o.height];
  return [name, `${h > w ? 'Portrait' : 'Landscape'}, ${w} × ${h}`];
}

function renderScreen() {
  const st = screen.state;
  const frame = el('div', 'screen-frame');
  const img = el('img');
  img.alt = 'The laptop screen';
  img.id = 'screen-img';
  if (screen.url) img.src = screen.url;
  img.addEventListener('click', () => screen.url && view(screen.url));
  const live = el('span', `live-chip ${screen.paused ? 'off' : ''}`);
  live.append(el('i'), screen.paused ? 'Paused' : 'Live');
  const msg = el('div', 'screen-msg', '');
  msg.id = 'screen-msg';
  frame.append(img, live, msg);

  const ctl = el('div', 'screen-ctl');
  const target = screenTarget();
  const out = st?.outputs?.[0];
  const [title, sub] = screen.mode === 'screen' ? (out ? outputLabel(out) : ['', '']) : [target ? target.title : 'No window', target?.app || ''];
  const label = el('span', 'screen-label');
  label.append(el('b', null, title), el('small', null, sub));
  const pause = el('button', 'btn tonal sl', screen.paused ? 'Resume' : 'Pause');
  pause.type = 'button';
  pause.addEventListener('click', () => {
    screen.paused = !screen.paused;
    renderScreen();
    if (!screen.paused) pollScreen();
  });
  const full = el('button', 'icon-btn tonal sl');
  full.type = 'button';
  full.setAttribute('aria-label', 'Full screen');
  full.append(svg(ICON.full, 18));
  full.addEventListener('click', () => screen.url && view(screen.url));
  ctl.append(label, pause, full);

  const seg = el('div', 'seg');
  seg.append(segButton('Whole screen', screen.mode === 'screen', () => setScreenMode('screen', null)),
    segButton('Window', screen.mode === 'window', () => setScreenMode('window', screen.window)));
  const parts = [frame, ctl, seg];
  if (st && st.up === false) parts.push(el('p', 'usage-note', 'The desktop is not running.'));
  for (const ws of st?.workspaces || []) {
    if (!ws.windows.length) continue;
    const group = el('div', 'wsgroup');
    const wl = el('div', 'wslabel');
    wl.append(el('span', null, ws.name ? `Workspace ${ws.idx} · ${ws.name}` : `Workspace ${ws.idx}`));
    if (ws.active) wl.append(el('span', 'wsbadge', 'active'));
    const list = el('div', 'wslist');
    for (const w of ws.windows) {
      const row = el('button', `wsrow sl${screen.mode === 'window' && target?.id === w.id ? ' on' : ''}`);
      row.type = 'button';
      const letter = el('span', 'wsletter', (w.app || '?')[0].toUpperCase());
      letter.style.setProperty('--tone', appTone(w.app));
      const txt = el('span', 'wstext');
      txt.append(el('span', 'wstitle', w.title || w.app), el('span', 'wsapp',
        [w.app, w.focused && 'focused', w.urgent && 'needs attention', w.ours && 'opened by seimei'].filter(Boolean).join(' · ')));
      row.append(letter, txt, svg('M9 5l7 7-7 7', 18));
      row.addEventListener('click', () => setScreenMode('window', w.id));
      list.append(row);
    }
    group.append(wl, list);
    parts.push(group);
  }
  const top = page.scrollTop;
  page.replaceChildren(...parts);
  page.scrollTop = top;
  showScreenMsg();
}

let screenMsg = '';
function showScreenMsg(text) {
  if (text !== undefined) screenMsg = text;
  const m = document.getElementById('screen-msg');
  if (m) {
    m.textContent = screenMsg;
    m.hidden = !screenMsg;
  }
}

function setScreenMode(mode, id) {
  screen.mode = mode;
  screen.window = id;
  screen.gen++;
  if (screen.url) URL.revokeObjectURL(screen.url);
  screen.url = null;
  renderScreen();
  page.scrollTo({ top: 0, behavior: 'smooth' });
  pollScreen();
}

async function refreshDesktop() {
  const st = await getJSON('api/desktop');
  if (!st || S.tab !== 'screen') return;
  screen.state = st;
  renderScreen();
}

async function pollScreen() {
  clearTimeout(screen.timer);
  if (S.tab !== 'screen' || screen.paused || document.hidden) return;
  const gen = screen.gen;
  const target = screenTarget();
  let wait = screen.mode === 'screen' ? 1000 : 4000;
  if (screen.mode === 'window' && !target) {
    showScreenMsg('Pick a window below.');
  } else {
    const q = screen.mode === 'screen' ? 'mode=screen' : `mode=window&id=${target.id}`;
    const r = await fetch(`api/screen?${q}`).catch(() => null);
    if (gen !== screen.gen || S.tab !== 'screen') return;
    if (r?.ok) {
      const url = URL.createObjectURL(await r.blob());
      const img = document.getElementById('screen-img');
      const old = screen.url;
      screen.url = url;
      if (img) img.src = url;
      if (old) setTimeout(() => URL.revokeObjectURL(old), 1000);
      showScreenMsg(screen.state?.locked ? 'Screen locked' : '');
    } else {
      const body = await r?.json().catch(() => null);
      if (r?.status === 401) return toLogin();
      showScreenMsg(body?.paused === 'bench' ? 'Paused while a benchmark runs' : body?.down ? 'The desktop is not running'
        : body?.error || 'Could not reach the laptop');
      wait = 5000;
    }
  }
  screen.timer = setTimeout(pollScreen, wait);
}

export async function showScreen() {
  screen.gen++;
  renderScreen();
  await refreshDesktop();
  pollScreen();
  clearInterval(screen.listTimer);
  screen.listTimer = setInterval(() => {
    if (S.tab !== 'screen') return clearInterval(screen.listTimer);
    if (!screen.paused && !document.hidden) refreshDesktop();
  }, 5000);
}

export function hideScreen() {
  clearTimeout(screen.timer);
  clearInterval(screen.listTimer);
}

document.addEventListener('visibilitychange', () => { if (!document.hidden && S.tab === 'screen') pollScreen(); });
