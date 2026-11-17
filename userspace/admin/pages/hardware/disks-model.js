// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Disks card says, as plain values: each whole disk with its health
// pill, its last short and extended tests from the disk's own log, and the
// arguments of the check that runs a test. The answers are `storage show` and
// `diskhealth DISK --json` as the console's routes give them.

import { dayWords } from '../../lib/snapshot-words.js';

/** The health of a disk as Storage's list pills it: `{word, tone}`; `health` is null for a disk that gives none. */
export function pillOf(health) {
  if (!health) return { word: 'Not checked', tone: 'off' };
  if (health.verdict === 'failing') return { word: 'Failing', tone: 'bad' };
  return health.verdict === 'warning' || health.rose ? { word: 'Wearing', tone: 'warn' } : { word: 'Healthy', tone: 'ok' };
}

/** The whole disks of `storage` (`GET /api/1/storage`): `[{name, model, pill, hasHealth}]`; null when it was not read. */
export function disksOf(storage) {
  if (!Array.isArray(storage?.disks)) return null;
  return storage.disks.map((disk) => ({
    name: disk.name, model: disk.model || null, pill: pillOf(disk.health), hasHealth: Boolean(disk.health),
  }));
}

/** The arguments of `hardware.check` for a short test of `disk`, or an extended one. */
export const testArgs = (disk, full) => ({ areas: 'disks', disk, ...(full ? { full: true } : {}) });

export const DISKS_WORDS = 'A short test is the disk\'s health, its short self-test and a read of 256 places, about 3 minutes. An extended test is its long self-test and a read of all of it: hours.';
export const LOCKED_TESTS = 'Unlock to see the disks\' own tests';
export const READER_TESTS = 'Administrators see the disks\' own tests';
export const NO_HEALTH = 'This disk does not report its health or its tests.';

const TESTS = Object.freeze([['short', 'Short test'], ['extended', 'Extended test']]);
const hours = (count) => new Intl.NumberFormat('en-US').format(count);

/** "Short test Nov 9, passed (at hour 21,800)": a test of the log; the day comes from the hours the disk has been on (`now`), and a failure has the tone `bad`. */
function testWords(name, test, on, time, now) {
  if (test.result === 'running') return { words: `${name} running` };
  const known = Number.isFinite(test.at_hours);
  const day = known && Number.isFinite(on) ? ` ${dayWords((now - Math.max(0, on - test.at_hours) * 3600) * 1000, time.zone, now * 1000)},` : '';
  return { words: `${name}${day} ${test.result}${known ? ` (at hour ${hours(test.at_hours)})` : ''}`, tone: test.result === 'failed' ? 'bad' : null };
}

/**
 * The last short and extended tests of a disk, `checked` being the answer of
 * `storage.health` for it (null when it could not be read): `[{words,
 * tone}]`, one for each kind the disk's log holds, or the one that says why
 * there is none. `time` is lib/time.js's and `now` Unix seconds.
 */
export function testsOf(checked, time, now) {
  if (checked === null) return [{ words: 'Its own tests could not be read' }];
  if (!Array.isArray(checked.self_tests)) return [{ words: 'This disk keeps no log of its own tests' }];
  const found = TESTS.flatMap(([kind, name]) => {
    const test = checked.self_tests.find((one) => one.kind === kind);
    return test ? [testWords(name, test, checked.hours, time, now)] : [];
  });
  return found.length > 0 ? found : [{ words: 'No short or extended test is in its log' }];
}
