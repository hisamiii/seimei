// The page's service worker: it shows seimei's pushes while the page is closed or hidden, and a tap opens the page (a thread's, the thread).
// The server sends none while the page is on screen, and no take-backs (a browser shows every push it gets).

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (e) => e.waitUntil(self.clients.claim()));

self.addEventListener('push', (e) => {
  let d = {};
  try { d = e.data?.json() || {}; } catch {}
  const tag = d.tag || d.kind || 'seimei';
  e.waitUntil(self.registration.showNotification(d.title || 'seimei', {
    body: d.body || '',
    tag,
    renotify: d.alert === '1',
    icon: 'static/0/apple-touch-icon.png',
    data: { tab: d.tab || 'chat', thread: d.thread || null },
  }));
});

self.addEventListener('notificationclick', (e) => {
  e.notification.close();
  e.waitUntil((async () => {
    const open = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    const page = open.find((c) => new URL(c.url).origin === self.location.origin);
    // A side thread's answer opens that thread.
    const thread = e.notification.data?.thread;
    if (page) {
      if (thread) page.postMessage({ thread });
      return page.focus();
    }
    return self.clients.openWindow(thread ? `./#thread=${encodeURIComponent(thread)}` : './');
  })());
});
