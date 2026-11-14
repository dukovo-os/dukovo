// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "Your account", at /account (design 12.3): display, time zone, the second
// step, API tokens and the terminal's settings, until Accounts has them.

export default {
  id: 'account',
  title: 'Your account',
  path: '/account',
  icon: 'accounts',
  keywords: ['display', 'theme', 'dark', 'light', 'text size', 'time zone', 'second step', 'tokens', 'password', 'profile'],
  sections: [
    { title: 'Display', path: '/account#display' },
    { title: 'Time zone', path: '/account#zone' },
    { title: 'Second step', path: '/account#second-step' },
    { title: 'API tokens', path: '/account#tokens' },
  ],
  load: () => import('./page.js'),
  parts: {
    overview: () => import('./overview.js'),
  },
};
