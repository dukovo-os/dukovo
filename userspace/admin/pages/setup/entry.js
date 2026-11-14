// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The machine's first set-up, at /setup, for administrators (design 12.2).

export default {
  id: 'setup',
  title: 'Set up this server',
  path: '/setup',
  icon: 'security',
  needs: 'admin',
  keywords: ['first set-up', 'make safe', 'finish setting up', 'checklist'],
  css: ['boards.css', 'pages/setup/setup.css'],
  load: () => import('./page.js'),
};
