// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Certificates. This folder holds parts only until C6 adds the page (see
// services/entry.js).

export default {
  id: 'certificates',
  title: 'Certificates',
  path: '/certificates',
  icon: 'certs',
  keywords: ['certificate', 'https', 'tls', 'renew', 'let\'s encrypt', 'expire'],
  parts: {
    overview: () => import('./overview.js'),
    palette: () => import('./palette.js'),
  },
};
