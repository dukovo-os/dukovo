// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The list of pages and their data, by hand (design 3.2). Loading it loads
// no page code: `load` and `parts` import a page's modules when they are
// first needed. It is in the shell's start, so an entry holds data and no
// comments. A test fails when a folder in pages/ has no entry here, or an
// entry no folder.
//
// The navigation's order is fixed here, ten apart, in the Main board's
// order: Overview 10, Logs 20, Reliability 30, Storage 40, Snapshots and
// backups 50, Networking 60, Firewall 70, Accounts 80, Services 90, Apps 100,
// Certificates 110, Security 120, Hardware 130, Updates 140, Terminal 150, and
// Report a problem 1000 at the bottom (`nav: {order: 1000, bottom: true}`).
// `nav: null` is a page reached by a link, `load: null` a folder of parts
// only, and the Terminal is listed only when the session may open one.

const ENTRIES = [
  {
    id: 'overview', title: 'Overview', path: '/', icon: 'overview', nav: { order: 10 }, phone: 1,
    keywords: ['home', 'health', 'status', 'dashboard', 'summary'],
    css: ['dialogs.css', 'boards.css', 'pages.css', 'pages/overview/overview.css'],
    load: () => import('./pages/overview/page.js'),
    parts: { overview: () => import('./pages/overview/overview.js') },
  },
  {
    id: 'logs', title: 'Logs', path: '/logs', icon: 'logs', nav: { order: 20 }, phone: 3,
    keywords: ['log', 'journal', 'messages', 'errors'],
    css: ['dialogs.css', 'boards.css', 'pages.css', 'pages/logs/logs.css'],
    load: () => import('./pages/logs/page.js'),
  },
  {
    id: 'reliability', title: 'Reliability', path: '/reliability', icon: 'reliability', nav: { order: 30 },
    keywords: ['crash', 'starts', 'boot', 'history'],
    css: ['dialogs.css', 'boards.css', 'pages.css', 'pages/reliability/rel.css'],
    load: () => import('./pages/reliability/page.js'),
  },
  {
    id: 'storage', title: 'Storage', path: '/storage', icon: 'storage', nav: { order: 40 },
    keywords: ['disk', 'space', 'mirror', 'health', 'grow', 'shared folders'],
    css: ['dialogs.css', 'pages.css', 'pages/storage/sto.css'],
    load: () => import('./pages/storage/page.js'),
    parts: { overview: () => import('./pages/storage/overview.js') },
  },
  {
    id: 'snapshots', title: 'Snapshots and backups', path: '/snapshots', icon: 'snapshots', nav: { order: 50 },
    keywords: ['backup', 'restore', 'snapshot', 'older copies', 'recovery key'],
    children: [{ title: 'Backups', path: '/snapshots/backups' }],
    css: ['pages.css', 'pages/snapshots/snap.css'],
    load: () => import('./pages/snapshots/page.js'),
    parts: { overview: () => import('./pages/snapshots/overview.js') },
  },
  {
    id: 'move', title: 'Move to new hardware', path: '/move', icon: 'server', needs: 'admin',
    keywords: ['migrate', 'new machine', 'old machine', 'restore everything'],
    css: ['dialogs.css', 'pages.css', 'pages/move/move.css'],
    load: () => import('./pages/move/page.js'),
  },
  {
    id: 'networking', title: 'Networking', path: '/networking', icon: 'network', nav: { order: 60 },
    keywords: ['address', 'ip', 'dns', 'router', 'dhcp'],
    css: ['dialogs.css', 'pages.css', 'lib/trial-banner.css', 'pages/networking/net.css'],
    load: () => import('./pages/networking/page.js'),
  },
  {
    id: 'firewall', title: 'Firewall', path: '/firewall', icon: 'firewall', nav: { order: 70 },
    keywords: ['port', 'rule', 'allow', 'deny', 'blocked'],
    css: ['dialogs.css', 'pages.css', 'lib/trial-banner.css', 'pages/firewall/fw.css'],
    load: () => import('./pages/firewall/page.js'),
  },
  {
    id: 'accounts', title: 'Accounts', path: '/accounts', icon: 'accounts', nav: { order: 80 },
    keywords: ['user', 'people', 'group', 'administrator', 'ssh key', 'sessions', 'tokens'],
    css: ['pages.css', 'pages/accounts/acc.css'],
    load: () => import('./pages/accounts/page.js'),
  },
  {
    id: 'welcome', title: 'Welcome', path: '/welcome', icon: 'info',
    keywords: ['first sign-in', 'time zone', 'get started'],
    css: ['boards.css', 'pages.css', 'pages/welcome/welcome.css'],
    load: () => import('./pages/welcome/page.js'),
  },
  {
    id: 'setup', title: 'Set up this server', path: '/setup', icon: 'security', needs: 'admin',
    keywords: ['first set-up', 'make safe', 'finish setting up', 'checklist'],
    css: ['boards.css', 'pages/setup/setup.css'],
    load: () => import('./pages/setup/page.js'),
  },
  {
    id: 'account', title: 'Your account', path: '/account', icon: 'accounts',
    keywords: ['display', 'theme', 'dark', 'light', 'text size', 'time zone', 'second step', 'tokens', 'password', 'profile'],
    sections: [
      { title: 'Display', path: '/account#display' },
      { title: 'Time zone', path: '/account#zone' },
      { title: 'Second step', path: '/account#second-step' },
      { title: 'API tokens', path: '/account#tokens' },
    ],
    css: ['pages.css', 'pages/accounts/acc.css'],
    load: () => import('./pages/account/page.js'),
    parts: { overview: () => import('./pages/account/overview.js') },
  },
  {
    id: 'terminal', title: 'Terminal', path: '/terminal', icon: 'terminal', nav: { order: 150, flag: 'terminal' },
    keywords: ['shell', 'command line', 'console', 'ssh'],
    children: [
      { title: 'Sessions', path: '/terminal/sessions', badge: 'sessions' },
      { title: 'Terminal settings', path: '/terminal/settings' },
    ],
    sections: [{ title: 'Terminal keys', path: '/terminal/settings#keys' }],
    css: ['pages/terminal/term.css'],
    load: () => import('./pages/terminal/page.js'),
    parts: {
      badge: () => import('./pages/terminal/badge.js'),
      palette: () => import('./pages/terminal/palette.js'),
    },
  },
  {
    id: 'services', title: 'Services', path: '/services', icon: 'services', nav: { order: 90 }, phone: 2,
    keywords: ['service', 'daemon', 'start', 'stop', 'restart', 'enable', 'disable', 'failed'],
    children: [{ title: 'Scheduled tasks', path: '/services/scheduled' }],
    css: ['dialogs.css', 'boards.css', 'pages.css', 'pages/services/svc.css'],
    load: () => import('./pages/services/page.js'),
    parts: {
      overview: () => import('./pages/services/overview.js'),
      palette: () => import('./pages/services/palette.js'),
      badge: () => import('./pages/services/badge.js'),
    },
  },
  {
    id: 'apps', title: 'Apps', path: '/apps', icon: 'apps', nav: { order: 100 },
    keywords: ['bundle', 'install', 'remove', 'catalog', 'rollback', 'licence'],
    css: ['pages.css', 'pages/apps/apps.css'],
    load: () => import('./pages/apps/page.js'),
    parts: { palette: () => import('./pages/apps/palette.js') },
  },
  {
    id: 'certificates', title: 'Certificates', path: '/certificates', icon: 'certs', nav: { order: 110 },
    keywords: ['certificate', 'https', 'tls', 'renew', 'let\'s encrypt', 'expire', 'host name', 'domain'],
    css: ['pages.css', 'pages/certificates/certs.css'],
    load: () => import('./pages/certificates/page.js'),
    parts: {
      overview: () => import('./pages/certificates/overview.js'),
      palette: () => import('./pages/certificates/palette.js'),
    },
  },
  {
    id: 'security', title: 'Security', path: '/security', icon: 'security', nav: { order: 120 },
    keywords: ['checks', 'ssh', 'blocked', 'unblock', 'wireguard', 'private access', 'sign-in rules'],
    css: ['dialogs.css', 'boards.css', 'pages.css', 'lib/trial-banner.css', 'pages/security/sec.css'],
    load: () => import('./pages/security/page.js'),
    parts: { overview: () => import('./pages/security/overview.js') },
  },
  {
    id: 'hardware', title: 'Hardware', path: '/hardware', icon: 'hardware', nav: { order: 130 },
    keywords: ['restart', 'shut down', 'reboot', 'power off', 'temperature', 'fan', 'memory test', 'devices', 'driver', 'self-test'],
    css: ['pages.css', 'pages/hardware/hardware.css'],
    load: () => import('./pages/hardware/page.js'),
    parts: { overview: () => import('./pages/hardware/overview.js'), palette: () => import('./pages/hardware/palette.js') },
  },
  {
    id: 'updates', title: 'Updates', path: '/updates', icon: 'updates', nav: { order: 140 },
    keywords: ['update', 'upgrade', 'rollback', 'go back', 'channel', 'hold', 'firmware', 'bisect', 'health check'],
    css: ['pages.css', 'pages/updates/updates.css'],
    load: () => import('./pages/updates/page.js'),
    parts: {
      overview: () => import('./pages/updates/overview.js'),
      badge: () => import('./pages/updates/badge.js'),
      palette: () => import('./pages/updates/palette.js'),
    },
  },
  {
    id: 'report', title: 'Report a problem', path: '/report', icon: 'report', nav: { order: 1000, bottom: true },
    keywords: ['bug', 'crash', 'problem', 'feedback', 'support'],
    css: ['pages.css', 'pages/report/report.css'],
    load: () => import('./pages/report/page.js'),
  },
  {
    id: 'alerts', title: 'Alerts', path: '/alerts', icon: 'bell', phone: 4, needs: 'admin',
    keywords: ['alert', 'notification', 'email', 'webhook', 'ntfy', 'push', 'phone'],
    css: ['pages.css', 'pages/alerts/alerts.css'],
    load: () => import('./pages/alerts/page.js'),
    parts: { overview: () => import('./pages/alerts/overview.js'), palette: () => import('./pages/alerts/palette.js') },
  },
];

// What an entry leaves out: no place in the navigation or the phone's tabs, a
// page for any signed-in person, nothing to find, no children, sections,
// style sheets or parts, and no page of its own.
const DEFAULTS = {
  nav: null, phone: null, needs: 'person', keywords: [], children: [], sections: [], css: [], load: null, parts: {},
};

export const PAGES = ENTRIES.map((entry) => ({ ...DEFAULTS, ...entry }));

/** The entry with `id`, or undefined. */
export const pageById = (id) => PAGES.find((entry) => entry.id === id);
