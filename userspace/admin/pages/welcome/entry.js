// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The first sign-in of each person, at /welcome (design 12.1): the time
// zone, the display and the second step, shown once.

export default {
  id: 'welcome',
  title: 'Welcome',
  path: '/welcome',
  icon: 'info',
  keywords: ['first sign-in', 'time zone', 'get started'],
  css: ['boards.css', 'pages/welcome/welcome.css'],
  load: () => import('./page.js'),
};
