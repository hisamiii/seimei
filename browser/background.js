// The seimei connector: a native-messaging port to host.py, which relays to the seimei server on this laptop.
// Requests come as {id, op, args}; each gets {id, ok, result} or {id, ok: false, error}.
const HOST = 'seimei';

let port = null;

function connect() {
  port = browser.runtime.connectNative(HOST);
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

// Private windows stay out of reach (and the extension is not allowed there anyway).
async function tab(id) {
  const t = await browser.tabs.get(Number(id));
  if (t.incognito) throw new Error('that tab is in a private window');
  return t;
}

const info = (t) => ({
  id: t.id, window: t.windowId, index: t.index, title: t.title || '', url: t.url || '',
  active: t.active, pinned: t.pinned, audible: Boolean(t.audible), asleep: Boolean(t.discarded),
});

const web = (url) => {
  const u = new URL(url);
  if (u.protocol !== 'http:' && u.protocol !== 'https:') throw new Error('only http and https links');
  return u.href;
};

const OPS = {
  async tabs() {
    const [tabs, focused] = await Promise.all([browser.tabs.query({}), browser.windows.getLastFocused()]);
    return { focused: focused.id, tabs: tabs.filter((t) => !t.incognito).map(info) };
  },
  async info({ tabs }) {
    return Promise.all(tabs.map(async (id) => info(await tab(id))));
  },
  async read({ tab: id }) {
    const t = await tab(id);
    if (t.discarded) await browser.tabs.reload(t.id);
    const [r] = await browser.tabs.executeScript(t.id, { file: '/read.js', runAt: 'document_idle' });
    return r;
  },
  async open({ url, background }) {
    const href = web(url);
    const t = await browser.tabs.create({ url: href, active: !background });
    // A new tab still shows about:blank until the page starts loading.
    return { ...info(t), url: href, title: t.title === href || !t.title ? '' : t.title };
  },
  async switch({ tab: id }) {
    const t = await tab(id);
    await browser.tabs.update(t.id, { active: true });
    await browser.windows.update(t.windowId, { focused: true });
    return info(await tab(t.id));
  },
  async close({ tabs }) {
    const ts = await Promise.all(tabs.map(tab));
    await browser.tabs.remove(ts.map((t) => t.id));
    return ts.length;
  },
  async download({ url, filename }) {
    const opts = { url: web(url), conflictAction: 'uniquify', saveAs: false };
    if (filename) opts.filename = filename;
    return { id: await browser.downloads.download(opts) };
  },
  async downloads({ limit }) {
    const items = await browser.downloads.search({ orderBy: ['-startTime'], limit: Math.min(Number(limit) || 10, 50) });
    return items.map((d) => ({
      id: d.id, url: d.url, path: d.filename, state: d.state, error: d.error || '', bytes: d.bytesReceived,
      total: d.totalBytes, started: d.startTime, mime: d.mime || '', exists: d.exists,
    }));
  },
};

connect();
