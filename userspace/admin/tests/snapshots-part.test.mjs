// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Snapshots and backups page's part of the Overview (design 5.11), from
// fixtures of `GET /api/1/backups` and `GET /api/1/snapshots`: what it gives
// the list of things to look at, what it gives when one of the two cannot be
// read, and the step "Copy your data off this server" that Finish setting up
// and /setup show.

import assert from 'node:assert/strict';
import test from 'node:test';

import { readSteps } from '../pages/overview/steps-read.js';
import { MUST_COVER, hasGoodBackup, needsOf, stepSummary } from '../pages/snapshots/overview-model.js';
import { needs, steps } from '../pages/snapshots/overview.js';
import { fakeCx } from './overview-fixtures.mjs';
import {
  at, backupsOf, checkFailed, context, current, failed, good, listOf, overdue, place, skipped, snapPlace, stale, withState,
} from './snapshots-fixtures.mjs';

const ids = (found) => found.map((one) => one.id);

test('a backup that failed is a warning with the reason and the last good one, and covers its own alert', () => {
  const [need, ...more] = needsOf(failed(), current(), context);
  assert.equal(more.length, 0);
  assert.equal(need.id, 'backup-failed-nas');
  assert.equal(need.severity, 'warn');
  assert.equal(need.title, 'The last backup to nas failed');
  assert.equal(need.detail, 'ssh: connect to host nas.local port 22: No route to host. The last good one was Nov 12 at 3:40 AM.');
  assert.equal(need.href, '/snapshots/backups/nas');
  assert.deepEqual(need.covers, ['backup-failed:nas']);
  assert.equal(need.at, Math.floor(at(2026, 11, 14, 3, 40) / 1000), 'it is dated by the last try');
});

test('an overdue backup is a warning, and bad at twice the days it is allowed', () => {
  const [late] = needsOf(overdue(5), current(), context);
  assert.equal(late.id, 'backup-overdue-nas');
  assert.equal(late.severity, 'warn');
  assert.equal(late.title, 'Backups to nas are 5 days overdue');
  assert.equal(late.detail, 'The last good one was Nov 9 at 3:40 AM. Open it to back up now.');
  assert.deepEqual(late.covers, ['backup-overdue:nas']);
  assert.equal(needsOf(overdue(6), current(), context)[0].severity, 'bad');
});

test('a place that was never backed up says so, and without the clocks the time is left out', () => {
  const never = backupsOf(withState('nas', { done_ms: 0 }, { overdue_days: 4 }));
  assert.equal(needsOf(never, current(), context)[0].detail, 'Nothing has been backed up there yet. Open it to back up now.');
  assert.equal(needsOf(overdue(5), current(), { now: context.now })[0].detail, 'Open it to back up now.');
});

test('a check by a restore that failed is a warning that sends the person to the place', () => {
  const [need] = needsOf(checkFailed(), current(), context);
  assert.equal(need.id, 'backup-check-nas');
  assert.equal(need.severity, 'warn');
  assert.equal(need.title, 'Checking nas\'s backups by restoring them failed');
  assert.equal(need.detail, 'there is not enough room to restore the newest backup.');
});

test('a folder that the schedule does not snapshot is a need, and covers the check that says the same', () => {
  assert.deepEqual(MUST_COVER, ['/users', '/shared']);
  const found = needsOf(good(), skipped(), context);
  assert.deepEqual(ids(found), ['snapshots-none-/shared']);
  assert.equal(found[0].title, 'No snapshots of /shared');
  assert.equal(found[0].severity, 'warn');
  assert.equal(found[0].href, '/snapshots');
  assert.deepEqual(found[0].covers, ['SEC-160']);
  const named = listOf([snapPlace('/users', '2026-11-14T03:02:00Z')], { places: ['/users'] });
  assert.deepEqual(ids(needsOf(good(), named, context)), ['snapshots-none-/shared']);
});

test('snapshots that are old but scheduled are not a need: the check is of the schedule', () => {
  assert.deepEqual(needsOf(good(), stale(), context), []);
});

test('a machine in good order, and an answer that was not read, give nothing', () => {
  assert.deepEqual(needsOf(good(), current(), context), []);
  assert.deepEqual(needsOf(null, null, context), []);
  assert.deepEqual(needsOf(good(), null, context), []);
  assert.deepEqual(needsOf(null, skipped(), context).map((one) => one.id), ['snapshots-none-/shared']);
});

test('every need has what the Overview needs of one', () => {
  const all = [
    ...needsOf(failed(), skipped(), context),
    ...needsOf(overdue(9), current(), context),
    ...needsOf(checkFailed(), current(), context),
  ];
  assert.ok(all.length >= 4);
  for (const need of all) {
    assert.equal(typeof need.id, 'string');
    assert.ok(['bad', 'warn', 'info'].includes(need.severity));
    assert.ok(need.title.length > 0 && need.detail.length > 0);
    assert.ok(need.href.startsWith('/snapshots'));
    assert.equal(need.icon, 'snapshots');
    assert.ok(Array.isArray(need.covers) && need.covers.length > 0);
  }
  assert.equal(new Set(ids(all)).size, ids(all).length, 'one id for one thing');
});

test('the part reads the backup places and the snapshots, once each', async () => {
  const cx = fakeCx({ '/api/1/backups': failed(), '/api/1/snapshots': skipped() });
  const found = await needs(cx);
  assert.deepEqual(ids(found), ['backup-failed-nas', 'snapshots-none-/shared']);
  assert.deepEqual(cx.calls.sort(), ['/api/1/backups', '/api/1/snapshots']);
  await needs(cx);
  assert.equal(cx.calls.length, 2, 'the cache answers the second read');
});

test('one read that fails leaves the needs of the other, and both failing leaves the part out', async () => {
  assert.deepEqual(ids(await needs(fakeCx({ '/api/1/backups': failed() }))), ['backup-failed-nas']);
  assert.deepEqual(ids(await needs(fakeCx({ '/api/1/snapshots': skipped() }))), ['snapshots-none-/shared']);
  await assert.rejects(needs(fakeCx({})), /no answer/);
});

// ---- the step ----

test('Copy your data off this server is a step on every machine', async () => {
  assert.deepEqual(steps.map((one) => one.id), ['move', 'backups'], 'the other step is Move\'s, which the machine\'s state offers');
  const step = steps.find((one) => one.id === 'backups');
  assert.equal(step.id, 'backups');
  assert.equal(step.title, 'Copy your data off this server');
  assert.equal(step.action, 'backups.add');
  assert.equal(step.order, 30);
  assert.equal(step.points, 8);
  assert.equal(step.available, undefined, 'a step with no `available` is on every machine');
  assert.equal(typeof step.form, 'function');
  const read = await readSteps({ parts: async (name, call) => [{ value: await call({ steps }, fakeCx({ '/api/1/backups': good() })) }] });
  assert.deepEqual(read.map((one) => [one.id, one.done, one.summary]), [['backups', true, 'nas backs up every day']], 'a machine that has a backup place is not offered Move');
});

test('the step is done when a place has a good backup and the last try did not fail', async () => {
  const step = steps.find((one) => one.id === 'backups');
  assert.equal(await step.done(fakeCx({ '/api/1/backups': good() })), true);
  assert.equal(await step.done(fakeCx({ '/api/1/backups': failed() })), false);
  assert.equal(await step.done(fakeCx({ '/api/1/backups': backupsOf() })), false);
  assert.equal(await step.done(fakeCx({ '/api/1/backups': backupsOf(withState('nas', { done_ms: 0 })) })), false);
  await assert.rejects(step.done(fakeCx({})), /no answer/);
  assert.equal(hasGoodBackup(null), false);
});

test('the step says where it stands', () => {
  assert.equal(stepSummary(backupsOf()), 'Nothing is copied off this server yet');
  assert.equal(stepSummary(null), 'Nothing is copied off this server yet');
  assert.equal(stepSummary(backupsOf(withState('nas', { done_ms: 0 }))), 'nas is added, and has no good backup yet');
  assert.equal(stepSummary(good()), 'nas backs up every day');
  assert.equal(stepSummary(backupsOf(place('nas', { every: 'week' }))), 'nas backs up every week');
  assert.equal(stepSummary(backupsOf(place('usb', { every: 'hand' }))), 'usb is backed up by hand');
  assert.equal(stepSummary(failed()), 'nas is added, and has no good backup yet');
});
