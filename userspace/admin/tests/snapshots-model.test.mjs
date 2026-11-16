// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Snapshots and backups page's plain values (design 5), from fixtures of
// `backup list --json`, `snap list --json` and `backup show --json`: which
// view an address asks for, the banners, What is protected, the backup
// places, what one holds, the tasks that back up, check and restore, the
// disks a backup can go to and what the forms may send.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { mergeSnapshots } from '../lib/snapshot-model.js';

import { heldBackups, detailOf, keepsWords, staysWords } from '../pages/snapshots/detail-model.js';
import {
  addArgs, nameProblem, NAME_MOST, recoveryKeyOf, S3_WORDS, snapshotNameProblem, sshAddress, sshKeyOf, suggestName, takenWords,
} from '../pages/snapshots/form-model.js';
import {
  atWords, backupPlaces, bannersOf, findPlace, hostOf, jobWords, lateOf, makeJobs, placeHref, placeRows, protectedRows,
  removableVolumes, routeOf, scheduledPlaces, skippedPlaces, snapPlaces, snapshotsCurrent, stateOf, whereWords,
} from '../pages/snapshots/model.js';
import {
  at, backupsOf, checkFailed, context, current, failed, good, listOf, NOW, overdue, place, shown, skipped, snapPlace, stale, staged,
  storageWithStick, withState,
} from './snapshots-fixtures.mjs';

// ---- the address ----

test('an address under /snapshots asks for the page, the backup places, one place or a folder', () => {
  assert.deepEqual(routeOf(''), { browse: false, backups: false, open: null });
  assert.deepEqual(routeOf(undefined), { browse: false, backups: false, open: null });
  assert.deepEqual(routeOf('/backups'), { browse: false, backups: true, open: null });
  assert.deepEqual(routeOf('/backups/nas'), { browse: false, backups: true, open: 'nas' });
  assert.deepEqual(routeOf('/backups/my%20nas/'), { browse: false, backups: true, open: 'my nas' });
  assert.deepEqual(routeOf('/browse'), { browse: true, backups: false, open: null });
  assert.deepEqual(routeOf('/something-else'), { browse: false, backups: false, open: null });
  assert.equal(routeOf('/backups/%E0%A4%A').open, '%E0%A4%A', 'a name that does not decode is taken as it is');
  assert.equal(placeHref('my nas'), '/snapshots/backups/my%20nas');
});

// ---- what backup list says of a place ----

test('a place\'s state is its times and texts, and what is missing is null and empty', () => {
  assert.deepEqual(stateOf(place('nas')), {
    tried: at(2026, 11, 14, 3, 40), done: at(2026, 11, 14, 3, 40), failed: '', verified: at(2026, 11, 11, 4, 0), verifyFailed: '',
  });
  assert.deepEqual(stateOf({}), { tried: null, done: null, failed: '', verified: null, verifyFailed: '' });
  assert.deepEqual(stateOf({ state: { done_ms: 0, failed: '  ', verified_ms: 'soon' } }), { tried: null, done: null, failed: '', verified: null, verifyFailed: '' });
  assert.deepEqual(stateOf(null), { tried: null, done: null, failed: '', verified: null, verifyFailed: '' });
});

test('a backup is overdue as the machine counts it, and bad at twice the 3 days a day and the 14 a week it allows', () => {
  assert.equal(lateOf({ every: 'day', overdue_days: 0 }), null);
  assert.equal(lateOf({ every: 'day', overdue_days: null }), null);
  assert.equal(lateOf({ every: 'day' }), null);
  assert.equal(lateOf({ every: 'day', overdue_days: 1.5 }), null);
  assert.deepEqual(lateOf({ every: 'day', overdue_days: 1 }), { days: 1, tone: 'warn' });
  assert.deepEqual(lateOf({ every: 'day', overdue_days: 3 }), { days: 3, tone: 'warn' });
  assert.deepEqual(lateOf({ every: 'day', overdue_days: 5 }), { days: 5, tone: 'warn' });
  assert.deepEqual(lateOf({ every: 'day', overdue_days: 6 }), { days: 6, tone: 'bad' });
  assert.deepEqual(lateOf({ every: 'week', overdue_days: 14 }), { days: 14, tone: 'warn' });
  assert.deepEqual(lateOf({ every: 'week', overdue_days: 28 }), { days: 28, tone: 'bad' });
  assert.deepEqual(lateOf({ every: 'hand', overdue_days: 90 }), { days: 90, tone: 'warn' }, 'a backup by hand has no allowance to double');
});

test('a time is a day and a clock', () => {
  assert.equal(atWords(at(2026, 11, 12, 3, 40), context), 'Nov 12 at 3:40 AM');
  assert.equal(atWords(at(2025, 12, 30, 15, 5), context), 'Dec 30, 2025 at 3:05 PM');
});

test('the snapshots are current when a place in the service made one in the last two days', () => {
  assert.equal(snapshotsCurrent(null, NOW), null);
  assert.equal(snapshotsCurrent(current(), NOW), true);
  assert.equal(snapshotsCurrent(stale(), NOW), false);
  assert.equal(snapshotsCurrent(listOf([]), NOW), false);
  assert.equal(snapshotsCurrent(skipped(), NOW), true, 'a place that the service does not see does not count');
  assert.equal(snapshotsCurrent(listOf([snapPlace('/users', '2026-11-14T03:02:00Z', { in_service: false })]), NOW), false);
});

// ---- what snap list says ----

/** A document `snap list --json` prints, from the golden files its own tests hold it to. */
const golden = (name) => JSON.parse(readFileSync(new URL(`../../programs/systools/tests/json/${name}`, import.meta.url), 'utf8'));

test('the page reads snap list as the command prints it, with each place and each snapshot', () => {
  const all = golden('snap-list.json');
  assert.deepEqual(snapPlaces(all), [
    { place: '/users', newest: Date.parse('2026-11-14T03:00:00Z'), count: 3, inService: true },
    { place: '/volumes/sdb1', newest: null, count: 0, inService: false },
  ]);
  assert.deepEqual(scheduledPlaces(all), ['/users', '/shared']);
  assert.deepEqual(skippedPlaces(all), ['/volumes/sdb1']);
  assert.equal(snapshotsCurrent(all, Date.parse('2026-11-15T03:00:00Z')), true);
  const one = golden('snap-list-place.json');
  const rows = mergeSnapshots([{ place: one.place, snapshots: one.snapshots }], all.update);
  assert.deepEqual(rows.map((row) => row.name), [
    'restored-2026-11-10-030000', '2026-11-14-0300', '2026-11-13-0300', 'before-the-upgrade', 'update', '2026-11-12-0300',
  ], 'newest first, the one with no time left out');
  const named = (name) => rows.find((row) => row.name === name);
  assert.deepEqual([named('restored-2026-11-10-030000').restored, named('before-the-upgrade').restored], [true, false]);
  assert.deepEqual(named('2026-11-14-0300').kept, ['newest', 'daily', 'weekly']);
  assert.equal(named('2026-11-12-0300').prunable, true);
  assert.equal(named('update').ms, Date.parse('2026-11-12T20:13:20Z'));
});

// ---- the banners ----

test('a failed backup is a bad banner with the last good one, what still protects, and the buttons', () => {
  const [banner, ...more] = bannersOf(failed(), { ...context, current: true, disks: true });
  assert.equal(more.length, 0);
  assert.equal(banner.id, 'failed:nas');
  assert.equal(banner.tone, 'bad');
  assert.equal(banner.lead, 'The last backup to nas failed: ssh: connect to host nas.local port 22: No route to host.');
  assert.match(banner.detail, /^The last good one was Nov 12 at 3:40 AM\. Snapshots on this server are current, so files changed since then can still be brought back/);
  assert.deepEqual(banner.buttons.map((button) => [button.id, button.name ?? null]), [['run', 'nas'], ['disk', null], ['alerts', null]]);
});

test('what still protects the files is said only when the snapshots are known', () => {
  const [stale1] = bannersOf(overdue(5), { ...context, current: false });
  assert.match(stale1.detail, /not current either/);
  const [unknown] = bannersOf(overdue(5), { ...context, current: null });
  assert.equal(unknown.detail, 'The last good one was Nov 9 at 3:40 AM.');
});

test('an overdue backup is a warning, and bad at twice the days it is allowed', () => {
  const [late] = bannersOf(overdue(5), context);
  assert.equal(late.id, 'overdue:nas');
  assert.equal(late.tone, 'warn');
  assert.equal(late.lead, 'Backups to nas are 5 days overdue.');
  assert.deepEqual(late.buttons, [{ id: 'run', label: 'Back up now', name: 'nas' }, { id: 'alerts', label: 'Alert settings', href: '/alerts' }], 'a disk is offered only when one is plugged in');
  assert.equal(bannersOf(overdue(6), context)[0].tone, 'bad');
  assert.equal(bannersOf(backupsOf(place('nas', { overdue_days: null })), context).length, 0);
});

test('a place that has never been backed up says so', () => {
  const never = backupsOf(withState('nas', { done_ms: 0 }, { overdue_days: 4 }));
  assert.match(bannersOf(never, context)[0].detail, /^Nothing has been backed up there yet\./);
});

test('a check by a restore that failed is a warning with Check now, and says that the backups may still be good', () => {
  const [banner] = bannersOf(checkFailed(), context);
  assert.equal(banner.id, 'check:nas');
  assert.equal(banner.tone, 'warn');
  assert.equal(banner.lead, 'Restoring nas\'s newest backups to check them failed: there is not enough room to restore the newest backup.');
  assert.match(banner.detail, /may still be good/);
  assert.deepEqual(banner.buttons, [{ id: 'verify', label: 'Check now', name: 'nas' }]);
});

test('a whole-machine restore that waits for the next start is a banner that sends the person to Move', () => {
  const [banner] = bannersOf(staged(), context);
  assert.equal(banner.id, 'staged');
  assert.equal(banner.lead, 'A whole-machine restore from nas is staged for the next start.');
  assert.deepEqual(banner.buttons, [{ id: 'move', label: 'Open Move to new hardware', href: '/move' }]);
});

test('the worst banner is first, and those as bad are in the order the places come', () => {
  const both = backupsOf(
    withState('usb', { verify_failed: 'the disk is full' }),
    withState('nas', { failed: 'no route' }),
    withState('cold', { done_ms: at(2026, 11, 1) }, { overdue_days: 13 }),
  );
  both.staged = { from: 'nas' };
  assert.deepEqual(bannersOf(both, context).map((banner) => [banner.id, banner.tone]), [
    ['failed:nas', 'bad'], ['overdue:cold', 'bad'], ['check:usb', 'warn'], ['staged', 'warn'],
  ]);
});

test('a machine in good order, and one that could not be read, have no banner', () => {
  assert.deepEqual(bannersOf(good(), context), []);
  assert.deepEqual(bannersOf(null, context), []);
  assert.deepEqual(bannersOf({}, context), []);
  assert.deepEqual(bannersOf({ places: 'none' }, context), []);
});

// ---- What is protected ----

test('every place that is snapshotted and every place a backup holds is a row, with its copies and its check', () => {
  const rows = protectedRows(current(), good(), context);
  assert.deepEqual(rows.map((row) => [row.path, row.name]), [
    ['/users', 'People\'s files'], ['/shared', 'Shared folders and websites'], ['/system/settings', 'Settings'],
  ]);
  assert.deepEqual(rows[0].snapshots, { words: 'Daily · 3:02 AM', tone: 'ok' });
  assert.deepEqual(rows[0].copies, [{ name: 'nas', rest: 'Nov 14', tone: 'ok' }]);
  assert.deepEqual(rows[0].check, { words: 'Nov 11, passed', tone: 'ok', name: 'nas' });
});

test('a place that only a backup holds is a row with no snapshots, and one nothing copies says so', () => {
  const backups = backupsOf(place('nas', { places: ['/users', '/applications'] }));
  const rows = protectedRows(current(), backups, context);
  assert.deepEqual(rows.map((row) => row.path), ['/users', '/shared', '/system/settings', '/applications']);
  assert.deepEqual(rows[3].snapshots, { words: 'None', tone: 'warn' });
  assert.deepEqual(rows[3].copies.map((copy) => copy.name), ['nas']);
  assert.deepEqual(rows[1].copies, []);
  assert.deepEqual(rows[1].check, { words: 'No copy to check', tone: '', name: null });
});

test('a place that snap names and the service does not see is Not made, and is listed as skipped', () => {
  const rows = protectedRows(skipped(), backupsOf(), context);
  assert.deepEqual(rows[1].snapshots, { words: 'Not made', tone: 'warn' });
  assert.deepEqual(skippedPlaces(skipped()), ['/shared']);
  assert.deepEqual(skippedPlaces(current()), []);
  assert.deepEqual(skippedPlaces(null), []);
});

test('a place that waits for its first snapshot says that one is made within the hour', () => {
  const none = listOf([snapPlace('/users', '', { snapshots: 0 })], { places: ['/users'] });
  assert.deepEqual(protectedRows(none, backupsOf(), context)[0].snapshots, { words: 'None yet: the first is made within the hour', tone: '' });
});

test('a copy says when it was made, and that it failed or is overdue', () => {
  assert.deepEqual(protectedRows(current(), failed(), context)[0].copies, [{ name: 'nas', rest: 'Nov 12, failed', tone: 'bad' }]);
  assert.deepEqual(protectedRows(current(), overdue(5), context)[0].copies, [{ name: 'nas', rest: 'Nov 9, 5 days overdue', tone: 'warn' }]);
  const waiting = backupsOf(withState('nas', { done_ms: 0 }));
  assert.deepEqual(protectedRows(current(), waiting, context)[0].copies, [{ name: 'nas', rest: 'not yet', tone: 'warn' }]);
  const byHand = backupsOf(withState('usb', { done_ms: 0 }, { every: 'hand' }));
  assert.deepEqual(protectedRows(current(), byHand, context)[0].copies, [{ name: 'usb', rest: 'not yet', tone: '' }], 'a backup by hand that has not been made is not a warning');
});

test('the last check is the newest that passed, Never, or failed with the place that Check now would check', () => {
  const two = backupsOf(
    place('nas', { state: { ...place('nas').state, verified_ms: at(2026, 11, 8, 4) } }),
    place('usb', { state: { ...place('usb').state, verified_ms: at(2026, 11, 12, 4) } }),
  );
  assert.deepEqual(protectedRows(current(), two, context)[0].check, { words: 'Nov 12, passed', tone: 'ok', name: 'usb' });
  const never = backupsOf(withState('nas', { verified_ms: 0 }));
  assert.deepEqual(protectedRows(current(), never, context)[0].check, { words: 'Never', tone: '', name: 'nas' });
  assert.deepEqual(protectedRows(current(), checkFailed(), context)[0].check, { words: 'failed', tone: 'warn', name: 'nas' });
});

// ---- the backup places ----

test('a place is told by where it is, how often, the last good backup and how it is now', () => {
  assert.deepEqual(placeRows(good(), context), [{
    name: 'nas', href: '/snapshots/backups/nas', where: 'SSH to nas.local · encrypted', every: 'day', last: 'Nov 14', now: { words: 'OK', tone: 'ok' }, job: null,
  }]);
  assert.deepEqual(placeRows(failed(), context)[0].now, { words: 'failed', tone: 'bad' });
  assert.deepEqual(placeRows(overdue(5), context)[0].now, { words: '5 days overdue', tone: 'warn' });
  assert.deepEqual(placeRows(overdue(9), context)[0].now, { words: '9 days overdue', tone: 'bad' });
  assert.deepEqual(placeRows(checkFailed(), context)[0].now, { words: 'check failed', tone: 'warn' });
  const fresh = placeRows(backupsOf(withState('nas', { done_ms: 0 })), context)[0];
  assert.deepEqual([fresh.last, fresh.now], ['Never', { words: 'Waiting for the first backup', tone: '' }]);
  assert.deepEqual(placeRows(backupsOf(withState('usb', { done_ms: 0 }, { every: 'hand' })), context)[0].now, { words: 'Not yet', tone: '' });
  assert.equal(placeRows(backupsOf(place('usb', { every: 'hand' })), context)[0].every, 'by hand');
  assert.deepEqual(placeRows(null, context), []);
});

test('a place that runs says which task, from the page\'s jobs, or a backup when only the machine says it runs', () => {
  assert.equal(placeRows(backupsOf(place('nas', { running: true })), context)[0].job, 'run');
  const jobs = { of: (name) => (name === 'nas' ? { kind: 'verify' } : null) };
  assert.equal(placeRows(backupsOf(place('nas', { running: true })), { ...context, jobs })[0].job, 'verify');
  assert.equal(placeRows(good(), { ...context, jobs })[0].job, 'verify');
});

test('a place with no name is not a place', () => {
  const odd = { places: [{ kind: 'ssh' }, null, place('nas')] };
  assert.deepEqual(backupPlaces(odd).map((one) => one.name), ['nas']);
  assert.equal(findPlace(odd, 'nas').name, 'nas');
  assert.equal(findPlace(odd, 'gone'), null);
});

test('where a backup goes is told by its kind: a server, a bucket, a plugged-in disk or a folder', () => {
  assert.equal(whereWords({ kind: 'ssh', to: 'ssh://backup@nas.local:2222/srv' }), 'SSH to nas.local · encrypted');
  assert.equal(whereWords({ kind: 'ssh', to: 'ssh://backup@[fe80::1]:22/srv' }), 'SSH to fe80::1 · encrypted');
  assert.equal(whereWords({ kind: 's3', to: 's3://s3.example.com/my-bucket/prefix' }), 'S3 bucket my-bucket · encrypted');
  assert.equal(whereWords({ kind: 's3', to: 's3:' }), 'S3 storage · encrypted');
  assert.equal(whereWords({ kind: 'folder', to: '/volumes/BACKUP/dukovo-backup', plugged_in: 'BACKUP' }), 'USB disk BACKUP · encrypted');
  assert.equal(whereWords({ kind: 'folder', to: '/volumes/Stick/x', plugged_in: true }), 'USB disk Stick · encrypted');
  assert.equal(whereWords({ kind: 'folder', to: '/mnt/x', plugged_in: true }), 'USB disk · encrypted');
  assert.equal(whereWords({ kind: 'folder', to: '/srv/backups' }), 'Folder /srv/backups · encrypted');
  assert.equal(hostOf('backup@nas.local:/srv/x'), 'nas.local');
  assert.equal(hostOf('nas.local'), 'nas.local');
});

// ---- the opened place ----

test('what a place keeps is told by its format', () => {
  assert.equal(keepsWords({ format: 'btrfs', keeps: { chains: 3, chain: 7 } }), '3 chains of up to 7 backups');
  assert.equal(keepsWords({ format: 'btrfs', keeps: { chains: 1, chain: 1 } }), '1 chain of up to 1 backup');
  assert.equal(keepsWords({ format: 'btrfs' }), '');
  assert.equal(keepsWords({ format: 'restic', keeps: { daily: 7, weekly: 4, monthly: 12 } }), '7 daily, 4 weekly, 12 monthly');
  assert.equal(keepsWords({ format: 'restic', keeps: { daily: 7 } }), '7 daily');
  assert.equal(keepsWords({ format: 'restic', keeps: {} }), 'Only the newest backup');
});

test('an opened place says what it holds and keeps, how its last check went and when its key was shown', () => {
  assert.deepEqual(detailOf(place('nas'), context), {
    holds: '/users, /shared and settings',
    keeps: '3 chains of up to 7 backups',
    check: { words: 'Nov 11: restored the newest backups and compared them. Passed. Checked every week.', tone: 'ok' },
    key: { words: 'Shown once on Oct 2', tone: '' },
  });
});

test('a check that failed, never ran or is off is told as it is, and a key that was not found is a warning', () => {
  const bad = detailOf(withState('nas', { verify_failed: 'the disk is full' }), context);
  assert.deepEqual(bad.check, { words: 'Failed: the disk is full. Checked every week.', tone: 'warn' });
  const never = detailOf(withState('nas', { verified_ms: 0 }, { verify: 'month' }), context);
  assert.deepEqual(never.check, { words: 'Never checked. Checked every month.', tone: '' });
  assert.equal(detailOf(place('nas', { verify: 'never' }), context).check.words.endsWith('Not checked unless someone asks.'), true);
  assert.deepEqual(detailOf(place('nas', { key_ms: 0 }), context).key, { words: 'No recovery key was found for it.', tone: 'warn' });
});

test('the backups a place holds are listed newest first, by place in the page\'s order, without those that do not read', () => {
  const held = heldBackups(shown());
  assert.deepEqual(held.map((one) => [one.path, one.name, one.backups.length]), [
    ['/users', 'People\'s files', 3], ['/shared', 'Shared folders and websites', 1], ['/system/settings', 'Settings', 1],
  ]);
  assert.deepEqual(held[0].backups.map((backup) => backup.time), ['2026-11-14-030200', '2026-11-12-030200', '2026-11-07-030200']);
  assert.deepEqual(held[0].backups.map((backup) => backup.whole), [false, false, true]);
  assert.deepEqual(heldBackups(null), []);
  assert.deepEqual(heldBackups({ places: [{ backups: [] }, null] }), []);
});

test('the words for where a removed place\'s backups stay follow its kind', () => {
  assert.equal(staysWords({ kind: 'ssh', to: 'ssh://backup@nas.local/srv' }), 'at nas.local');
  assert.equal(staysWords({ kind: 's3', to: 's3://s3.example.com/my-bucket' }), 'in the bucket my-bucket');
  assert.equal(staysWords({ kind: 'folder', to: '/volumes/BACKUP/x', plugged_in: 'BACKUP' }), 'on USB disk BACKUP');
  assert.equal(staysWords({ kind: 'folder', to: '/srv/backups' }), 'in /srv/backups');
});

// ---- the disks a backup can go to ----

test('a mounted removable disk that no place uses can take a backup, given as the volume that backup add makes dukovo-backup in', () => {
  assert.deepEqual(removableVolumes(storageWithStick(), good()), [{
    mount: '/volumes/BACKUP', label: 'BACKUP', disk: 'sdc', free: 3.9e12, size: 4e12, to: '/volumes/BACKUP',
  }]);
});

test('a disk that a place already uses, one that is not mounted and an answer that is not read give none', () => {
  const used = backupsOf(place('usb', { to: '/volumes/BACKUP' }));
  assert.deepEqual(removableVolumes(storageWithStick(), used), []);
  // A place added at a folder of the volume uses it too.
  assert.deepEqual(removableVolumes(storageWithStick(), backupsOf(place('usb', { to: '/volumes/BACKUP/backups' }))), []);
  assert.deepEqual(removableVolumes(null), []);
  assert.deepEqual(removableVolumes({ disks: 'none' }, good()), []);
});

// ---- the tasks that back up, check and restore ----

const task = (id, action, name, more = {}) => ({ id, action, state: 'running', args: { name }, ...more });

test('the jobs are the running backups, checks and restores, and nothing else that runs', () => {
  const jobs = makeJobs();
  jobs.learn([
    task('t1', 'backups.run', 'nas', { percent: 41 }),
    task('t2', 'backups.verify', 'usb', { state: 'waiting' }),
    task('t3', 'services.restart', 'nginx'),
    task('t4', 'backups.run', 'old', { state: 'done' }),
  ]);
  assert.deepEqual(jobs.all().map((job) => [job.id, job.kind, job.name]), [['t1', 'run', 'nas'], ['t2', 'verify', 'usb']]);
  assert.equal(jobs.of('nas').percent, 41);
  assert.equal(jobs.of('nobody'), null);
  jobs.learn('nothing');
  assert.deepEqual(jobs.all(), []);
});

test('a job says how far it is as a percent, as a step, or only that it runs', () => {
  const jobs = makeJobs();
  jobs.learn([
    task('t1', 'backups.run', 'nas', { percent: 41 }),
    task('t2', 'backups.verify', 'usb', { step: 2, steps: 3 }),
    task('t3', 'backups.restore', 'far'),
    task('t4', 'backups.run', 'once', { step: 1, steps: 1 }),
    task('t5', 'backups.run', 'over', { percent: 140 }),
    task('t6', 'backups.run', 'under', { percent: -5 }),
    task('t7', 'backups.run', 'half', { percent: 41.5 }),
  ]);
  assert.deepEqual(['nas', 'usb', 'far', 'once', 'over', 'under', 'half'].map((name) => jobWords(jobs.of(name))), [
    'Backing up… 41%', 'Checking… step 2 of 3', 'Restoring…', 'Backing up…', 'Backing up… 100%', 'Backing up… 0%', 'Backing up…',
  ]);
  assert.equal(jobWords({ kind: 'other', percent: null, step: null }), 'Working…');
});

test('a job that this page began is its own, through a read of the tasks, and an end says which', () => {
  const jobs = makeJobs();
  jobs.started('t9', 'backups.restore', { name: 'nas', place: '/users', at: '2026-11-14-030200' });
  assert.equal(jobs.of('nas').mine, true);
  jobs.learn([task('t9', 'backups.restore', 'nas', { args: { name: 'nas', place: '/users', at: '2026-11-14-030200' } }), task('t8', 'backups.run', 'usb')]);
  assert.deepEqual(jobs.all().map((job) => [job.id, job.mine]), [['t9', true], ['t8', false]]);
  assert.deepEqual(jobs.event({ id: 't9', action: 'backups.restore', state: 'running', percent: 50 }), {});
  assert.equal(jobs.of('nas').percent, 50);
  const ended = jobs.event({ id: 't9', action: 'backups.restore', state: 'done' });
  assert.equal(ended.state, 'done');
  assert.equal(ended.ended.mine, true);
  assert.deepEqual(ended.ended.args, { name: 'nas', place: '/users', at: '2026-11-14-030200' });
  assert.equal(jobs.of('nas'), null);
});

test('an event of a task the page does not know is read whole, and one of another action is not the page\'s', () => {
  const jobs = makeJobs();
  assert.deepEqual(jobs.event({ id: 't5', action: 'backups.run', state: 'running' }), { learn: 't5' });
  assert.deepEqual(jobs.event({ id: 't5', action: 'backups.run', state: 'done' }), {}, 'an end of one it never knew changes nothing');
  assert.deepEqual(jobs.event({ id: 't6', action: 'services.restart', state: 'running' }), {});
  assert.deepEqual(jobs.event({}), {});
  assert.deepEqual(jobs.event(null), {});
  jobs.put(task('t5', 'backups.run', 'nas'));
  assert.equal(jobs.of('nas').id, 't5');
  assert.deepEqual(jobs.event({ id: 't5', state: 'failed' }).state, 'failed');
});

// ---- what Add a backup place may send ----

test('a name is letters, digits, - and _, at most 32, not started with -, and not in use', () => {
  assert.equal(nameProblem(''), 'Give it a name');
  assert.equal(nameProblem('a'.repeat(NAME_MOST + 1)), `Use at most ${NAME_MOST} characters`);
  assert.equal(nameProblem('a'.repeat(NAME_MOST)), '');
  assert.equal(nameProblem('my nas'), 'Use letters, digits, - and _ only');
  assert.equal(nameProblem('-nas'), 'The name cannot start with -');
  assert.equal(nameProblem('nas', ['nas']), 'A backup place is already called nas');
  assert.equal(nameProblem('nas_2-b', ['usb']), '');
});

test('a name is suggested from the disk\'s label or the server\'s name, and never from an address', () => {
  assert.equal(suggestName({ where: 'disk', volume: { label: 'BACKUP' } }), 'usb-BACKUP');
  assert.equal(suggestName({ where: 'disk', volume: { label: 'My Disk!' } }), 'usb-My-Disk');
  assert.equal(suggestName({ where: 'disk', volume: null }), '');
  assert.equal(suggestName({ where: 'ssh', address: 'backup@nas.local:/srv' }), 'nas');
  assert.equal(suggestName({ where: 'ssh', address: ' ssh://backup@Nas-One.example.org/srv ' }), 'Nas-One');
  assert.equal(suggestName({ where: 'ssh', address: 'backup@192.168.1.20:/srv' }), '');
  assert.equal(suggestName({ where: 'ssh', address: 'backup@[fe80::1]:/srv' }), '');
  assert.equal(suggestName({ where: 'ssh', address: '' }), '');
  assert.equal(suggestName({ where: 's3' }), '');
});

test('an SSH server is typed as user@host:/folder or as an ssh:// address, and is sent as the address', () => {
  assert.deepEqual(sshAddress('backup@192.168.1.20:/srv/dukovo'), { to: 'ssh://backup@192.168.1.20/srv/dukovo' });
  assert.deepEqual(sshAddress(' backup@nas.local:~/dukovo '), { to: 'ssh://backup@nas.local/~/dukovo' });
  assert.deepEqual(sshAddress('backup@[fe80::1]:/srv'), { to: 'ssh://backup@[fe80::1]/srv' });
  assert.deepEqual(sshAddress('ssh://backup@nas.local:2222/srv'), { to: 'ssh://backup@nas.local:2222/srv' });
  assert.match(sshAddress('nas.local').error, /user@host:\/folder/);
  assert.match(sshAddress('ssh://nas.local/srv').error, /ssh:\/\/user@host:2222\/folder/);
  assert.match(sshAddress('backup@nas.local').error, /user@host:\/folder/);
  assert.match(sshAddress('').error, /user@host:\/folder/);
});

test('Add a backup place sends a name, an address, how often and how to check, and the format only when it is not the default', () => {
  const volume = { to: '/volumes/BACKUP' };
  const base = { where: 'disk', volume, address: '', name: ' usb-BACKUP ', every: 'day', verify: 'week', format: 'btrfs' };
  assert.deepEqual(addArgs(base), { args: { name: 'usb-BACKUP', to: '/volumes/BACKUP', every: 'day', verify: 'week' } });
  assert.deepEqual(addArgs({ ...base, format: 'restic', every: 'hand', verify: 'never' }).args, {
    name: 'usb-BACKUP', to: '/volumes/BACKUP', every: 'hand', verify: 'never', format: 'restic',
  });
  assert.deepEqual(addArgs({ ...base, where: 'ssh', address: 'backup@nas.local:/srv', name: 'nas' }).args.to, 'ssh://backup@nas.local/srv');
});

test('what is wrong with the form is told under its field, and S3 is told to be added at a terminal', () => {
  const base = { where: 'disk', volume: null, address: '', name: 'x', every: 'day', verify: 'week', format: 'btrfs' };
  assert.deepEqual(addArgs(base), { errors: { to: 'Choose a disk' } });
  assert.deepEqual(addArgs({ ...base, name: '', where: 'ssh', address: 'nas' }).errors.name, 'Give it a name');
  assert.match(addArgs({ ...base, where: 'ssh', address: 'nas' }).errors.to, /user@host/);
  assert.deepEqual(addArgs({ ...base, where: 's3' }), { errors: { to: S3_WORDS } });
  assert.equal(addArgs({ ...base, where: 'ssh', address: 'backup@nas.local:/srv', name: 'nas' }, ['nas']).errors.name, 'A backup place is already called nas');
});

// ---- the keys that an answer holds ----

const AGE = 'AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ';
const SSH = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEXAMPLEEXAMPLEEXAMPLE root@dukovo';

test('the recovery key is found in whatever shape the answer has it', () => {
  assert.equal(recoveryKeyOf(AGE), AGE);
  assert.equal(recoveryKeyOf({ key: AGE }), AGE);
  assert.equal(recoveryKeyOf({ stdout: `# created: 2026-11-14\n${AGE}\n` }), AGE);
  assert.equal(recoveryKeyOf({ lines: ['hello', { text: AGE }] }), AGE);
  assert.equal(recoveryKeyOf({ key: 'not a key' }), null);
  assert.equal(recoveryKeyOf(null), null);
  assert.equal(recoveryKeyOf(42), null);
  assert.equal(recoveryKeyOf({ a: { b: { c: { d: { e: AGE } } } } }), null, 'a key deeper than an answer goes is not looked for');
});

test('the SSH key is found in whatever shape the answer has it', () => {
  assert.equal(sshKeyOf(SSH), SSH);
  assert.equal(sshKeyOf({ public_key: `${SSH}\n` }), SSH);
  assert.equal(sshKeyOf({ stdout: `Your key is\n${SSH}\n` }), SSH);
  assert.equal(sshKeyOf({ key: 'nothing' }), null);
  assert.equal(sshKeyOf(undefined), null);
});

// ---- Take a snapshot now ----

test('a snapshot\'s name is optional, has no slash, no leading dot and is not a time', () => {
  assert.equal(snapshotNameProblem(''), '');
  assert.equal(snapshotNameProblem('before-the-move'), '');
  assert.equal(snapshotNameProblem('a'.repeat(255)), '');
  assert.equal(snapshotNameProblem('a'.repeat(256)), 'Use at most 255 characters');
  assert.equal(snapshotNameProblem('.hidden'), 'The name cannot start with a dot');
  assert.equal(snapshotNameProblem('a/b'), 'The name cannot hold a /');
  assert.equal(snapshotNameProblem('a\0b'), 'The name cannot hold a NUL character');
  assert.equal(snapshotNameProblem('2026-11-14-0302'), 'A name like that is a time, which snap gives itself');
  assert.equal(snapshotNameProblem('2026-11-14-0302.2'), 'A name like that is a time, which snap gives itself');
  assert.equal(snapshotNameProblem('2026-11-14-0302.1'), '', 'snap does not make a .1');
  assert.equal(snapshotNameProblem('2026-11-14'), '');
});

test('the results of Take a snapshot now say which places were made and which were not', () => {
  assert.deepEqual(takenWords([{ place: '/users', ok: true }, { place: '/shared', ok: true }]), ['Made a snapshot of /users and /shared.']);
  assert.deepEqual(takenWords([{ place: '/users', ok: true }, { place: '/system/settings', ok: false }]), [
    'Made a snapshot of /users.', 'No snapshot was made of settings.',
  ]);
  assert.deepEqual(takenWords([{ place: '/users', ok: false }]), ['No snapshot was made of /users.']);
  assert.deepEqual(takenWords([]), []);
});
