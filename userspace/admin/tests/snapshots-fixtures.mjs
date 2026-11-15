// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Answers of `GET /api/1/backups` (`backup list --json`, design 11.6), `GET
// /api/1/snapshots` (`snap list --json`, design 11.5), `backups.show` and
// `GET /api/1/storage`, for the tests of the Snapshots and backups page.
// Every time is relative to NOW, Saturday Nov 14 2026, 10:12 UTC, so that
// what the page says of a day does not depend on when the test runs. Not a
// test itself, and never installed.

import { makeTime } from '../lib/time.js';

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** Unix milliseconds of a UTC time. */
export const at = (year, month, day, hour = 0, minute = 0) => Date.UTC(year, month - 1, day, hour, minute);

export const NOW = at(2026, 11, 14, 10, 12);

/** The clocks the page draws on: UTC, and now as NOW. */
export const time = makeTime('UTC', () => NOW);

/** The context that the model functions take. */
export const context = { time, now: NOW };

export { DAY, HOUR };

/** A backup place as `backup list --json` gives it, a good one: made this morning, checked three days ago. */
export const place = (name, more = {}) => ({
  name,
  kind: 'ssh',
  to: 'ssh://backup@nas.local/srv/dukovo',
  every: 'day',
  verify: 'week',
  format: 'btrfs',
  keeps: { chains: 3, chain: 7 },
  places: ['/users', '/shared', '/system/settings'],
  running: false,
  overdue_days: 0,
  key_ms: at(2026, 10, 2, 9, 30),
  state: { tried_ms: at(2026, 11, 14, 3, 40), done_ms: at(2026, 11, 14, 3, 40), failed: '', verified_ms: at(2026, 11, 11, 4, 0), verify_failed: '' },
  ...more,
});

/** `state` fields merged into a place's good state. */
export const withState = (name, state, more = {}) => {
  const base = place(name, more);
  return { ...base, state: { ...base.state, ...state } };
};

/** `GET /api/1/backups`. */
export const backupsOf = (...places) => ({ format: 1, places, staged: null });

/** A machine with one good backup place. */
export const good = () => backupsOf(place('nas'));

/** The last backup to nas failed two days after the last good one. */
export const failed = () => backupsOf(withState('nas', {
  tried_ms: at(2026, 11, 14, 3, 40), done_ms: at(2026, 11, 12, 3, 40), failed: 'ssh: connect to host nas.local port 22: No route to host',
}));

/** nas has not been backed up for five days, and is made every day. */
export const overdue = (days = 5) => backupsOf(withState('nas', { done_ms: at(2026, 11, 14 - days, 3, 40) }, { overdue_days: days }));

/** The check by a restore failed. */
export const checkFailed = () => backupsOf(withState('nas', { verify_failed: 'there is not enough room to restore the newest backup' }));

/** A whole-machine restore is staged. */
export const staged = () => ({ ...good(), staged: { from: 'nas', at: '2026-11-14-013000' } });

/** `GET /api/1/snapshots`: the places snap names, and what it keeps. */
export const listOf = (places, settings = {}) => ({
  format: 1,
  places,
  settings: { daily: 7, weekly: 4, places: ['/users', '/shared', '/system/settings'], ...settings },
  update: [],
});

/** A place of `snap list --json` with snapshots, the newest made at `made` (RFC 3339) and named by it, as the daily round names one. */
export const snapPlace = (path, made, more = {}) => ({
  place: path,
  snapshots: 9,
  newest: made ? `${made.slice(0, 10)}-${made.slice(11, 13)}${made.slice(14, 16)}` : null,
  newest_made: made || null,
  folder: `/snapshots/${path.split('/').pop()}`,
  in_service: true,
  ...more,
});

/** Every place has a snapshot from this morning. */
export const current = () => listOf([
  snapPlace('/users', '2026-11-14T03:02:00Z'),
  snapPlace('/shared', '2026-11-14T03:02:00Z'),
  snapPlace('/system/settings', '2026-11-14T03:02:00Z'),
]);

/** Nothing has been snapshotted for four days. */
export const stale = () => listOf([
  snapPlace('/users', '2026-11-10T03:02:00Z'),
  snapPlace('/shared', '2026-11-10T03:02:00Z'),
  snapPlace('/system/settings', '2026-11-10T03:02:00Z'),
]);

/** snap.toml names /shared, and the service does not see it. */
export const skipped = () => listOf([
  snapPlace('/users', '2026-11-14T03:02:00Z'),
  snapPlace('/shared', '2026-11-12T03:02:00Z', { in_service: false }),
  snapPlace('/system/settings', '2026-11-14T03:02:00Z'),
]);

/** `snap list PLACE --json`: a snapshot of a place. */
export const snapshot = (name, made, more = {}) => ({ name, made, named: false, kept: ['daily'], prunable: false, restored: false, ...more });

/** `backups.show`'s answer: the backups each place of a backup place holds, with times in milliseconds. */
export const shown = () => ({
  places: [
    { path: '/shared', backups: [{ time: '2026-11-13-030200', at_ms: at(2026, 11, 13, 3, 2), whole: false }] },
    {
      path: '/users',
      backups: [
        { time: '2026-11-12-030200', at_ms: at(2026, 11, 12, 3, 2), whole: false },
        { time: '2026-11-14-030200', at_ms: at(2026, 11, 14, 3, 2), whole: false },
        { time: '2026-11-07-030200', at_ms: at(2026, 11, 7, 3, 2), whole: true },
        { time: 'not a time', at_ms: 'soon' },
      ],
    },
    { path: '/applications', backups: [] },
    { path: '/system/settings', backups: [{ time: '2026-11-14-030200', at_ms: at(2026, 11, 14, 3, 2), whole: true }] },
  ],
});

/** `GET /api/1/storage` with a USB disk that is mounted and one that is not. */
export const storageWithStick = () => ({
  disks: [
    { name: 'nvme0', removable: false, partitions: [{ name: 'nvme0p1', mounts: ['/'], size: 256e9 }] },
    { name: 'sdc', removable: true, partitions: [{ name: 'sdc1', label: 'BACKUP', mounts: ['/volumes/BACKUP'], size: 4e12 }] },
    { name: 'sdd', removable: true, partitions: [{ name: 'sdd1', label: 'SPARE', mounts: [], size: 1e12 }] },
  ],
  volumes: [{ devices: ['sdc1'], free: 3.9e12 }],
  filesystems: [],
});
