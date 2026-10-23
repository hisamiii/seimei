// Puts the last theme's tokens back before the first paint (theme.js saves them), so a light theme never flashes dark.
try {
  const t = JSON.parse(localStorage.getItem('seimei.themeVars') || 'null');
  if (t) {
    const s = document.documentElement.style;
    for (const k in t.vars) s.setProperty('--' + k, t.vars[k]);
    s.colorScheme = t.dark ? 'dark' : 'light';
  }
} catch {}
