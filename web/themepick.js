// The theme picker (#81): Menu, then Theme. Each tile is a small chat drawn in its theme's own tokens.
import { el } from './util.js';
import { sheet, showSheet } from './sheet.js';
import { THEMES, file, tokens, setVars, prefs, set, isDark, phoneSeed, seedOf, onTheme } from './theme.js';

// An element with children (strings or nodes).
function h(tag, cls, ...kids) {
  const e = el(tag, cls);
  e.append(...kids);
  return e;
}

const MODES = [['system', 'System'], ['light', 'Light'], ['dark', 'Dark']];

function preview() {
  const card = h('div', 'tcard', h('div', 'tct', 'Run ', el('code', null, 'deploy.sh'), '?'),
    h('div', 'tbt', el('span', 'tdeny', 'Deny'), el('span', 'tallow', 'Allow')));
  return h('div', 'tprev', el('div', 'tub', 'ship it'), h('div', 'tsay', 'Deployed ', el('b', null, '0ea1eca'), '. Tests pass.'), card);
}

async function tile(name, p) {
  const f = await file(name);
  const d = isDark(p);
  const prev = preview();
  setVars(prev, await tokens(name, d, p));
  const sub = el('span', null);
  const b = h('button', 'ttile' + (p.theme === name ? ' sel' : ''), prev, h('div', 'tlbl', el('b', null, f.name), sub));
  b.type = 'button';
  b.setAttribute('aria-pressed', String(p.theme === name));
  b.addEventListener('click', () => pick({ theme: name }));
  if (!f.dynamic) {
    sub.textContent = f[d ? 'dark' : 'light'].label;
    return b;
  }
  const ph = phoneSeed();
  if (ph) {
    sub.textContent = `From ${ph.from || 'the phone'}'s wallpaper`;
    return b;
  }
  // No phone has sent its wallpaper's color yet: pick a seed here.
  sub.textContent = 'Seed color';
  const input = el('input', 'tseed');
  input.type = 'color';
  input.value = seedOf(p);
  input.setAttribute('aria-label', 'Material You seed color');
  input.addEventListener('click', (e) => e.stopPropagation());
  input.addEventListener('change', () => pick({ theme: name, seed: input.value }));
  sub.append(input);
  return b;
}

async function accents(p) {
  const f = await file('catppuccin');
  const side = isDark(p) ? 'dark' : 'light';
  const on = p.accent || f.accent;
  const dots = el('div', 'tdots');
  for (const [n, t] of Object.entries(f.accents[side])) {
    const b = el('button', 'tdot' + (n === on ? ' on' : ''));
    b.type = 'button';
    b.title = n;
    b.setAttribute('aria-label', `Accent ${n}`);
    b.style.setProperty('background', t.primary);
    b.addEventListener('click', () => pick({ accent: n }));
    dots.append(b);
  }
  return h('div', 'trow', el('span', 'trl', 'Accent'), dots);
}

async function render() {
  const p = prefs();
  const seg = el('div', 'tseg');
  for (const [m, label] of MODES) {
    const b = el('button', m === p.mode ? 'on' : '', label);
    b.type = 'button';
    b.addEventListener('click', () => pick({ mode: m }));
    seg.append(b);
  }
  const grid = h('div', 'tgrid', ...(await Promise.all(THEMES.map((n) => tile(n, p)))));
  const body = [el('h2', 'list', 'Theme'), h('div', 'trow', seg), grid];
  if (p.theme === 'catppuccin') body.push(await accents(p));
  if (sheet.view === 'theme') sheet.body.replaceChildren(...body);
}

async function pick(change) {
  await set(change);
  render();
}

export async function openThemes() {
  sheet.body.replaceChildren(el('h2', 'list', 'Theme'));
  showSheet('theme');
  await render();
}

// The phone's seed or the system's light/dark changed while the picker is open.
onTheme(() => { if (sheet.view === 'theme') render(); });
