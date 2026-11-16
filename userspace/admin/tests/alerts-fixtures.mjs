// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What alertd and admind answer to the Alerts page, as the commands print it
// and as a test wants it. Not a test itself.

import { makeTime } from '../lib/time.js';

/** 3:12 PM on Saturday 14 November 2026, UTC. */
export const NOW = Date.UTC(2026, 10, 14, 15, 12, 0);
export const time = makeTime('UTC', () => NOW);

/** Unix seconds of `hour`:`minute` UTC on `day` November. */
export const at = (day, hour, minute = 0) => Date.UTC(2026, 10, day, hour, minute) / 1000;

const plain = (value) => JSON.parse(JSON.stringify(value));

/** `alerts list --json`: a critical alert since Nov 13 and a warning since Nov 14. */
export const active = () => plain([
  { source: 'metricsd', alert: { key: 'fs-full:/users', severity: 'warning', title: '/users is 93% full', detail: 'Free some room.' }, since: at(14, 9), changed: at(14, 9) },
  { source: 'diskhealth', alert: { key: 'smart:wd-1', severity: 'critical', title: 'A disk is failing', detail: 'Copy its data off.' }, since: at(13, 10, 2), changed: at(13, 10, 2) },
]);

/** `alerts history --json`: a raise, a clear and a test. */
export const history = () => plain([
  { at: at(13, 10, 2), source: 'diskhealth', state: 'raised', alert: { key: 'smart:wd-1', severity: 'critical', title: 'A disk is failing', detail: '' } },
  { at: at(14, 9, 30), source: 'metricsd', state: 'cleared', alert: { key: 'fs-full:/data', severity: 'warning', title: '/data is 93% full', detail: '' } },
  { at: at(14, 7, 20), source: 'alertd', state: 'test', alert: { key: 'test', severity: 'warning', title: 'A test alert', detail: '' } },
]);

/** The email channel as `alerts status --json` lists it. */
export const emailChannel = (more = {}) => ({
  channel: 'email to ops@example.org and 1 more', kind: 'email', host: 'mail.example.org', severity: 'warning', cleared: true, about: null, number: null, id: null, name: null, last_ok: at(14, 7, 20), last_error: null, error_id: null, waiting: 0, ...more,
});

/** An ntfy webhook that failed at 2:00 AM and sent nothing before. */
export const ntfyChannel = (more = {}) => ({
  channel: 'ntfy (ntfy.sh)', kind: 'ntfy', host: 'ntfy.sh', severity: 'critical', cleared: false, about: ['data', 'stopped'], number: 2, id: null, name: null, last_ok: null, last_error: [at(14, 2), 'ntfy answered 403 Forbidden: forbidden'], error_id: 'ALR-102', waiting: 2, ...more,
});

/** The push subscription of a browser. */
export const pushChannel = (more = {}) => ({
  channel: 'push to Firefox on the kitchen tablet', kind: 'push', host: 'updates.push.services.example', severity: 'warning', cleared: false, about: ['hardware'], number: null, id: '0123456789abcdef', name: 'Firefox on the kitchen tablet', last_ok: null, last_error: null, error_id: null, waiting: 0, ...more,
});

/** `alerts test --json`'s answer as the action gives it: the JSON is a line of the output. */
export const tested = (deliveries) => ({ done: true, changed: true, output: ['alerts: test sent', JSON.stringify({ format: 1, deliveries })] });

/** 65 bytes as an uncompressed point, as base64url. */
export const PUBLIC_KEY = Buffer.concat([Buffer.from([4]), Buffer.alloc(64, 7)]).toString('base64url');
