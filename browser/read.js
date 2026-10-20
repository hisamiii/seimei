// Injected only when seimei asks to read this tab; returns the visible text.
// Form values never come along: innerText leaves input values out, and a selection inside a password field is skipped.
(() => {
  const MAX = 1000000;
  const root = document.body || document.documentElement;
  const text = root ? root.innerText || root.textContent || '' : '';
  const active = document.activeElement;
  const secret = active && active.tagName === 'INPUT' && String(active.type).toLowerCase() === 'password';
  const selection = secret ? '' : String(window.getSelection() || '');
  return {
    title: document.title,
    url: location.href,
    text: text.slice(0, MAX),
    truncated: text.length > MAX,
    selection: selection.slice(0, 20000),
  };
})();
