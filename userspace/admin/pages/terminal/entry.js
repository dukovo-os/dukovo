// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Terminal, at /terminal and under it. It is in the navigation only
// when the session says the account may open one (`session.terminal`).

export default {
  id: 'terminal',
  title: 'Terminal',
  path: '/terminal',
  icon: 'terminal',
  nav: { order: 150, flag: 'terminal' },
  keywords: ['shell', 'command line', 'console', 'ssh'],
  children: [
    { title: 'Sessions', path: '/terminal/sessions', badge: 'sessions' },
    { title: 'Terminal settings', path: '/terminal/settings' },
  ],
  sections: [{ title: 'Terminal keys', path: '/terminal/settings#keys' }],
  css: ['pages/terminal/term.css'],
  load: () => import('./page.js'),
  parts: {
    badge: () => import('./badge.js'),
    palette: () => import('./palette.js'),
  },
};
