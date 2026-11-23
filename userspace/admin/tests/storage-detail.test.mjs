// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// One disk's panel and the steps to replace a disk (design 4.3 to 4.5), from
// fixtures of `storage show --json`: the rows of facts, the age and the last
// check in words, the scrub button, the room to grow into, and what the
// steps are about.

import assert from 'node:assert/strict';
import test from 'node:test';

import { makeTime } from '../lib/time.js';
import {
  ageWords, attributeRows, dayWords, detailOf, growOf, healthOf, lastFullCheck, scrubOf, stepsOf,
} from '../pages/storage/disk-detail.js';
import { findDisk } from '../pages/storage/model.js';
import {
  data, disk, GB, NOW, partition, passed, show, showAlone, showGrowable, showMissing, showWearing, SCRUB, system, volume,
} from './storage-fixtures.mjs';

const time = makeTime('UTC', () => NOW * 1000);

const detail = (answer, name, more = {}) => detailOf(findDisk(answer, name), answer, { time, now: NOW, admin: true, ...more });
const row = (found, label) => found.rows.find((one) => one.label === label);

test('the panel of a mirror disk says what it holds, its age, its counts and its last scrub', () => {
  const found = detail(showWearing(), 'sdb');
  assert.equal(found.name, 'sdb');
  assert.equal(found.line, 'WD Red Plus WD20EFZX · 2 TB');
  assert.deepEqual(found.health, { word: 'Wearing', tone: 'warn', note: '8 sectors moved to spares.' });
  assert.equal(row(found, 'Model').value, 'WD Red Plus WD20EFZX · serial SDB-SERIAL');
  assert.deepEqual(row(found, 'Used for').items, ['Half of the data mirror (Btrfs RAID1)']);
  assert.equal(row(found, 'Age').value, '2 years 6 months powered on (21,804 hours)');
  assert.equal(row(found, 'Temperature').value, '38 °C');
  assert.deepEqual(row(found, 'Moved sectors'), { label: 'Moved sectors', value: '8', tone: 'warn' });
  assert.deepEqual(row(found, 'Waiting sectors'), { label: 'Waiting sectors', value: '0', tone: undefined });
  assert.equal(row(found, 'Early warning').value, 'sdb moved 8 worn-out sectors to spares. Seen 20 hours ago.');
  assert.equal(row(found, 'Read errors fixed').value, '0 in the last scrub, Nov 1');
});

test('the panel of an NVMe drive has its media errors and the writes it used', () => {
  const found = detail(show(), 'nvme0');
  assert.equal(row(found, 'Media errors').value, '0');
  assert.equal(row(found, 'Wear').value, 'Used 4 percent of its rated writes');
  assert.equal(row(found, 'Moved sectors'), undefined);
  assert.deepEqual(row(found, 'Used for').items, ['Start-up files (the ESP)', 'The system (Btrfs single)']);
});

test('a disk that gives no health says so and has no counts', () => {
  const found = detail(show(), 'sdc');
  assert.equal(found.hasHealth, false);
  assert.deepEqual(found.health, { word: 'Not checked', tone: 'off', note: 'This disk does not report its health.' });
  assert.equal(row(found, 'Health').value, 'Not checked: this disk does not report its health');
  assert.equal(row(found, 'Age'), undefined);
  assert.equal(row(found, 'Last full check'), undefined);
  assert.equal(row(found, 'Read errors fixed'), undefined);
  assert.deepEqual(row(found, 'Used for').items, ['Unused']);
});

test('the last full check is for an administrator, and reads from Check now', () => {
  assert.equal(row(detail(show(), 'sdb', { admin: false }), 'Last full check').value, 'Administrators see the disk\'s own tests');
  assert.equal(row(detail(show(), 'sdb'), 'Last full check').value, 'Press Check now to read it');
  const checked = { hours: 21804, self_tests: [{ kind: 'short', result: 'passed', at_hours: 21800 }, { kind: 'extended', result: 'passed', at_hours: 21700 }] };
  assert.equal(row(detail(show(), 'sdb', { checked }), 'Last full check').value, 'Passed, Nov 10');
});

test('a self-test that failed, is running or is not there is told as it is', () => {
  assert.deepEqual(lastFullCheck([{ kind: 'extended', result: 'failed', at_hours: 21800 }], 21804, time, NOW), { value: 'Failed, Nov 14', tone: 'bad' });
  assert.deepEqual(lastFullCheck([{ kind: 'extended', result: 'running', at_hours: 21804 }], 21804, time, NOW), { value: 'Running now' });
  assert.deepEqual(lastFullCheck([{ kind: 'short', result: 'passed', at_hours: 1 }], 21804, time, NOW), { value: 'No full check is logged on this disk' });
  assert.deepEqual(lastFullCheck(null, 21804, time, NOW), { value: 'No full check is logged on this disk' });
  assert.deepEqual(lastFullCheck([{ kind: 'extended', result: 'passed' }], 21804, time, NOW), { value: 'Passed' });
});

test('the last scrub is told by what it found', () => {
  const answer = show();
  const make = (scrub) => {
    answer.volumes[1] = data({ scrub });
    return row(detail(answer, 'sda'), 'Read errors fixed');
  };
  assert.equal(make(null).value, 'No scrub has run yet');
  assert.equal(make({ ...SCRUB, errors: 3, fixed: 3, result: 'ok' }).value, '3 in the last scrub, Nov 1');
  assert.deepEqual(make({ ...SCRUB, errors: 5, fixed: 3, unfixable: 2, result: 'errors' }), {
    label: 'Read errors fixed', value: '3 of 5 in the last scrub, Nov 1; 2 could not be mended', tone: 'bad',
  });
  assert.equal(make({ ...SCRUB, result: 'stopped' }).value, 'The last scrub, Nov 1, was stopped before its end');
});

test('the scrub button says what a scrub is and how long it takes from the last one', () => {
  const answer = show();
  assert.deepEqual(scrubOf(answer.volumes[1]), {
    mount: '/volumes/data', label: 'Scrub the mirror', hint: 'This reads every block of 1.24 TB: about 3 hours. The machine stays usable, a little slower.',
  });
  assert.deepEqual(scrubOf(answer.volumes[0]), {
    mount: '/', label: 'Check every block', hint: 'This reads every block of 61 GB. The machine stays usable, a little slower.',
  });
  assert.equal(scrubOf(null), null);
  assert.equal(detail(answer, 'sda').scrub.label, 'Scrub the mirror');
  assert.equal(detail(showAlone(), 'nvme0').scrub.label, 'Check every block');
  assert.equal(detail(show(), 'sdc').scrub, null);
});

test('a disk with room after its last Btrfs partition can grow into it', () => {
  const answer = showGrowable();
  assert.deepEqual(growOf(answer.disks[1], answer), { bytes: 40 * GB, words: 'This disk has 40 GB after its last partition, which the data volume can grow into.' });
  assert.equal(detail(answer, 'sda').grow.bytes, 40 * GB);
  assert.equal(growOf(show().disks[1], show()), null);
});

test('a disk cannot grow when the room is 1 percent or less, or its last partition is not Btrfs', () => {
  const answer = show();
  answer.disks[1].free_after = 0.01 * answer.disks[1].size;
  assert.equal(growOf(answer.disks[1], answer), null);
  const swap = disk('sdd', 'D', 100 * GB, [partition('sdd1', 1, 0, 50 * GB, { type: 'swap', filesystem: 'swap' })], { free_after: 50 * GB });
  assert.equal(growOf(swap, answer), null);
  assert.equal(growOf(disk('sde', 'D', 100 * GB, [], { free_after: 100 * GB }), answer), null);
});

test('a Btrfs partition that no volume lists grows "the filesystem"', () => {
  const answer = show();
  const lone = disk('sdd', 'D', 100 * GB, [partition('sdd1', 1, 0, 50 * GB, { filesystem: 'btrfs' })], { free_after: 50 * GB });
  assert.equal(growOf(lone, answer).words, 'This disk has 50 GB after its last partition, which the filesystem can grow into.');
});

test('the age of a disk is in years and months, then months, days and hours', () => {
  assert.equal(ageWords(21804), '2 years 6 months powered on (21,804 hours)');
  assert.equal(ageWords(8766), '1 year powered on (8,766 hours)');
  assert.equal(ageWords(2000), '3 months powered on (2,000 hours)');
  assert.equal(ageWords(100), '4 days powered on (100 hours)');
  assert.equal(ageWords(12), '12 hours powered on (12 hours)');
  assert.equal(ageWords(null), null);
  assert.equal(ageWords(-1), null);
});

test('a day is written without its year in this year, and with it in another', () => {
  assert.equal(dayWords(NOW - 13 * 86400, time, NOW), 'Nov 1');
  assert.equal(dayWords(NOW - 400 * 86400, time, NOW), 'Oct 10, 2025');
  assert.equal(dayWords(NOW, makeTime('Pacific/Kiritimati', () => NOW * 1000), NOW), 'Nov 15');
});

test('the check\'s answer is the disk\'s object, whether diskhealth gave a list or one', () => {
  const one = { disk: 'sdb', attributes: [{ id: 5, name: 'Reallocated_Sector_Ct', prefail: true, value: 100, worst: 100, threshold: 10, raw: 8 }, { id: 9, name: 'Power_On_Hours', prefail: false, value: 90, worst: 90, threshold: 0, raw: 21804 }] };
  assert.equal(healthOf([{ disk: 'sda' }, one], 'sdb'), one);
  assert.equal(healthOf([one], 'sdx'), one);
  assert.equal(healthOf(one, 'sdb'), one);
  assert.equal(healthOf([], 'sdb'), null);
  assert.equal(healthOf(null, 'sdb'), null);
  assert.deepEqual(attributeRows(one), [['5 Reallocated_Sector_Ct', '100', '100', '10', '8', 'Tells of failure'], ['9 Power_On_Hours', '90', '90', '0', '21804', 'Tells of age']]);
  assert.deepEqual(attributeRows({ attributes: null }), []);
});

// ---- the steps to use or replace a disk ----------------------------------------------------

test('the steps for a mirror that lost a disk are about its mount and the disk that is left', () => {
  assert.deepEqual(stepsOf(showMissing(), null), { mount: '/volumes/data', survivor: 'sda' });
});

test('the steps from a mirror disk\'s page name the other disk of the mirror', () => {
  assert.deepEqual(stepsOf(show(), 'sdb'), { mount: '/volumes/data', survivor: 'sda' });
  assert.deepEqual(stepsOf(show(), 'sda'), { mount: '/volumes/data', survivor: 'sdb' });
});

test('with no mirror open, the steps are about the mirror, else a volume that is not the system', () => {
  assert.deepEqual(stepsOf(show(), null), { mount: '/volumes/data', survivor: 'sda' });
  const spread = show();
  spread.volumes[1] = data({ profiles: { data: ['single'], metadata: ['raid1'], system: ['raid1'] } });
  assert.equal(stepsOf(spread, null).mount, '/volumes/data');
});

test('a machine with one disk has no disk that is left, and a mount to use when no volume is known', () => {
  assert.deepEqual(stepsOf(showAlone(), null), { mount: '/volumes/data', survivor: null });
  assert.deepEqual(stepsOf(showAlone(), 'nvme0'), { mount: '/', survivor: null });
  assert.deepEqual(stepsOf({ disks: [], volumes: [] }, null), { mount: '/volumes/data', survivor: null });
  assert.deepEqual(stepsOf(null, 'sdb'), { mount: '/volumes/data', survivor: null });
});

test('the panel of a disk that is a RAID volume has no health record and no self-test, and says whose it is', () => {
  const answer = show();
  const raid = {
    controller: 'PERC H740P Mini', n: 0, target: 0, name: 'boot', level: 'RAID 1', bays: 'Bays 0 and 1', members: ['Bay 0', 'Bay 1'], failedBays: [], replace: false,
    health: { word: 'Optimal', tone: 'ok', note: 'A RAID 1 volume.' }, line: 'A RAID 1 volume on PERC H740P Mini, made of bays 0 and 1',
  };
  const found = detail(answer, 'sdc', { raid });
  assert.equal(found.hasHealth, false);
  assert.equal(found.raid.replace, false);
  assert.equal(found.line, 'A RAID 1 volume on PERC H740P Mini, made of bays 0 and 1 · 4 TB');
  assert.deepEqual(found.health, raid.health);
  assert.deepEqual(row(found, 'Health'), { label: 'Health', value: 'Optimal: the controller\'s word for the volume', tone: undefined });
  assert.equal(row(found, 'Member disks').value, 'Bays 0 and 1.');
  assert.equal(row(found, 'Last full check'), undefined);
  const failing = detail(answer, 'sdc', { raid: { ...raid, replace: true, failedBays: ['Bay 4'], health: { word: 'Degraded', tone: 'warn', note: '' } } });
  assert.equal(failing.raid.replace, true);
  assert.deepEqual(failing.raid.bays, ['Bay 4']);
  assert.equal(row(failing, 'Health').tone, 'warn');
  assert.equal(detail(answer, 'sdc').raid, null);
  assert.equal(row(detail(answer, 'sdc'), 'Health').value, 'Not checked: this disk does not report its health');
});
