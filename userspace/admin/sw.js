// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The console's service worker: it shows a notice for each push alertd sends
// and opens the page the alert is about when the notice is pressed. Nothing
// else: no fetch handler and no cache, since the console never works offline
// and a worker that served old pages after an update would be worse.

// The page each alert is about, by the start of its key (alertd knows no
// console paths); anything else opens Alerts.
const PAGES = [
  [/^(smart|smart-rising|fs-full|fs-filling|btrfs-[a-z-]+):/, () => '/storage'],
  [/^(backup-failed|backup-overdue):/, () => '/snapshots/backups'],
  [/^service:(.+)$/, (found) => `/services/${encodeURIComponent(found[1])}`],
  [/^certificate:/, () => '/certificates'],
  [/^(temp:|memory-test$)/, () => '/hardware'],
  [/^(update-[a-z-]+|start-failed)$/, () => '/updates#happened'],
];

function pathFor(key) {
  for (const [pattern, path] of PAGES) {
    const found = pattern.exec(key);
    if (found) return path(found);
  }
  return '/alerts';
}

// A path is used only when it stays on this site: it starts with one slash.
const safe = (path) => (typeof path === 'string' && /^\/(?![/\\])/.test(path) ? path : '/alerts');

self.addEventListener('push', (event) => {
  let message = {};
  try {
    message = event.data.json();
  } catch {
    // A push with no words still has to show something.
  }
  const first = message.first ?? {};
  const host = String(message.host ?? 'Dukovo Admin');
  const count = Number.isInteger(message.count) ? message.count : 1;
  const key = String(first.key ?? '');
  const tag = `${first.source ?? ''}${key}`;
  const lines = [String(first.detail ?? '').split('\n')[0].slice(0, 300), count > 1 && `and ${count - 1} more`].filter(Boolean);
  const title = first.title ? `${first.state === 'cleared' ? 'Cleared: ' : ''}${first.title}` : 'An alert was told';
  event.waitUntil(self.registration.showNotification(`${host} · ${title}`, {
    body: lines.join('\n'),
    tag,
    renotify: first.severity === 'critical' && tag !== '',
    data: { path: pathFor(key) },
  }));
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const path = safe(event.notification.data?.path);
  const here = new URL(path, self.location.origin);
  event.waitUntil(self.clients.matchAll({ type: 'window', includeUncontrolled: true }).then((windows) => {
    const open = windows.find((one) => new URL(one.url).pathname === here.pathname);
    return open ? open.focus() : self.clients.openWindow(path);
  }));
});
