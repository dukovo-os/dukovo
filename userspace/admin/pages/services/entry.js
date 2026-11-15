// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Services. This folder holds parts only until C4 adds the page: the
// Overview can name a failed service and the palette can find one. C4 adds
// page.js, sets `load` and `nav`, and changes nothing else.

export default {
  id: 'services',
  title: 'Services',
  path: '/services',
  icon: 'services',
  keywords: ['service', 'daemon', 'start', 'stop', 'restart', 'enable', 'disable', 'failed'],
  parts: {
    overview: () => import('./overview.js'),
    palette: () => import('./palette.js'),
    badge: () => import('./badge.js'),
  },
};
