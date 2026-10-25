// Themes: each is a token file in themes/ (contrib/themes.py builds them); the page sets its tokens as CSS
// variables on <html>. Light or dark follows the system unless pinned. Material You builds an M3 scheme from a seed:
// the one the phone app sends from its wallpaper (api/theme), else one picked here. theme-boot.js puts the last tokens
// back before the first paint.

const KEY = 'seimei.theme', VARS = 'seimei.themeVars', SEED = 'seimei.themeSeed';
export const THEMES = ['catppuccin', 'rose-pine', 'gruvbox', 'nord', 'material-you'];
const files = {};
const dark = matchMedia('(prefers-color-scheme: dark)');
let serverSeed = load(SEED);
const listeners = new Set();

function load(k) {
  try { return JSON.parse(localStorage.getItem(k) || 'null'); } catch { return null; }
}
function save(k, v) {
  try { localStorage.setItem(k, JSON.stringify(v)); } catch {}
}

export function prefs() {
  return { theme: 'catppuccin', mode: 'system', accent: null, seed: null, ...(load(KEY) || {}) };
}

export function isDark(p = prefs()) {
  return p.mode === 'dark' || (p.mode === 'system' && dark.matches);
}

export async function file(name) {
  if (!files[name]) files[name] = fetch(new URL(`./themes/${name}.json`, import.meta.url)).then((r) => r.json());
  return files[name];
}

// The seed Material You uses here: the phone's, else the one picked on this page, else the M3 baseline.
export function seedOf(p = prefs()) {
  return serverSeed?.seed || p.seed || '#6750a4';
}
export function phoneSeed() {
  return serverSeed;
}

function rgb(h) {
  return [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
}
// t of a, the rest b: the same rule contrib/themes.py and Theme.kt use.
export function mix(a, b, t) {
  const x = rgb(a), y = rgb(b);
  return '#' + x.map((v, i) => Math.round(v * t + y[i] * (1 - t)).toString(16).padStart(2, '0')).join('');
}

function extras(t, isDark) {
  const s = t.surface;
  const on = (c) => isDark ? mix(c, '#ffffff', .46) : mix(c, '#000000', .5);
  const box = (c) => isDark ? mix(c, s, .18) : mix(c, s, .14);
  return {
    'warn-container': box(t.warn), 'on-warn-container': on(t.warn), 'ok-container': box(t.ok), 'on-ok-container': on(t.ok),
    'error-container': box(t.error), 'on-error-container': on(t.error), 'on-warn': isDark ? mix(t.warn, '#000000', .22) : '#ffffff',
  };
}

// An M3 scheme as seimei's tokens.
async function material(f, isDark, seed) {
  const { Hct, SchemeTonalSpot, argbFromHex, hexFromArgb } = await import('./mcu.js');
  const m = new SchemeTonalSpot(Hct.fromInt(argbFromHex(seed)), isDark, 0);
  const c = (k) => hexFromArgb(m[k]);
  const side = f[isDark ? 'dark' : 'light'];
  const t = {
    surface: c('surface'), 'surface-low': mix(c('surface'), c('surfaceContainerLowest'), .5), 'surface-lowest': c('surfaceContainerLowest'),
    container: c('surfaceContainer'), 'container-high': c('surfaceContainerHigh'), 'container-highest': c('surfaceContainerHighest'),
    primary: c('primary'), 'on-primary': c('onPrimary'), 'primary-container': c('primaryContainer'),
    'on-primary-container': c('onPrimaryContainer'), 'inverse-primary': c('inversePrimary'),
    tonal: c('secondaryContainer'), 'on-tonal': c('onSecondaryContainer'),
    ok: side.ok, warn: side.warn, error: c('error'),
    text: c('onSurface'), 'text-2': c('onSurfaceVariant'), 'text-3': mix(c('onSurfaceVariant'), c('surface'), .78), outline: c('outline'),
  };
  t.tint = mix(t.primary, t.surface, .07);
  Object.assign(t, extras(t, isDark));
  t['error-container'] = c('errorContainer');
  t['on-error-container'] = c('onErrorContainer');
  t.tints = side.tints;
  return t;
}

// The tokens of [name] on one side, with its accent (Catppuccin) or seed (Material You).
export async function tokens(name, isDark, p = prefs()) {
  const f = await file(name);
  if (f.dynamic) return material(f, isDark, seedOf(p));
  const side = isDark ? 'dark' : 'light';
  const t = { ...f[side] };
  const acc = f.accents && (p.accent || f.accent);
  if (acc && f.accents[side][acc]) Object.assign(t, f.accents[side][acc]);
  return t;
}

export function setVars(node, t) {
  for (const [k, v] of Object.entries(t)) if (typeof v === 'string' && k !== 'label') node.style.setProperty(`--${k}`, v);
}

export let current = null;

export async function apply(p = prefs()) {
  const d = isDark(p);
  const t = await tokens(THEMES.includes(p.theme) ? p.theme : 'catppuccin', d, p);
  const root = document.documentElement;
  setVars(root, t);
  root.style.colorScheme = d ? 'dark' : 'light';
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', t.surface);
  current = t;
  save(VARS, { dark: d, vars: Object.fromEntries(Object.entries(t).filter(([k, v]) => typeof v === 'string' && k !== 'label')) });
  for (const f of listeners) f(t);
  return t;
}

export function onTheme(f) {
  listeners.add(f);
}

export function set(change) {
  const p = { ...prefs(), ...change };
  save(KEY, p);
  return apply(p);
}

// The phone's seed came in (api/theme or the event stream): Material You follows it.
export function phone(rec) {
  if (!rec?.seed || rec.seed === serverSeed?.seed) return;
  serverSeed = rec;
  save(SEED, rec);
  if (prefs().theme === 'material-you') apply();
}

// [seed]: also ask the server for the phone's seed (logged in only; the login page uses the kept one).
export async function init(seed = true) {
  dark.addEventListener('change', () => { if (prefs().mode === 'system') apply(); });
  await apply();
  if (seed) fetch('api/theme').then((r) => r.ok ? r.json() : null).then(phone).catch(() => {});
}
