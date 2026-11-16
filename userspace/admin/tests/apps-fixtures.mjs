// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What admind answers for the Apps pages: `bundle list --json` and `bundle
// info --json` as the manual writes them, with the members that the console
// asks of them (`license`, `service_list`, `provides`, `uses`, `used_by`), and
// the last check for updates. Not a test itself, and never installed.

import { makeTime } from '../lib/time.js';

/** The time of the fixtures: Nov 16 2026, 12:00 UTC, in milliseconds. */
export const NOW = Date.UTC(2026, 10, 16, 12, 0, 0);

const flaws = (state, count = 0, worst = null) => ({ state, count, worst });

/** One entry of `bundle list --json`. */
const bundle = (id, name, version, more = {}) => ({
  id, name, version, for_everyone: true, catalog: 'dukovo', held: false, update: null, flaws: flaws('none'), ...more,
});

/** `bundle list --json`: six apps for everyone and one that is a person's own. */
export const list = () => ({
  bundles: [
    bundle('io.nginx.nginx', 'nginx', '1.27.3'),
    bundle('io.valkey.valkey', 'Valkey', '8.0.3', { update: '8.0.4', flaws: flaws('known', 2, 'high') }),
    bundle('org.postgresql.postgresql', 'PostgreSQL', '17.2', { update: '17.3' }),
    bundle('net.php.php-fpm', 'PHP-FPM', '8.4.2', { update: '8.4.3' }),
    bundle('org.python.python', 'Python', '3.15.2'),
    bundle('org.sqlite.sqlite', 'SQLite', '3.47.2', { held: true, update: '3.48.0' }),
    bundle('org.example.tool', 'Tool', '1.0.0', { for_everyone: false, catalog: null }),
  ],
});

/** An offer of `update check --json` for an app. */
const offer = (item, from, to, more = {}) => ({
  item, from, to, state: 'update', catalog: 'dukovo', ...more,
});

/** `update last --json`: the check of 4 minutes before NOW, with the offers of the apps and one of the system. */
export const last = (more = {}) => ({
  format: 1,
  checked_ms: NOW - 4 * 60_000,
  by: 'updated',
  updates: {
    offers: [
      { item: 'system', from: '1.0.0-alpha.1', to: '1.0.0-alpha.2', state: 'update' },
      offer('io.valkey.valkey', '8.0.3', '8.0.4'),
      offer('org.postgresql.postgresql', '17.2', '17.3'),
      offer('net.php.php-fpm', '8.4.2', '8.4.3', { state: 'waiting', wave_day: 3 }),
      offer('org.sqlite.sqlite', '3.47.2', '3.48.0', { state: 'held' }),
    ],
    unchecked: [],
    ...more,
  },
});

/** `bundle info io.valkey.valkey --json`, with what the console asks of it. */
export const valkey = (more = {}) => ({
  id: 'io.valkey.valkey',
  name: 'Valkey',
  version: '8.0.3',
  publisher: 'The Valkey project',
  summary: 'Cache and message broker',
  for_everyone: true,
  key: 'RWQ3Yl5valkeykey',
  trust: { kind: 'catalog', catalog: 'dukovo', since: '2026-11-01T00:00:00Z' },
  files: 41,
  bytes: 9_437_184,
  support: null,
  update_feed: null,
  flaws: {
    state: 'listed',
    list_made: '2026-11-15T00:00:00Z',
    known: [
      { id: 'DKSA-2026-0007', severity: 'high', component: 'valkey', fixed: '8.0.4', summary: 'a crafted command stops the server' },
      { id: 'DKSA-2026-0003', severity: 'medium', component: 'valkey', fixed: null, summary: 'a slow command holds the server' },
    ],
  },
  permissions: [
    { name: 'files.app-data', lets: 'keep its data in its own folder', tier: null, need: 'required', folders: [{ folder: '/shared/services/io.valkey.valkey', access: 'read and write' }], ports: [], apps: [], why: null },
    { name: 'net.listen', lets: 'listen for connections', tier: null, need: 'required', folders: [], ports: [6379], apps: [], why: null },
  ],
  commands: ['valkey-cli'],
  commands_with_your_access: [],
  services: ['valkey: the server'],
  grants: [],
  installed: '2026-11-12T08:30:00Z',
  folder: '/applications/io.valkey.valkey',
  data: 'its data folder, /shared/services/io.valkey.valkey',
  previous: '8.0.2',
  held: false,
  license: 'BSD-3-Clause',
  service_list: [{ name: 'valkey', account: 'valkey', start: 'off' }],
  provides: [{ interface: 'redis', socket: '/run/valkey/valkey.sock' }],
  uses: [],
  used_by: [{ id: 'net.php.php-fpm', name: 'PHP-FPM', what: 'sessions' }],
  ...more,
});

/** `bundle info` for the other apps: the same facts with their own name and nothing that waits. */
export const plain = (id, name, version, more = {}) => valkey({
  id,
  name,
  version,
  summary: name,
  flaws: { state: 'none', list_made: '2026-11-15T00:00:00Z', known: [] },
  previous: null,
  license: 'MIT',
  service_list: [],
  provides: [],
  uses: [],
  used_by: [],
  services: [],
  permissions: [],
  ...more,
});

/** What Spark answers for `GET /api/1/services`, for the app's service. */
export const services = (state = 'stopped', enabled = false) => ({
  now_ms: 1000,
  services: [
    { name: 'valkey', description: 'Valkey', state, enabled },
    { name: 'admind', description: 'The console', state: 'running', enabled: true },
  ],
});

/** `update settings --json`, as far as the apps use it. */
export const settings = (apps = 'notify') => ({ format: 1, channel: 'stable', check: 'notify', apps });

/** A task, as `GET /api/1/tasks` lists it. */
export const task = (id, action, args, more = {}) => ({
  id, action, args, state: 'running', percent: null, ...more,
});

/** The time helper the pages are given, with the clock at NOW. */
export const time = makeTime('UTC', () => NOW);
