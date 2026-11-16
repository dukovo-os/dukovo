// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the commands behind the Updates page print, as the tests need it: the
// check, the slots and the history are the golden files of the Rust tests
// (userspace/programs/systools/tests/json), the rest is written as the
// manuals show it. Not a test itself, and never installed.

import { makeTime } from '../lib/time.js';

/** 3:12 PM on Saturday 14 November 2026, UTC. */
export const NOW = Date.UTC(2026, 10, 14, 15, 12, 0);
export const time = makeTime('UTC', () => NOW);

const plain = (value) => JSON.parse(JSON.stringify(value));

/** `update check --json`: a system update with a bridge, a firmware set, an app to update and one held, a withdrawn release that runs, and the apps that could not be checked. */
export const check = () => plain({
  format: 1,
  offers: [
    {
      item: 'system', from: '0.14.0', to: '0.16.0', state: 'update', wave_day: null, waves_held: null, withdrawn: null, bridge: '0.15.1', download: { files: 37, bytes: 19_322_880 },
    },
    {
      item: 'firmware', from: '2026.2', to: '2026.3', state: 'update',
    },
    {
      item: 'org.nginx.nginx', from: '1.2.0', to: '1.3.0', state: 'update', catalog: 'dukovo',
    },
    {
      item: 'org.example.notes', from: '1.2.0', to: '1.3.0', state: 'held', catalog: 'dukovo',
    },
  ],
  running_withdrawn: { version: '0.14.0', reason: 'it loses files', advice: 'rollback' },
  unchecked: [{ item: 'apps', why: 'bundled does not answer' }],
  notes: ['Your bundles update by themselves, daily, since 2026-10-02'],
  security: {
    system: [{
      id: 'DKSA-2026-0001', severity: 'high', summary: 'A local user can read another user\'s files', aliases: ['CVE-2026-12345'], fixed_in: ['0.16.0'], action: 'Update with `update apply`, then restart.', fix: 'offered', release: '0.16.0',
    }],
    app_flaws: 2,
  },
});

/** The check with each offer in the state `states` names (`{system, firmware}`), and nothing else the offers say changed. */
export function checkWith(states = {}, more = {}) {
  const doc = check();
  doc.offers = doc.offers.filter((offer) => offer.item === 'system' || offer.item === 'firmware');
  doc.running_withdrawn = null;
  doc.unchecked = [];
  doc.security = { system: [], app_flaws: 0 };
  for (const offer of doc.offers) Object.assign(offer, { state: states[offer.item] ?? offer.state }, more[offer.item]);
  return doc;
}

/** `update last --json`: what the daily check kept, 12 minutes ago. */
export const last = (doc = check(), more = {}) => ({
  format: 1, checked_ms: NOW - 12 * 60_000, by: 'updated', updates: doc, ...more,
});

/** `update settings --json`. */
export const settings = (more = {}) => ({
  format: 1,
  channel: 'stable',
  check: 'notify',
  apps: 'notify',
  metered: 'wait',
  unattended: false,
  restart: null,
  hold: { system: null, firmware: null },
  held: null,
  apps_automatic: false,
  apps_disagree: false,
  server: 'https://updates.dukovo.org',
  canary: false,
  ...more,
});

/** `update checks --json`. */
export const checks = () => ({
  format: 1,
  essential: ['logd', 'authd', 'consoled', 'netd', 'sshd'],
  stable: 120,
  deadline: 600,
  checks: [
    {
      name: 'nginx', command: ['curl', '-fsS', '--max-time', '30', 'https://localhost/'], required: true, timeout: 30, kind: 'url', target: 'https://localhost/',
    },
    {
      name: 'postgres', command: ['systemctl', 'is-active', 'postgres'], required: false, timeout: 5, kind: 'service', target: 'postgres',
    },
    {
      name: 'backup place', command: ['backup', 'check', 'nas'], required: false, timeout: 60, kind: 'command', target: null,
    },
  ],
});

/** `sysupdate status --json`: 0.15.0 runs in slot a, and 0.16.0 is staged in slot b with two tries left. */
export const slots = (more = {}) => ({
  format: 1,
  running: 'a',
  default: 'a',
  previous: null,
  staged: { slot: 'b', tries_left: 2 },
  next: 'b',
  unfinished_start: null,
  slots: [{ slot: 'a', version: '0.15.0', firmware: '2026.2' }, { slot: 'b', version: '0.16.0', firmware: '2026.3' }],
  channel: 'stable',
  canary: false,
  wave: 412,
  held: { system: null, firmware: null },
  last_health_check: { time: '2026-11-12T03:00:00Z', passed: true, detail: '14 services up' },
  last_update: { from: '0.14.0', to: '0.15.0', slot: 'a', settings_restored: false },
  rescue: '0.14.0',
  ...more,
});

/** `sysupdate status --json` after the update proved itself: 0.16.0 runs in slot b, and 0.15.0 is kept in slot a. */
export const idle = (more = {}) => slots({
  running: 'b',
  default: 'b',
  previous: 'a',
  staged: null,
  next: 'b',
  last_update: { from: '0.15.0', to: '0.16.0', slot: 'b', settings_restored: false },
  ...more,
});

/** `update history --json`. */
export const history = () => plain({
  format: 1,
  changes: [
    {
      start: 9, time: '2026-10-28T15:06:40Z', item: 'system', from: '0.14.0', to: '0.15.0', outcome: 'failed', error: 'UPD-101',
    },
    {
      start: 9, time: '2026-10-28T15:06:40Z', item: 'firmware', from: null, to: '2026.2', outcome: 'failed', error: 'UPD-101',
    },
    {
      start: 10, time: '2026-10-29T18:53:20Z', item: 'system', from: '0.15.0', to: '0.14.0', outcome: 'good', error: null,
    },
    {
      start: 11, time: '2026-11-06T21:20:00Z', item: 'system', from: '0.14.0', to: '0.15.0', outcome: 'unfinished', error: null,
    },
  ],
  starts_kept: 5,
  apps: [
    {
      time: '2026-11-01T02:26:40Z', id: 'org.nginx.nginx', for_everyone: true, what: 'updated', from: '1.2.0', to: '1.3.0', by_user: null, note: 'from catalog dukovo',
    },
    {
      time: '2026-11-03T10:00:00Z', id: 'org.example.notes', for_everyone: true, what: 'held', from: '1.2.0', to: '1.2.0', by_user: 1000, note: '',
    },
  ],
});

/** `update advisories --json`. */
export const advisories = () => plain({
  format: 1,
  system: [{
    action: 'Update to Dukovo 1.4.2 with `update apply`, then restart.', aliases: ['CVE-2026-12345'], fix: 'offered', fixed_in: ['1.4.2'], id: 'DKSA-2026-0001', release: '1.4.2', severity: 'high', summary: 'A local user can read another user\'s files',
  }],
  apps: [
    {
      action: null, aliases: [], component: null, fixed_by_update: '1.3.0', fixed_in: ['1.3.0'], id: 'DKSA-2026-0002', item: 'org.nginx.nginx', severity: 'high', source: 'dksa', summary: 'A crafted request stops the server', version: '1.2.0',
    },
    {
      action: null, aliases: [], component: 'libxyz 1.2.0', fixed_by_update: '9.9.9', fixed_in: ['1.2.1'], id: 'RUSTSEC-2026-0001', item: 'org.example.notes', severity: 'medium', source: 'catalog', summary: 'a flaw of the library', version: '1.0.0',
    },
  ],
  unchecked: [],
});

/** `bundle list --json`. */
export const bundles = () => ({
  format: 1,
  bundles: [
    {
      id: 'org.nginx.nginx', name: 'nginx', version: '1.2.0', for_everyone: true, catalog: 'dukovo', held: false, update: '1.3.0', flaws: { state: 'known', count: 1, worst: 'high' },
    },
    {
      id: 'org.example.notes', name: 'notes', version: '1.2.0', for_everyone: true, catalog: 'dukovo', held: true, update: null, flaws: { state: 'none', count: 0, worst: null },
    },
    {
      id: 'org.example.mine', name: 'mine', version: '0.1.0', for_everyone: false, catalog: null, held: true, update: null, flaws: { state: 'unknown', count: 0, worst: null },
    },
  ],
});

/** `changes --failed --json` for an update that fell back. */
export const failed = (more = {}) => ({
  failed: {
    start: 42, at_ms: NOW - 3 * 3_600_000, release: '0.16.0', slot: 'b', result: 'fell-back', check: 'BOOT-103', ready_ms: null, changes: [],
  },
  since_good: [{ what: 'org.nginx.nginx', version: '1.30.6', at_ms: NOW - 2 * 86_400_000 }, { what: 'Dukovo', version: '0.16.0', at_ms: null }],
  bisect: { running: false, try: null, of: null },
  ...more,
});

/** `changes bisect --status --json`: four changes to try, nothing begun. */
export const search = (more = {}) => ({
  running: false,
  try: null,
  of: null,
  failed_start: 42,
  levers: [
    {
      what: 'system', kind: 'slot', version: '0.16.0', at_ms: NOW - 3 * 86_400_000,
    },
    {
      what: 'boot options', kind: 'options', version: 'quiet nowatchdog', at_ms: null,
    },
    {
      what: 'org.nginx.nginx', kind: 'app', version: '1.30.6', at_ms: NOW - 2 * 86_400_000,
    },
    {
      what: 'org.python.python', kind: 'app', version: '3.14.1', at_ms: NOW - 2 * 86_400_000,
    },
  ],
  cannot: [{ what: 'enp1s0', kind: 'card', why: 'the machine\'s hardware, which no restart changes' }],
  tries: [],
  next_ms: null,
  result: null,
  ...more,
});

/** The search waiting on its second try, after a first that started well. */
export const running = (more = {}) => search({
  running: true,
  try: 2,
  of: 2,
  tries: [{
    try: 1, left_out: ['system', 'boot options'], confirm: false, verdict: 'good', start: 43, at_ms: NOW - 120_000,
  }, {
    try: 2, left_out: ['org.nginx.nginx'], confirm: false, verdict: null, start: null, at_ms: null,
  }],
  next_ms: NOW + 45_000,
  ...more,
});

/** The search that ended, naming a change. */
export const found = (confirmed, more = {}) => search({
  tries: [{
    try: 1, left_out: ['system', 'boot options'], confirm: false, verdict: 'good', start: 43, at_ms: NOW - 240_000,
  }, {
    try: 2, left_out: ['org.nginx.nginx'], confirm: false, verdict: 'good', start: 44, at_ms: NOW - 120_000,
  }],
  result: {
    end: 'found',
    what: 'org.nginx.nginx',
    version: '1.30.6',
    confirmed,
    undo: ['elevate bundle rollback org.nginx.nginx --for-everyone'],
    words: [`Found: org.nginx.nginx 1.30.6${confirmed ? ', confirmed' : ', by elimination'}`],
  },
  ...more,
});

/** The failure notice the machine kept. */
export const notices = () => ({
  notices: [{
    name: 'failed-start', kind: 'warn', lines: ['nginx failed a required check (BOOT-103).', 'Likely cause: its certificate is not readable. Fix: run `elevate certs repair`.'],
  }],
});
