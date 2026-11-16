// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Answers of the Security page's reads, in the shapes of design 11.7 to 11.11
// (`dukovo-check --json`, `ssh-server show --json`, `admin settings --json`,
// `wireguard show --json`, `firewall blocked --json`, `lastlog --json`), for
// the tests of the page. The times are those of snapshots-fixtures.mjs: now is
// Nov 14 2026, 10:12 UTC. Not a test itself, and never installed.

import { at, NOW, time } from './snapshots-fixtures.mjs';

export { at, NOW, time };

/** One check of the kept result. */
export const check = (id, result, more = {}) => ({
  area: 'ssh', check: `check ${id}`, result, detail: `${id} says so`, id, fix: `Fix ${id} this way. Then check again.`, ...more,
});

/** The kept result of dukovo-check: three warnings, 26 that pass and one skipped. */
export const kept = (more = {}) => ({
  version: 1,
  checked: '2026-11-14T05:17:03Z',
  release: '1.0.0-alpha.1',
  results: [
    check('SEC-110', 'warning', { detail: 'Passwords sign in over SSH', area: 'ssh' }),
    check('SEC-120', 'warning', { detail: 'alice has no password', area: 'accounts' }),
    check('SEC-130', 'warning', { detail: 'A newer release is waiting', area: 'updates' }),
    ...Array.from({ length: 26 }, (_, number) => check(`SEC-9${String(number).padStart(2, '0')}`, 'passed')),
    check('SEC-140', 'skipped', { detail: 'No clock to check' }),
  ],
  counts: { passed: 26, warning: 3, failed: 0, skipped: 1 },
  ...more,
});

/** `ssh-server show --json`. */
export const ssh = (more = {}) => ({
  on: true, running: true, port: 22, passwords: true, keys: true, allow_users: [], allow_groups: [], deny_users: [], deny_groups: [], forwarding: 'yes', gateway_ports: 'no', mistakes: [], host_key: 'SHA256:ZkAslGjFiUHdGf', trial: null, ...more,
});

/** `admin settings --json`. */
export const settings = (more = {}) => ({
  sign_in: { second_step: 'administrators', session_hours: 12, idle_minutes: 30 },
  console: { listen: 9090, reach: 'private', certificate: 'this-machine' },
  terminal: { allowed: 'everyone', sessions_per_account: 32 },
  wrong: null,
  ...more,
});

/** `firewall blocked --json`. */
export const blocked = (more = {}) => ({
  on: true,
  tries: 5,
  within_s: 600,
  first_s: 600,
  longest_s: 86_400,
  never: ['192.168.1.0/24', '10.8.0.0/24'],
  blocks: [
    { address: '203.0.113.41/32', until_ms: at(2026, 11, 14, 11, 19), today: 2, why: '3 wrong passwords over SSH in 2 minutes, for admin' },
    { address: '192.0.2.77/32', until_ms: at(2026, 11, 14, 12, 2), today: 1, why: '12 console sign-ins' },
  ],
  ...more,
});

/** `wireguard show --json`, with two devices. */
export const wireguard = (more = {}) => ({
  interfaces: [{
    name: 'wg0',
    address: '10.8.0.1/24',
    port: 51_820,
    endpoint: 'home.example.org',
    public_key: 'KEY=',
    up: true,
    devices: [
      { name: 'phone', public_key: 'P=', addresses: ['10.8.0.2/32'], endpoint: '198.51.100.7:40122', latest_handshake_ms: at(2026, 11, 14, 10, 10), received: 1234, sent: 5678 },
      { name: 'laptop', public_key: 'L=', addresses: ['10.8.0.3/32'], endpoint: null, latest_handshake_ms: 0, received: 0, sent: 0 },
    ],
    ...more,
  }],
});

/** The private key of a device in the fixtures: a secret that no page may keep. */
export const SECRET = 'SECRETSECRETSECRETSECRETSECRETSECRETSEC=';

/**
 * `wireguard add NAME --json`: the device's file, whose private key is shown
 * once. A real file is about 240 bytes, more than a QR code of the console
 * holds; `short` leaves out what would make it so.
 */
export const added = (name, { endpoint = 'home.example.org' } = {}) => ({
  name,
  address: '10.8.0.4/32',
  config: `[Interface]\nPrivateKey = ${SECRET}\nAddress = 10.8.0.4/32\n\n[Peer]\nPublicKey = ${'P'.repeat(43)}=\nEndpoint = ${endpoint}:51820\nAllowedIPs = 10.8.0.1/32\nPersistentKeepalive = 25\n`,
  live: true,
});

/** An endpoint so long that the device's file does not fit a QR code: past 412 bytes. */
export const LONG_ENDPOINT = `${'a'.repeat(63)}.${'b'.repeat(63)}.${'c'.repeat(63)}.example.org`;

/** `lastlog --json`. */
export const lastlog = () => [
  { name: 'owner', sign_in: { at_ms: at(2026, 11, 14, 10, 20), terminal: 'pts/1', from: '192.168.1.25', via: 'ssh' }, failed: 0, last_failed: null },
  { name: 'alice', sign_in: { at_ms: at(2026, 11, 13, 19, 40), terminal: 'pts/0', from: '10.8.0.2', via: 'ssh' }, failed: 0, last_failed: null },
  { name: 'sam', sign_in: { at_ms: at(2026, 11, 14, 9, 5), terminal: 'web', from: '192.168.1.30', via: 'console' }, failed: 0, last_failed: null },
  { name: 'ted', sign_in: { at_ms: at(2026, 11, 14, 10, 21), terminal: 'pts/2', from: '192.168.1.31', via: 'console' }, failed: 0, last_failed: null },
];

/** `user list --json`, as far as the SSH form reads it. */
export const accounts = () => [
  { name: 'owner', groups: ['admins', 'owner'] },
  { name: 'alice', groups: ['admins', 'alice'] },
  { name: 'sam', groups: ['sam', 'photos'] },
];

/** `GET /api/1/network`, as far as the reach check reads it. */
export const network = () => ({ cards: [{ name: 'net0', addresses: [{ address: '192.168.1.10', prefix: 24 }] }] });

/** Spark's document with a trial of `id` that has `leftMs` left. */
export const sparkWith = (id, leftMs = 120_000) => ({
  show_version: 1, now_ms: 1_000_000, services: [], trials: [{ id, files: [], services: [], user: 1000, left_ms: leftMs }], undone: [],
});

/** Spark's document with no trial. */
export const sparkIdle = () => ({
  show_version: 1, now_ms: 1_000_000, services: [], trials: [], undone: [],
});
