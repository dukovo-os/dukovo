// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What admind answers for the page that adds an app: `bundle search --json`
// and `bundle sheet --json` as the manual will write them (the members of
// `bundle info --json` that need no install, and the sheet's own), the backups
// and the services around them. Not a test itself, and never installed.

import { plain } from './apps-fixtures.mjs';

/** The SHA-256 of the file the sheet of nginx was made from. */
export const SHA = '9f2c4b7a1d0e8a63b5c1f4e7d2a9086b3c5e1f7a4d6b8c0e2f4a6c8e0b1d3f57';

/** Another file's SHA-256: what the catalog offers after it changed. */
export const OTHER_SHA = '1e3d5c7b9a0f2d4e6c8a0b2d4f6e8c0a1b3d5f7e9c1a3b5d7f9e1c3a5b7d9f01';

const flaws = (state, count = 0, worst = null) => ({ state, count, worst });

/** `bundle search --json`: five bundles of one catalog, in no order, and one catalog that could not be read when `unread` says so. */
export const search = (more = {}) => ({
  format: 1,
  bundles: [
    { id: 'org.postgresql.postgresql', name: 'PostgreSQL', version: '17.3', catalog: 'dukovo', flaws: flaws('known', 2, 'high'), installed: '17.2' },
    { id: 'org.nginx.nginx', name: 'nginx', version: '1.30.6', catalog: 'dukovo', flaws: flaws('none'), installed: null },
    { id: 'org.example.notes', name: 'Notes', version: '2.0.1', catalog: 'dukovo', flaws: flaws('unknown'), installed: null },
    { id: 'net.php.php', name: 'PHP', version: '8.4.3', catalog: 'dukovo', flaws: flaws('known', 1, 'medium'), installed: null },
    { id: 'org.python.python', name: 'python', version: '3.15.2', catalog: 'dukovo', flaws: flaws('none'), installed: null },
  ],
  unread: [],
  ...more,
});

/** The apps that are installed, as `GET /api/1/apps` lists them: only what the page asks of them. */
export const installedList = (...ids) => ({
  bundles: ids.map(([id, name, version]) => ({
    id, name, version, for_everyone: true, catalog: 'dukovo', held: false, update: null, flaws: flaws('none'),
  })),
});

/** `bundle sheet org.nginx.nginx --json`. */
export const nginx = (more = {}) => ({
  format: 1,
  id: 'org.nginx.nginx',
  name: 'nginx',
  version: '1.30.6',
  publisher: 'The nginx project',
  summary: 'Web server and reverse proxy',
  key: 'RWQ3Yl5nginxkey',
  trust: { kind: 'catalog', catalog: 'dukovo', since: '2026-11-01T00:00:00Z' },
  files: 212,
  bytes: 9_000_000,
  support: 'https://nginx.org/support',
  update_feed: null,
  flaws: { state: 'none', list_made: '2026-11-15T00:00:00Z', known: [] },
  permissions: [
    { name: 'net.listen', lets: 'listen for connections', tier: 'at install', need: 'required', folders: [], ports: [80, 443], apps: [], why: null },
    {
      name: 'files.shared', lets: 'read the folders of /shared', tier: 'with a warning', need: 'required', folders: [{ folder: '/shared/www', access: 'read' }], ports: [], apps: [], why: 'It serves your websites from /shared/www.',
    },
  ],
  commands: ['nginx'],
  commands_with_your_access: [],
  services: ['nginx: the web server'],
  license: 'BSD-2-Clause',
  service_list: [{ name: 'nginx', account: 'nginx', start: 'off' }],
  provides: [],
  uses: [],
  catalog: 'dukovo',
  sha256: SHA,
  download_bytes: 3_100_000,
  services_need_everyone: true,
  data: ['/shared/services/org.nginx.nginx/nginx'],
  installed: null,
  refusal: null,
  ...more,
});

/** `bundle sheet net.php.php --json`: PHP-FPM, which offers the socket a web server uses. */
export const php = (more = {}) => nginx({
  id: 'net.php.php',
  name: 'PHP',
  version: '8.4.3',
  summary: 'PHP, run as pages for a web server',
  service_list: [{ name: 'php-fpm', account: 'php-fpm', start: 'off' }],
  provides: [{ interface: 'fastcgi', socket: '/run/php-fpm.sock' }],
  data: ['/shared/services/net.php.php/php-fpm'],
  ...more,
});

/** `bundle sheet org.example.notes --json`: an app with no service, whose commands run with the person's access. */
export const notes = (more = {}) => nginx({
  id: 'org.example.notes',
  name: 'Notes',
  version: '2.0.1',
  summary: 'Notes at a terminal',
  publisher: 'Example Ltd',
  support: 'support@example.org',
  permissions: [],
  commands: ['notes'],
  commands_with_your_access: ['notes-sync'],
  services: [],
  service_list: [],
  services_need_everyone: false,
  data: [],
  uses: [{ interface: 'fastcgi', socket: null }],
  ...more,
});

/** `GET /api/1/backups`: one place that copies the folders of `places`. */
export const backups = (...places) => ({ format: 1, places: [{ name: 'nas', every: 'night', places, state: {} }], staged: null });

/** What `bundle info` says of an app that provides something: only what a sheet's `uses` is matched against. */
export const provider = (id, name, links) => plain(id, name, '1.0.0', { provides: links });
