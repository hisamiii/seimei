// Injected into a page (each frame on its own) when seimei reads, acts or waits there.
// It runs in the content-script scope: the page cannot see the element map below, and setting a field's value goes through the browser's own setter.
// Events it sends have isTrusted false. Secret fields (passwords, card numbers, one-time codes, files) are never read or filled.
if (!globalThis.seimeiPage) {
  const MAX_TEXT = 1000000;
  const MAX_ELEMENTS = 300;
  const PICK = [
    'a[href]', 'button', 'input:not([type=hidden])', 'select', 'textarea', 'summary', '[contenteditable=""]',
    '[contenteditable=true]', '[onclick]', '[tabindex]:not([tabindex="-1"])',
    ...['button', 'link', 'checkbox', 'radio', 'tab', 'menuitem', 'option', 'switch', 'combobox', 'textbox',
      'searchbox', 'slider'].map((r) => `[role=${r}]`),
  ].join(',');
  const SECRET = /\b(cc-[a-z-]+|one-time-code|current-password|new-password)\b/;

  // What kind of secret a field holds ('password', 'card number', ...), or '' for an ordinary field.
  const secret = (el) => {
    if (el.tagName !== 'INPUT' && el.tagName !== 'TEXTAREA') return '';
    const type = String(el.type).toLowerCase();
    if (type === 'password' || type === 'file') return type;
    const m = SECRET.exec(String(el.getAttribute('autocomplete') || ''));
    if (!m) return '';
    if (m[1].startsWith('cc-')) return 'payment card';
    return m[1] === 'one-time-code' ? 'one-time code' : 'password';
  };

  const visible = (el) => {
    if (!el.getClientRects().length) return false;
    const s = getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none';
  };

  const clean = (s, n = 80) => {
    const t = String(s || '').replace(/\s+/g, ' ').trim();
    return t.length > n ? `${t.slice(0, n - 1)}...` : t;
  };

  // The name a person or a screen reader would use for the element.
  const nameOf = (el) => {
    const by = el.getAttribute('aria-labelledby');
    if (by) {
      const t = by.split(/\s+/).map((id) => document.getElementById(id)?.textContent || '').join(' ');
      if (clean(t)) return clean(t);
    }
    const direct = el.getAttribute('aria-label') || (el.labels && el.labels[0] && el.labels[0].textContent);
    if (clean(direct)) return clean(direct);
    if (el.tagName === 'INPUT' && ['button', 'submit', 'reset'].includes(el.type)) return clean(el.value);
    if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT') {
      return clean(el.getAttribute('placeholder') || el.getAttribute('title') || el.name || '');
    }
    const img = el.querySelector && el.querySelector('img[alt]');
    return clean(el.innerText || el.getAttribute('title') || (img && img.alt) || '');
  };

  const kindOf = (el) => {
    const role = el.getAttribute('role');
    if (role) return role;
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return 'link';
    if (tag === 'input') {
      const t = String(el.type).toLowerCase();
      if (['button', 'submit', 'reset', 'image'].includes(t)) return 'button';
      if (t === 'checkbox' || t === 'radio' || t === 'password' || t === 'file') return t;
      return 'textbox';
    }
    if (tag === 'textarea') return 'textbox';
    if (tag === 'select') return 'select';
    if (tag === 'summary' || tag === 'button') return 'button';
    if (el.isContentEditable) return 'editor';
    return 'clickable';
  };

  // One line for the element: kind, name, then its target or state.
  const describe = (el) => {
    const kind = kindOf(el);
    let out = `${kind} "${nameOf(el)}"`;
    if (el.tagName === 'A' && el.href) out += ` -> ${el.href}`;
    if (secret(el)) return `${out} [hidden]`;
    if (el.tagName === 'SELECT') {
      const opts = [...el.options].slice(0, 12).map((o) => clean(o.label || o.text, 40));
      out += ` value="${clean(el.selectedOptions[0] ? el.selectedOptions[0].label : '', 60)}" options: ${opts.join(', ')}`;
      if (el.options.length > 12) out += `, ... (${el.options.length})`;
    } else if (el.type === 'checkbox' || el.type === 'radio') {
      if (el.checked) out += ' checked';
    } else if ((el.tagName === 'INPUT' && kind === 'textbox') || el.tagName === 'TEXTAREA') {
      if (el.value) out += ` value="${clean(el.value, 120)}"`;
    }
    if (el.disabled) out += ' disabled';
    return out;
  };

  // Every element to act on, in document order, shadow roots (open or closed) included.
  const collect = (root, out) => {
    for (const el of root.querySelectorAll('*')) {
      if (out.length >= MAX_ELEMENTS) return out;
      if (el.matches(PICK) && visible(el)) out.push(el);
      // Firefox gives closed roots through the element; Chromium through chrome.dom.
      const shadow = 'openOrClosedShadowRoot' in el ? el.openOrClosedShadowRoot : globalThis.chrome?.dom?.openOrClosedShadowRoot(el);
      if (shadow) collect(shadow, out);
    }
    return out;
  };

  const page = {
    refs: [],

    read(args) {
      const root = document.body || document.documentElement;
      const text = root ? root.innerText || root.textContent || '' : '';
      const active = document.activeElement;
      const selection = active && secret(active) ? '' : String(window.getSelection() || '');
      const r = {
        title: document.title,
        url: location.href,
        text: text.slice(0, MAX_TEXT),
        truncated: text.length > MAX_TEXT,
        selection: selection.slice(0, 20000),
      };
      if (args.elements) {
        page.refs = root ? collect(document, []) : [];
        r.elements = page.refs.map((el, i) => `e${i + 1} ${describe(el)}`);
        r.capped = page.refs.length >= MAX_ELEMENTS;
      }
      return r;
    },

    find(args) {
      if (args.ref) {
        const el = page.refs[Number(String(args.ref).replace(/^e/, '')) - 1];
        if (!el) throw new Error(`no element ${args.ref}: read the page with elements first`);
        if (!el.isConnected) throw new Error(`${args.ref} is gone: the page changed since browser_read; read it again`);
        return el;
      }
      if (args.selector) {
        let all;
        try {
          all = [...document.querySelectorAll(args.selector)];
        } catch (e) {
          throw new Error(`bad selector: ${args.selector}`);
        }
        const el = all.find(visible) || all[0];
        if (!el) throw new Error(`nothing matches ${args.selector}`);
        return el;
      }
      return null;
    },

    // What the page shows around the element and in dialogs, to tell what an action changed.
    snapshot(el) {
      const area = (el && el.isConnected && el.closest('form, [role=dialog], dialog, section, main')) || document.body;
      const dialogs = [...document.querySelectorAll('dialog[open], [role=dialog], [role=alertdialog], [aria-modal=true]')]
        .filter(visible);
      return {
        url: location.href,
        area: area ? area.innerText || '' : '',
        dialogs: dialogs.map((d) => clean(d.innerText, 300)),
      };
    },

    keyEvent(target, type, combo) {
      const parts = combo.split('+');
      const key = parts.pop();
      const mods = new Set(parts.map((p) => p.toLowerCase()));
      const name = key.length === 1 ? key : key[0].toUpperCase() + key.slice(1);
      return target.dispatchEvent(new KeyboardEvent(type, {
        key: name, bubbles: true, cancelable: true, composed: true,
        ctrlKey: mods.has('ctrl'), shiftKey: mods.has('shift'), altKey: mods.has('alt'), metaKey: mods.has('meta'),
      }));
    },

    pointer(el) {
      const r = el.getBoundingClientRect();
      const at = { bubbles: true, cancelable: true, composed: true, clientX: r.left + r.width / 2, clientY: r.top + r.height / 2, button: 0 };
      el.dispatchEvent(new PointerEvent('pointerdown', at));
      el.dispatchEvent(new MouseEvent('mousedown', at));
      if (el.focus) el.focus();
      el.dispatchEvent(new PointerEvent('pointerup', at));
      el.dispatchEvent(new MouseEvent('mouseup', at));
      el.click();
    },

    // Page dialogs (alert, confirm, prompt) would block the page; while acting they are recorded instead.
    // confirm() answers what the agent asked for (default: cancel), prompt() always cancels.
    quietDialogs(accept, seen) {
      const w = window.wrappedJSObject;
      if (!w) {
        // Chromium: background.js swapped them in the page's world; they report here.
        const on = (e) => seen.push(clean(e.detail, 320));
        window.addEventListener('seimei-dialog', on);
        return () => window.removeEventListener('seimei-dialog', on);
      }
      const saved = { alert: w.alert, confirm: w.confirm, prompt: w.prompt };
      exportFunction((m) => { seen.push(`alert: ${clean(m, 300)}`); }, window, { defineAs: 'alert' });
      exportFunction((m) => { seen.push(`confirm: ${clean(m, 300)} (${accept ? 'accepted' : 'cancelled'})`); return accept; }, window, { defineAs: 'confirm' });
      exportFunction((m) => { seen.push(`prompt: ${clean(m, 300)} (cancelled)`); return null; }, window, { defineAs: 'prompt' });
      return () => Object.assign(w, saved);
    },

    async act(args) {
      const el = page.find(args);
      const action = args.action;
      if (!el && !['scroll', 'press'].includes(action)) throw new Error(`${action} needs a ref or a selector`);
      if (el && el.disabled && action !== 'scroll') throw new Error(`${args.ref || args.selector} is disabled`);
      const label = el ? `${args.ref || args.selector} ${describe(el)}` : 'the page';
      const before = page.snapshot(el);
      const seen = [];
      const restore = page.quietDialogs(Boolean(args.confirm), seen);
      let note = '';
      try {
        if (el && action !== 'scroll') el.scrollIntoView({ block: 'center', inline: 'center' });
        if (action === 'click') {
          if (el.tagName === 'A' && el.target === '_blank' && /^https?:/.test(el.href)) {
            return { label, newTab: el.href };
          }
          page.pointer(el);
        } else if (action === 'fill') {
          if (secret(el)) throw new Error(`${args.ref || args.selector} is a ${secret(el)} field: ask the user to fill it`);
          const text = String(args.text ?? '');
          if (el.tagName === 'SELECT') return page.act({ ...args, action: 'select' });
          el.focus();
          if (el.isContentEditable) {
            const range = document.createRange();
            range.selectNodeContents(el);
            const sel = window.getSelection();
            sel.removeAllRanges();
            sel.addRange(range);
            document.execCommand('insertText', false, text);
          } else if ('value' in el) {
            el.value = text;
            el.dispatchEvent(new InputEvent('input', { bubbles: true, composed: true, inputType: 'insertText', data: text }));
            el.dispatchEvent(new Event('change', { bubbles: true }));
          } else {
            throw new Error(`${args.ref || args.selector} is not a field`);
          }
        } else if (action === 'select') {
          if (el.tagName !== 'SELECT') throw new Error(`${args.ref || args.selector} is not a select`);
          const want = clean(args.text, 1000);
          const opt = [...el.options].find((o) => o.value === want) || [...el.options].find((o) => clean(o.label || o.text, 1000) === want);
          if (!opt) throw new Error(`no option "${want}" in ${args.ref || args.selector}`);
          el.focus();
          el.value = opt.value;
          el.dispatchEvent(new Event('input', { bubbles: true }));
          el.dispatchEvent(new Event('change', { bubbles: true }));
        } else if (action === 'check' || action === 'uncheck') {
          const want = action === 'check';
          const checked = 'checked' in el ? el.checked : el.getAttribute('aria-checked') === 'true';
          if (checked === want) note = `it was already ${want ? 'checked' : 'unchecked'}`;
          else page.pointer(el);
        } else if (action === 'press') {
          const target = el || document.activeElement || document.body;
          if (el && el.focus) el.focus();
          for (const combo of String(args.text || '').split(/\s+/).filter(Boolean)) {
            const go = page.keyEvent(target, 'keydown', combo);
            page.keyEvent(target, 'keypress', combo);
            page.keyEvent(target, 'keyup', combo);
            // A synthetic Enter does not submit a form by itself.
            const form = target.form;
            if (go && combo.toLowerCase() === 'enter' && form && target.tagName === 'INPUT') form.requestSubmit();
          }
        } else if (action === 'scroll') {
          if (el) el.scrollIntoView({ block: 'center' });
          else window.scrollBy(0, Number(args.amount) || window.innerHeight * 0.8);
          const h = document.documentElement.scrollHeight;
          note = `scrolled to ${Math.round(window.scrollY)} of ${h - window.innerHeight} px`;
        } else if (action === 'submit') {
          const form = el.form || el.closest('form') || el.querySelector('form');
          if (!form) throw new Error(`${args.ref || args.selector} is not in a form`);
          form.requestSubmit();
        } else if (action === 'focus') {
          el.focus();
        } else {
          throw new Error(`unknown action ${action}`);
        }
        await page.settle(1500);
      } finally {
        restore();
      }
      const after = page.snapshot(el);
      const old = new Set(before.area.split('\n').map((l) => l.trim()));
      const added = after.area.split('\n').map((l) => l.trim()).filter((l) => l && !old.has(l));
      return {
        label, note, dialogs: seen,
        newText: clean(added.join(' | '), 400),
        openDialogs: after.dialogs.filter((d) => !before.dialogs.includes(d)),
      };
    },

    // Resolves when the DOM has been quiet for 300 ms, or after max ms.
    settle(max) {
      return new Promise((resolve) => {
        let quiet;
        const done = () => { obs.disconnect(); clearTimeout(quiet); clearTimeout(cap); resolve(); };
        const obs = new MutationObserver(() => { clearTimeout(quiet); quiet = setTimeout(done, 300); });
        obs.observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
        quiet = setTimeout(done, 300);
        const cap = setTimeout(done, max);
      });
    },

    // Waits for a selector or text to appear, or a selector to go.
    // Hidden tabs throttle timers, so this watches mutations; the extension enforces the timeout and calls stop().
    wait(args) {
      const check = () => {
        if (args.for === 'text') return (document.body ? document.body.textContent : '').includes(args.value);
        const all = [...document.querySelectorAll(args.value)];
        if (args.for === 'gone') return !all.some(visible);
        return all.some(visible) ? all.filter(visible).length : false;
      };
      try {
        check();
      } catch (e) {
        throw new Error(`bad selector: ${args.value}`);
      }
      return new Promise((resolve) => {
        const t0 = performance.now();
        const finish = (n) => { page.stop(); resolve({ ms: Math.round(performance.now() - t0), matches: n }); };
        const now = check();
        if (now) return finish(now);
        page.observer = new MutationObserver(() => {
          const n = check();
          if (n) finish(n);
        });
        page.observer.observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
      });
    },

    stop() {
      if (page.observer) page.observer.disconnect();
      page.observer = null;
    },

    // The page's size and an element's box, in CSS pixels of the document, for screenshots.
    box(args) {
      const el = page.find(args);
      const d = document.documentElement;
      const r = el ? el.getBoundingClientRect() : null;
      return {
        width: d.scrollWidth, height: d.scrollHeight, viewW: window.innerWidth, viewH: window.innerHeight,
        x: window.scrollX, y: window.scrollY,
        el: r && { x: r.left + window.scrollX, y: r.top + window.scrollY, width: r.width, height: r.height },
      };
    },
  };
  globalThis.seimeiPage = page;
}
