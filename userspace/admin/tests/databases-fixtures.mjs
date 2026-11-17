// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Answers in the shapes of `database list --json` and of the reads Add a
// database makes (`cert list --json`, `user list --json`, `backup list
// --json`, `hostnamectl status --json`), for the tests of the Databases
// pages. A small server: a shop's database that other machines may reach, a
// blog with open sessions and a line written by hand, and notes, which
// signs in as an account; the firewall opens the port to one network and
// backups copy each database, the shop's of this morning. Not a test itself,
// and never installed.

import { at, backupsOf, place } from './snapshots-fixtures.mjs';
import { account } from './accounts-fixtures.mjs';

/** The password the lead's command would print, as a test must never find it in a page. */
export const PASSWORD = 'Zq7-sentinel-password-0123456789ab';

/** `server` of a read: PostgreSQL 19, running, on this machine only. */
export const server = (more = {}) => ({
  installed: true,
  enabled: true,
  running: true,
  version: 'PostgreSQL 19.0 on x86_64-pc-linux-gnu, compiled by gcc 15.2',
  listen: 'localhost',
  ssl: false,
  certificate: null,
  firewall: { networks: [], earlier: null },
  copies: false,
  ...more,
});

/** A database of a read. */
export const database = (name, more = {}) => ({
  name,
  owner: name,
  encoding: 'UTF8',
  provider: 'libc',
  locale: 'en_US.UTF-8',
  bytes: 38_000_000,
  sessions: 0,
  site: null,
  made_by_database: true,
  collation_mismatch: false,
  copy: null,
  ...more,
});

/** A role of a read. */
export const role = (name, more = {}) => ({
  name, login: true, password: true, made_by_database: true, ...more,
});

/** A line of `pg_hba.conf` of a read: the one `database allow` writes, unless `more` says otherwise. */
export const line = (databaseName, address, more = {}) => ({
  line: 90,
  type: 'hostssl',
  databases: [databaseName],
  users: [databaseName],
  address,
  method: 'scram-sha-256',
  by_hand: false,
  ...more,
});

/** `database list --json` for the small server. */
export const list = (more = {}) => ({
  format: 1,
  server: server({
    listen: '*',
    ssl: true,
    certificate: '/system/live/certificates/postgresql/this-machine/fullchain.pem',
    firewall: { networks: ['192.168.1.0/24'], earlier: null },
    copies: true,
  }),
  databases: [
    database('shop', { site: 'shop.example.org', bytes: 38_000_000, copy: { at_ms: at(2026, 11, 14, 2), bytes: 12_345_678 } }),
    database('blog', { sessions: 2, bytes: 1_200_000, collation_mismatch: true, copy: { at_ms: at(2026, 11, 13, 2), bytes: 345_678 } }),
    database('notes', { owner: 'webdev', provider: 'builtin', locale: 'C.UTF-8', bytes: 8000 }),
  ],
  roles: [
    role('shop'),
    role('blog'),
    role('webdev', { password: false }),
    role('postgres', { made_by_database: false }),
  ],
  access: [
    line('shop', '192.168.1.0/24', { line: 91, users: ['shop'] }),
    line('blog', '10.9.0.0/16', { line: 40, type: 'host', method: 'md5', by_hand: true }),
    line('all', '127.0.0.1/32', { line: 12, type: 'host', databases: ['all'], users: ['all'], by_hand: true }),
  ],
  problems: [],
  ...more,
});

/** A server with nothing made: PostgreSQL runs and has no database of this page's. */
export const empty = () => list({ databases: [], roles: [role('postgres', { made_by_database: false })], access: [], server: server() });

/** PostgreSQL is not installed: the lists are empty, as the command says. */
export const missing = () => list({
  server: server({ installed: false, enabled: false, running: false, version: '' }), databases: [], roles: [], access: [],
});

/** PostgreSQL is installed and off. */
export const off = () => list({ server: server({ enabled: false, running: false }), databases: [], roles: [], access: [] });

/** PostgreSQL does not like its own file: what `problems` says of a hand edit. */
export const broken = () => list({ problems: [{ id: 'DB-104', words: 'invalid authentication method "scram"', line: 41 }] });

/** `cert list --json`: this machine's own, one that names the machine, one that does not, and two that cannot be chosen. */
export const certificates = () => [
  { name: 'this-machine', kind: 'own', names: ['nas.example.org', 'nas.local'], days_left: 300, used_by: ['admind'] },
  { name: 'wide.example.org', kind: 'acme', names: ['*.example.org'], days_left: 40, used_by: [] },
  { name: 'blog.example.org', kind: 'acme', names: ['blog.example.org'], days_left: 40, used_by: ['nginx'] },
  { name: 'old.example.org', kind: 'acme', names: ['old.example.org'], days_left: -2, used_by: [] },
  { name: 'gone.example.org', kind: 'added', names: ['gone.example.org'], days_left: 90, revoked: true, used_by: [] },
];

/** `user list --json`: people, one disabled, one whose name a role cannot have. */
export const accounts = () => [
  account('webdev', { uid: 1001 }),
  account('chase', { uid: 1000, admin: true }),
  account('old', { uid: 1004, disabled: true }),
  account('my-user', { uid: 1005 }),
];

/** `backup list --json` with one place that copies /shared every night. */
export const backups = () => backupsOf(place('nas'));

/** `hostnamectl status --json`. */
export const machine = () => ({
  format: 1, name: 'nas.example.org', first: 'nas', domain: 'example.org', local: 'nas.local', release: null,
});

/** The line `database add --json` prints, inside the answer of the change. */
export const added = (database_, more = {}) => ({
  done: true,
  changed: true,
  output: [
    `database: added ${database_} (owner ${database_}, en_US.UTF-8)`,
    JSON.stringify({
      format: 1, database: database_, role: database_, password: PASSWORD, host: '127.0.0.1', port: 5432, ...more,
    }),
  ],
});

/** A task that ended well. */
export const finished = (id, log = ['PostgreSQL is listening on the network']) => ({
  id, state: 'done', step: 5, steps: 5, words: '', log, changed: true,
});

/** `databases.copies` for `name`: this morning's copy in the folder in use and last night's in a snapshot. */
export const copies = (name = 'shop') => ({
  format: 1,
  database: name,
  copies: [
    { at_ms: at(2026, 11, 14, 2), bytes: 12_345_678, from: 'live', path: `/shared/services/org.postgresql.postgresql/postgres/dumps/${name}.dump` },
    { at_ms: at(2026, 11, 13, 2), bytes: 11_000_000, from: '2026-11-13-0200', path: `/snapshots/shared/2026-11-13-0200/services/org.postgresql.postgresql/postgres/dumps/${name}.dump` },
  ],
});
