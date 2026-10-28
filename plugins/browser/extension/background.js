// The seimei connector: a native-messaging port to `seimei browser-host`, which relays to the seimei server on this computer.
// Requests come as {id, op, args}; each gets {id, ok, result} or {id, ok: false, error}.
// One script for Firefox (manifest.json, MV2) and Chromium browsers (chromium.json, MV3); FIREFOX picks the few calls that differ.
// Newer Chromium has a `browser` namespace too: Firefox is told apart by getBrowserInfo, which only it has.
const api = globalThis.browser ?? chrome;
const FIREFOX = typeof api.runtime.getBrowserInfo === 'function';
const HOST = 'seimei';
// The tab group seimei's own background tabs go into.
const GROUP = 'seimei';
// A screenshot's data URL stays under this, so the reply stays small.
const SHOT_MAX = 1100000;

let port = null;

function connect() {
  if (port) return;
  port = api.runtime.connectNative(HOST);
  port.onMessage.addListener(handle);
  port.onDisconnect.addListener(() => {
    port = null;
    setTimeout(connect, 10000);
  });
}

async function handle(m) {
  let reply;
  try {
    const op = OPS[m.op];
    if (!op) throw new Error(`unknown op ${m.op}`);
    reply = { id: m.id, ok: true, result: await op(m.args || {}) };
  } catch (e) {
    reply = { id: m.id, ok: false, error: String((e && e.message) || e) };
  }
  if (port) port.postMessage(reply);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Private windows stay out of reach (and the extension is not allowed there anyway).
async function tab(id) {
  const t = await api.tabs.get(Number(id));
  if (t.incognito) throw new Error('that tab is in a private window');
  return t;
}

// The ids of seimei's tab groups, by title (tab groups need Firefox 139 or later).
async function ourGroups() {
  if (!api.tabGroups) return new Set();
  return new Set((await api.tabGroups.query({ title: GROUP })).map((g) => g.id));
}

const info = (t, groups) => ({
  id: t.id, window: t.windowId, index: t.index, title: t.title || '', url: t.url || '',
  active: t.active, pinned: t.pinned, audible: Boolean(t.audible), asleep: Boolean(t.discarded),
  seimei: Boolean(groups && groups.has(t.groupId)),
});

const web = (url) => {
  const u = new URL(url);
  if (u.protocol !== 'http:' && u.protocol !== 'https:') throw new Error('only http and https links');
  return u.href;
};

// Resolves when the tab has finished loading, or after ms.
function loaded(id, ms) {
  return new Promise((resolve) => {
    const done = () => {
      api.tabs.onUpdated.removeListener(on);
      clearTimeout(cap);
      resolve();
    };
    const on = (tid, change) => { if (tid === id && change.status === 'complete') done(); };
    api.tabs.onUpdated.addListener(on);
    const cap = setTimeout(done, ms);
    api.tabs.get(id).then((t) => { if (t.status === 'complete') done(); }, done);
  });
}

// A tab to work in: a discarded one is loaded first, and one still loading gets to finish.
async function ready(t) {
  if (t.discarded) {
    await api.tabs.reload(t.id);
    await sleep(100);
  } else if (t.status !== 'loading') {
    return t;
  }
  await loaded(t.id, 30000);
  return tab(t.id);
}

// Runs page.js's fn(args) in one frame of the tab.
async function call(id, frame, fn, args) {
  if (FIREFOX) {
    await api.tabs.executeScript(id, { file: '/page.js', frameId: frame, runAt: 'document_idle' });
    const [r] = await api.tabs.executeScript(id, { code: `seimeiPage.${fn}(${JSON.stringify(args)})`, frameId: frame });
    return r;
  }
  const target = { tabId: id, frameIds: [frame] };
  await chrome.scripting.executeScript({ target, files: ['page.js'] });
  // Chromium drops an injected function's exception: it comes back as a value and is thrown here.
  const [r] = await chrome.scripting.executeScript({
    target,
    args: [fn, args],
    func: async (f, a) => {
      try {
        return { value: await globalThis.seimeiPage[f](a) };
      } catch (e) {
        return { error: String((e && e.message) || e) };
      }
    },
  });
  if (r.result.error) throw new Error(r.result.error);
  return r.result.value;
}

// Chromium: a content script cannot replace the page's alert, confirm and prompt, so swap them in the page's own world while acting.
// They report to page.js (quietDialogs) through a DOM event. Returns the undo.
async function pageDialogs(id, frame, accept) {
  if (FIREFOX) return async () => {};
  const target = { tabId: id, frameIds: [frame] };
  await chrome.scripting.executeScript({
    target,
    world: 'MAIN',
    args: [accept],
    func: (ok) => {
      const say = (text) => window.dispatchEvent(new CustomEvent('seimei-dialog', { detail: String(text) }));
      window.__seimeiDialogs = window.__seimeiDialogs || { alert: window.alert, confirm: window.confirm, prompt: window.prompt };
      window.alert = (m) => say(`alert: ${m}`);
      window.confirm = (m) => { say(`confirm: ${m} (${ok ? 'accepted' : 'cancelled'})`); return ok; };
      window.prompt = (m) => { say(`prompt: ${m} (cancelled)`); return null; };
    },
  });
  return () => chrome.scripting.executeScript({
    target,
    world: 'MAIN',
    func: () => {
      if (window.__seimeiDialogs) Object.assign(window, window.__seimeiDialogs);
      delete window.__seimeiDialogs;
    },
  }).catch(() => {});
}

// A screenshot of rect (page pixels) as a data URL.
// Chromium has no captureTab: its debugger protocol can take one of a background tab too (the browser shows "seimei started debugging this browser" meanwhile).
async function capture(id, opts) {
  if (FIREFOX) return api.tabs.captureTab(id, opts);
  const { format, quality, rect, scale } = opts;
  const target = { tabId: id };
  await chrome.debugger.attach(target, '1.3');
  try {
    const shot = chrome.debugger.sendCommand(target, 'Page.captureScreenshot', {
      format, quality: format === 'jpeg' ? quality : undefined, captureBeyondViewport: true,
      clip: { x: rect.x, y: rect.y, width: rect.width, height: rect.height, scale },
    });
    const r = await Promise.race([shot, sleep(15000).then(() => { throw new Error('the screenshot took too long'); })]);
    return `data:image/${format};base64,${r.data}`;
  } finally {
    await chrome.debugger.detach(target).catch(() => {});
  }
}

// The frames of each tab's last read with elements: f1 is frames.get(tab)[0].
const frames = new Map();

// "f2:e3" is element e3 in the second frame of the last read; "e3" is in the top frame.
function parseRef(tabId, ref) {
  if (!ref) return { frame: 0, ref: null };
  const m = /^(?:f(\d+):)?(e\d+)$/.exec(String(ref).trim());
  if (!m) throw new Error(`bad ref ${ref}: refs look like e14 or f2:e3`);
  if (!m[1]) return { frame: 0, ref: m[2] };
  const frame = (frames.get(tabId) || [])[Number(m[1]) - 1];
  if (frame === undefined) throw new Error(`no frame f${m[1]}: read the page with elements first`);
  return { frame, ref: m[2] };
}

// Resolves with true once the tab starts loading a new page.
function navigation(id) {
  let on;
  const started = new Promise((resolve) => {
    on = (tid, change) => { if (tid === id && change.status === 'loading') resolve(true); };
    api.tabs.onUpdated.addListener(on);
  });
  return { started, stop: () => api.tabs.onUpdated.removeListener(on) };
}

// Put a tab into seimei's collapsed group in its window, made on first use.
async function group(t) {
  if (!api.tabGroups) return;
  const [g] = await api.tabGroups.query({ title: GROUP, windowId: t.windowId });
  if (g) {
    await api.tabs.group({ groupId: g.id, tabIds: [t.id] });
    return;
  }
  const id = await api.tabs.group({ tabIds: [t.id], createProperties: { windowId: t.windowId } });
  await api.tabGroups.update(id, { title: GROUP, collapsed: true, color: 'purple' });
}

async function open(url, front, opener) {
  const href = web(url);
  const props = { url: href, active: Boolean(front) };
  if (opener) props.openerTabId = opener;
  let t = await api.tabs.create(props);
  if (!front) {
    await group(t);
    t = await tab(t.id);
  }
  // A new tab still shows about:blank until the page starts loading.
  return { ...info(t, await ourGroups()), url: href, title: t.title === href || !t.title ? '' : t.title };
}

// Requests in flight per tab, for waiting until the network is idle. Long polls (older than LONG) do not count.
const LONG = 5000;
const inflight = new Map();
const lastNet = new Map();
function netStart(d) {
  if (d.tabId < 0) return;
  if (!inflight.has(d.tabId)) inflight.set(d.tabId, new Map());
  inflight.get(d.tabId).set(d.requestId, Date.now());
  lastNet.set(d.tabId, Date.now());
}
function netEnd(d) {
  if (d.tabId < 0 || !inflight.has(d.tabId)) return;
  inflight.get(d.tabId).delete(d.requestId);
  lastNet.set(d.tabId, Date.now());
}
api.webRequest.onBeforeRequest.addListener(netStart, { urls: ['<all_urls>'] });
api.webRequest.onCompleted.addListener(netEnd, { urls: ['<all_urls>'] });
api.webRequest.onErrorOccurred.addListener(netEnd, { urls: ['<all_urls>'] });
api.tabs.onRemoved.addListener((id) => { inflight.delete(id); lastNet.delete(id); frames.delete(id); });

function busy(id) {
  const now = Date.now();
  return [...(inflight.get(id) || new Map()).values()].filter((t) => now - t < LONG).length;
}

const OPS = {
  async about() {
    if (FIREFOX) {
      const b = await api.runtime.getBrowserInfo();
      return { name: b.name, version: b.version };
    }
    // Brands like "Not_A Brand" are noise; "Chromium" is listed by every Chromium browser, so it comes last.
    const brands = (navigator.userAgentData?.brands || []).filter((b) => !/not.a.brand/i.test(b.brand));
    const b = brands.find((x) => x.brand !== 'Chromium') || brands[0] || { brand: 'Chromium', version: '' };
    return { name: b.brand, version: b.version };
  },
  async tabs() {
    const [tabs, focused, groups] = await Promise.all([api.tabs.query({}), api.windows.getLastFocused(), ourGroups()]);
    return { focused: focused.id, tabs: tabs.filter((t) => !t.incognito).map((t) => info(t, groups)) };
  },
  async info({ tabs }) {
    const groups = await ourGroups();
    return Promise.all(tabs.map(async (id) => info(await tab(id), groups)));
  },
  async read({ tab: id, elements }) {
    const t = await ready(await tab(id));
    const r = await call(t.id, 0, 'read', { elements: Boolean(elements) });
    if (!elements) return r;
    // Elements in frames, after the top frame's, with the frame in their ref.
    const all = (await api.webNavigation.getAllFrames({ tabId: t.id })) || [];
    const seen = [];
    for (const f of all) {
      if (f.frameId === 0 || r.elements.length >= 300) continue;
      try {
        const fr = await call(t.id, f.frameId, 'read', { elements: true });
        if (!fr.elements.length) continue;
        seen.push(f.frameId);
        const n = seen.length;
        r.elements.push(`(frame f${n}: ${fr.url})`, ...fr.elements.map((l) => `f${n}:${l}`));
      } catch (e) {
        // Frames the extension may not enter (about: pages, add-on stores) are left out.
      }
    }
    frames.set(t.id, seen);
    r.elements = r.elements.slice(0, 320);
    return r;
  },
  async act(args) {
    const t = await ready(await tab(args.tab));
    const { frame, ref } = parseRef(t.id, args.ref);
    const before = t.url;
    const nav = navigation(t.id);
    let r = null;
    let moved = false;
    try {
      const undo = await pageDialogs(t.id, frame, Boolean(args.confirm));
      const run = call(t.id, frame, 'act', { ...args, ref });
      run.catch(() => {});
      run.finally(undo).catch(() => {});
      // A click that loads a new page ends the page's script: the navigation is the answer then.
      r = await Promise.race([run, nav.started.then(() => null)]);
      if (r === null) moved = true;
    } catch (e) {
      const now = await api.tabs.get(t.id);
      if (now.status !== 'loading' && now.url === before) throw e;
      moved = true;
    } finally {
      nav.stop();
    }
    if (r && r.newTab) {
      const opened = await open(r.newTab, false, t.id);
      return { label: r.label, opened };
    }
    if (moved) await loaded(t.id, 15000);
    const now = await tab(t.id);
    return {
      ...(r || { label: args.ref || args.selector || 'the page' }),
      navigated: moved || now.url !== before, url: now.url, title: now.title || '',
    };
  },
  async wait(args) {
    const t = await ready(await tab(args.tab));
    const end = Date.now() + Math.min(Number(args.timeout) || 10, 60) * 1000;
    const t0 = Date.now();
    const status = async () => {
      const now = await tab(t.id);
      return { ms: Date.now() - t0, url: now.url, title: now.title || '' };
    };
    if (args.for === 'navigation') {
      const now = await api.tabs.get(t.id);
      if (now.status !== 'loading') {
        const nav = navigation(t.id);
        const went = await Promise.race([nav.started, sleep(end - Date.now()).then(() => false)]);
        nav.stop();
        if (!went) return { ...(await status()), timedOut: true };
      }
      await loaded(t.id, Math.max(end - Date.now(), 0));
      const s = await status();
      return { ...s, timedOut: (await api.tabs.get(t.id)).status !== 'complete' };
    }
    if (args.for === 'idle') {
      while (Date.now() < end) {
        const now = await api.tabs.get(t.id);
        const quiet = Date.now() - (lastNet.get(t.id) || 0);
        if (now.status === 'complete' && busy(t.id) === 0 && quiet >= 500) return status();
        await sleep(100);
      }
      return { ...(await status()), timedOut: true, pending: busy(t.id) };
    }
    // selector, text, gone: watch the page; a navigation meanwhile starts the watch again on the new page.
    while (Date.now() < end) {
      try {
        const found = await Promise.race([
          call(t.id, 0, 'wait', { for: args.for, value: args.value }),
          sleep(end - Date.now()).then(() => null),
        ]);
        if (found) return { ...(await status()), matches: found.matches };
        break;
      } catch (e) {
        if (String(e.message || e).startsWith('bad selector')) throw e;
        await loaded(t.id, Math.max(end - Date.now(), 0));
      }
    }
    try {
      await call(t.id, 0, 'stop', {});
    } catch (e) {
      // The page went away: nothing to stop.
    }
    return { ...(await status()), timedOut: true };
  },
  async screenshot(args) {
    const t = await ready(await tab(args.tab));
    const { frame, ref } = parseRef(t.id, args.ref);
    if (frame) throw new Error('screenshots of elements inside frames are not supported; take the whole tab');
    const b = await call(t.id, 0, 'box', { ref });
    let rect;
    if (b.el) rect = b.el;
    else if (args.full) rect = { x: 0, y: 0, width: b.viewW, height: Math.min(b.height, 16384) };
    else rect = { x: b.x, y: b.y, width: b.viewW, height: b.viewH };
    if (rect.width < 1 || rect.height < 1) throw new Error('the element has no size on the page');
    const ladder = [['png', 100, 1], ['jpeg', 90, 1], ['jpeg', 75, 1], ['jpeg', 75, 0.7], ['jpeg', 70, 0.5], ['jpeg', 60, 0.35]];
    let data = '';
    let used = ladder[0];
    for (const step of ladder) {
      const [format, quality, scale] = step;
      data = await capture(t.id, { format, quality, rect, scale });
      used = step;
      if (data.length <= SHOT_MAX) break;
    }
    return {
      data, kind: used[0], scale: used[2], width: Math.round(rect.width), height: Math.round(rect.height),
      url: t.url, title: t.title || '',
    };
  },
  async open({ url, front }) {
    return open(url, front);
  },
  async navigate({ tab: id, url, go }) {
    const t = await tab(id);
    if (url) await api.tabs.update(t.id, { url: web(url) });
    else if (go === 'back') await api.tabs.goBack(t.id);
    else if (go === 'forward') await api.tabs.goForward(t.id);
    else if (go === 'reload') await api.tabs.reload(t.id);
    else throw new Error('give a url or go: back, forward or reload');
    await sleep(100);
    await loaded(t.id, 30000);
    return info(await tab(t.id), await ourGroups());
  },
  async switch({ tab: id }) {
    const t = await tab(id);
    await api.tabs.update(t.id, { active: true });
    await api.windows.update(t.windowId, { focused: true });
    return info(await tab(t.id), await ourGroups());
  },
  async close({ tabs }) {
    const ts = await Promise.all(tabs.map(tab));
    await api.tabs.remove(ts.map((t) => t.id));
    return ts.length;
  },
  async download({ url, filename }) {
    const opts = { url: web(url), conflictAction: 'uniquify', saveAs: false };
    if (filename) opts.filename = filename;
    return { id: await api.downloads.download(opts) };
  },
  async downloads({ limit }) {
    const items = await api.downloads.search({ orderBy: ['-startTime'], limit: Math.min(Number(limit) || 10, 50) });
    return items.map((d) => ({
      id: d.id, url: d.url, path: d.filename, state: d.state, error: d.error || '', bytes: d.bytesReceived,
      total: d.totalBytes, started: d.startTime, mime: d.mime || '', exists: d.exists,
    }));
  },
};

connect();
// Chromium may stop the background worker while seimei is down; an alarm wakes it to try the host again.
if (!FIREFOX) {
  chrome.alarms.create('connect', { periodInMinutes: 1 });
  chrome.alarms.onAlarm.addListener(connect);
}
