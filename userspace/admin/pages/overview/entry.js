// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Overview: the first page, at /. Its page is the Overview helper's;
// its part gives the Overview's own needs (tasks that failed or stopped in
// the last 24 hours, and hand edits not seen in 7 days).

export default {
  id: 'overview',
  title: 'Overview',
  path: '/',
  icon: 'overview',
  nav: { order: 10 },
  phone: 1,
  keywords: ['home', 'health', 'status', 'dashboard', 'summary'],
  css: ['boards.css', 'pages/overview/overview.css'],
  load: () => import('./page.js'),
  parts: {
    overview: () => import('./overview.js'),
  },
};
