// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the commands behind the Certificates page print, as the tests need
// it: `cert list --json`, `cert show --json`, `cert history --json`,
// `cert get --check --json`, `hostnamectl status --json` and the answers of
// the routes around them, as their manuals and Rust tests show them. Not a
// test itself, and never installed.

import { makeTime } from '../lib/time.js';

/** 3:12 PM on Saturday 14 November 2026, UTC. */
export const NOW = Date.UTC(2026, 10, 14, 15, 12, 0);
export const time = makeTime('UTC', () => NOW);

const DAY = 86_400_000;
const iso = (ms) => new Date(ms).toISOString().replace(/\.\d{3}Z$/, 'Z');
const plain = (value) => JSON.parse(JSON.stringify(value));

/** The SHA-256 fingerprint of a certificate, as `cert` prints it: 64 hex digits. */
export const FINGERPRINT = 'ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12';

/** The same, as browsers show it. */
export const SHOWN_FINGERPRINT = FINGERPRINT.toUpperCase().match(/.{2}/g).join(':');

const account = { made_ms: Date.UTC(2026, 10, 2, 9, 0, 0), contact: false };

/** One certificate of `cert list --json`, with what a test says over it. */
export const certificate = (more = {}) => plain({
  name: 'shop.example.org',
  kind: 'acme',
  names: ['shop.example.org', 'www.shop.example.org'],
  not_before: iso(NOW - 15 * DAY),
  not_after: iso(NOW + 75 * DAY),
  days_left: 75,
  fingerprint_sha256: FINGERPRINT,
  issuer: 'C=US, O=Let\'s Encrypt, CN=R11',
  trusted: true,
  used_by: ['nginx'],
  renews: iso(Date.UTC(2026, 11, 30, 3, 0, 0)),
  account,
  ...more,
});

/** This server's own certificate, made nine days ago and good for 800 days. */
export const own = (more = {}) => certificate({
  name: 'this-machine',
  kind: 'self-signed',
  names: ['duke-nas.example.org', 'duke-nas', 'duke-nas.local', 'localhost'],
  not_before: iso(NOW - 9 * DAY),
  not_after: iso(NOW + 791 * DAY),
  days_left: 791,
  issuer: 'CN=duke-nas.example.org',
  trusted: false,
  used_by: ['admind'],
  renews: undefined,
  account: null,
  ...more,
});

/** `cert list --json` on a machine that has each kind. */
export const list = () => [
  own(),
  certificate(),
  certificate({
    name: 'photos.example.org',
    names: ['photos.example.org'],
    not_after: iso(NOW + 11 * DAY),
    days_left: 11,
    renews: undefined,
    used_by: [],
  }),
  certificate({
    name: 'intranet.example.org',
    kind: 'added',
    names: ['intranet.example.org'],
    issuer: 'O=Example Corp, CN=Example Intranet CA',
    trusted: false,
    not_after: iso(NOW + 200 * DAY),
    days_left: 200,
    used_by: [],
    renews: undefined,
    account: null,
  }),
  certificate({
    name: 'blog.example.org',
    names: ['blog.example.org'],
    revoked: true,
    used_by: [],
    renews: undefined,
  }),
];

/** Only this server's own, as a new machine has it. */
export const onlyOwn = () => [own()];

/** `cert show NAME --json`: a row of the list, with its chain in PEM. */
export const show = (more = {}) => ({
  ...certificate(more),
  chain_pem: '-----BEGIN CERTIFICATE-----\nQ0hBSU4=\n-----END CERTIFICATE-----\n',
  why_not_trusted: null,
});

const event = (at, name, kind, detail, by = 'certd', took = null) => ({
  at_ms: at, name, event: kind, detail, took_ms: took, by,
});

/** `cert history --json`: the oldest first. */
export const history = () => [
  event(NOW - 40 * DAY, 'this-machine', 'made', 'made for duke-nas.example.org'),
  event(Date.UTC(2026, 9, 23, 6, 0, 0), 'shop.example.org', 'failed', 'Port 80 was closed (CRT-103)'),
  event(Date.UTC(2026, 9, 24, 6, 0, 0), 'shop.example.org', 'renewed', 'from Let\'s Encrypt', 'certd', 4000),
  event(Date.UTC(2026, 10, 1, 6, 0, 0), 'photos.example.org', 'renewed', 'from Let\'s Encrypt', 'certd', 4000),
  event(Date.UTC(2026, 10, 14, 6, 0, 0), 'photos.example.org', 'failed', 'Let\'s Encrypt could not reach port 80 (CRT-106)'),
];

/** `cert wanted --json` when nothing is tried again. */
export const noneWanted = () => ({ format: 1, wanted: [] });

/** `cert wanted --json`: an order certd tries again, and one it has given up. */
export const wanted = () => plain({
  format: 1,
  wanted: [
    {
      name: 'blog.example.org',
      names: ['blog.example.org', 'www.blog.example.org', 'news.blog.example.org'],
      directory: 'https://acme-v02.api.letsencrypt.org/directory',
      first_ms: NOW - 3 * 3_600_000,
      last_ms: NOW - 55 * 60_000,
      next_ms: Date.UTC(2026, 10, 14, 16, 5, 0),
      tries: 3,
      why: 'Let\'s Encrypt could not reach port 80 (CRT-106)',
      given_up: false,
    },
    {
      name: 'old.example.org',
      names: ['old.example.org'],
      directory: 'https://acme-v02.api.letsencrypt.org/directory',
      first_ms: NOW - 15 * DAY,
      last_ms: NOW - DAY,
      next_ms: 0,
      tries: 40,
      why: 'No address for old.example.org (CRT-104)',
      given_up: true,
    },
  ],
});

/** `hostnamectl status --json`. */
export const machine = (more = {}) => ({
  format: 1, name: 'duke-nas', first: 'duke-nas', domain: null, local: 'duke-nas.local', release: 'Dukovo Server 0.15.0', ...more,
});

/** The same, for a machine that has a domain. */
export const machineWithDomain = () => machine({ name: 'duke-nas.example.org', domain: 'example.org' });

/** `GET /api/1/console/about`. */
export const about = (more = {}) => ({
  host: 'duke-nas', addresses: ['192.168.1.10'], port: 9090, fingerprint: SHOWN_FINGERPRINT, release: 'Dukovo Server 0.15.0', ...more,
});

/** `GET /api/1/security/sign-in`, whose `console.certificate` names what the console serves. */
export const signIn = (certificate = 'this-machine') => ({
  format: 1,
  sign_in: { second_step: 'off', session_hours: 12, idle_minutes: 30 },
  console: { listen: '0.0.0.0:9090', reach: 'private', certificate },
  terminal: { allowed: 'admins', sessions_per_account: 3 },
  wrong: null,
});

const check = (result, name, words, id = null) => ({
  id, result, name, words,
});

/** `cert get --check --json` when everything passes. */
export const report = (more = {}) => plain({
  format: 1,
  checks: [
    check('passed', 'shop.example.org', 'shop.example.org leads to 203.0.113.10'),
    check('passed', 'www.shop.example.org', 'www.shop.example.org leads to 203.0.113.10'),
    check('passed', null, 'port 80 is open (firewall rule 3)'),
    check('passed', null, 'nginx holds port 80 and serves the answer'),
  ],
  firewall_rule: 3,
  port80_holder: 'nginx',
  terms_url: 'https://letsencrypt.org/documents/LE-SA-v1.5-February-24-2025.pdf',
  dns_records: [],
  ...more,
});

/** The check when port 80 is closed by the firewall. */
export const closed = () => report({
  checks: [
    check('passed', 'shop.example.org', 'shop.example.org leads to 203.0.113.10'),
    check('failed', null, 'the firewall closes port 80, which the authority must reach: open it with `elevate firewall allow tcp 80`', 'CRT-103'),
    check('passed', null, 'port 80 is free: the order answers the authority itself'),
  ],
  firewall_rule: null,
  port80_holder: null,
});

/** The check when one name has no address, and the others pass. */
export const oneMissing = () => report({
  checks: [
    check('passed', 'shop.example.org', 'shop.example.org leads to 203.0.113.10'),
    check('failed', 'www.blog.example.org', 'www.blog.example.org does not resolve to an address: give it an A record (and AAAA for IPv6) that leads to this machine', 'CRT-101'),
    check('passed', null, 'port 80 is open (firewall rule 3)'),
    check('passed', null, 'nginx holds port 80 and serves the answer'),
  ],
});

/** The check of names proved by DNS: the checks of the names and the port are left out. */
export const byDns = () => report({
  checks: [
    check('skipped', 'shop.example.org', 'shop.example.org is not looked up: the authority never connects to this machine when the names are proved by DNS'),
    check('skipped', null, 'port 80 is not checked: the authority never connects to this machine when the names are proved by DNS'),
  ],
  firewall_rule: null,
  port80_holder: null,
  dns_records: [{ name: '_acme-challenge.shop.example.org', type: 'TXT' }],
});

/** A task of `certificate.get` that runs at step `step`. */
export const task = (more = {}) => ({
  id: 41,
  action: 'certificate.get',
  state: 'running',
  step: 3,
  steps: 5,
  words: 'the authority checks port 80',
  percent: 40,
  who: 'chase',
  started: Math.floor(NOW / 1000) - 20,
  log: [],
  ...more,
});

/** The log of an order by DNS that waits for its records. */
export const dnsLog = () => [
  'step 1 of 6: checking the names and this machine',
  'add: _acme-challenge.shop.example.org TXT "abc-123_value"',
  'add: _acme-challenge.www.shop.example.org TXT "def-456_value"',
];
