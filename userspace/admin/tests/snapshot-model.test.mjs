// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/snapshot-model.js, lib/snapshot-words.js and lib/snapshot-folder.js
// (design 3.5, 5.5 and 5.6): the snapshots of every place as one list by day
// in the person's zone, the words of the rule that keeps them, the label of a
// restored file across a change of summer time, a snapshot's time in a
// heading, the address of its folder, the rows of that folder and the name
// that a restored copy gets.

import assert from 'node:assert/strict';
import test from 'node:test';

import { besideName, RESTORE_MOST, rowsOf, trail, within } from '../lib/snapshot-folder.js';
import {
  byDay, earlierWords, inOrder, keptWords, mergeSnapshots, oldestWords, placeName, placeShort,
  placesWords, restoredWords, whyKept,
} from '../lib/snapshot-model.js';
import { makeTime } from '../lib/time.js';
import {
  beside, browseHref, dayWords, whenWords,
} from '../lib/snapshot-words.js';

/** Unix milliseconds of a UTC time. */
const at = (year, month, day, hour = 0, minute = 0) => Date.UTC(year, month - 1, day, hour, minute);

// Saturday Nov 14 2026, 10:12 UTC.
const NOW = at(2026, 11, 14, 10, 12);

/** A snapshot as `snap list PLACE --json` gives it. */
const snapshot = (name, made, more = {}) => ({ name, made, named: false, kept: ['daily'], prunable: false, restored: false, ...more });

const names = (rows) => rows.map((row) => row.name);

test('a place has the name a person knows it by, and the others have their path', () => {
  assert.equal(placeName('/users'), 'People\'s files');
  assert.equal(placeName('/shared'), 'Shared folders and websites');
  assert.equal(placeName('/system/settings'), 'Settings');
  assert.equal(placeName('/applications'), 'Apps and their data');
  assert.equal(placeName('/volumes/data/@photos'), '/volumes/data/@photos');
  assert.equal(placeShort('/system/settings'), 'settings');
  assert.equal(placeShort('/volumes/data/@photos'), '/volumes/data/@photos');
});

test('places are told in a sentence, and listed with the known ones first, once each', () => {
  assert.equal(placesWords(['/users', '/shared', '/system/settings']), '/users, /shared and settings');
  assert.equal(placesWords(['/users']), '/users');
  assert.equal(placesWords([]), '');
  assert.deepEqual(inOrder(['/volumes/z', '/applications', '/users', '/volumes/a', '/system/settings', '/users', '/shared']),
    ['/users', '/shared', '/system/settings', '/applications', '/volumes/a', '/volumes/z']);
});

test('a folder is within a place by whole names', () => {
  assert.equal(within('/users', '/users'), true);
  assert.equal(within('/users', '/users/alice/photos'), true);
  assert.equal(within('/users', '/users2'), false);
  assert.equal(within('/users', '/shared'), false);
});

test('the rule that keeps snapshots is said in words that follow "Snapshots"', () => {
  assert.equal(keptWords({ daily: 7, weekly: 4 }), 'kept daily for 7 days and weekly for 4 weeks');
  assert.equal(keptWords({ daily: 1, weekly: 1 }), 'kept daily for 1 day and weekly for 1 week');
  assert.equal(keptWords({ daily: 14, weekly: 0 }), 'kept daily for 14 days');
  assert.equal(keptWords({ daily: 0, weekly: 8 }), 'kept weekly for 8 weeks');
  assert.equal(keptWords({ daily: 0, weekly: 0 }), 'only the newest kept');
  assert.equal(keptWords(null), '', 'a server that does not say the rule gives none');
  assert.equal(keptWords({}), '');
});

test('the snapshots of every place are one list, newest first, and a round that made several is one row', () => {
  const rows = mergeSnapshots([
    { place: '/users', snapshots: [snapshot('2026-11-12-0300', '2026-11-12T03:00:00Z'), snapshot('2026-11-13-0300', '2026-11-13T03:00:00Z', { kept: ['newest', 'daily'] })] },
    { place: '/shared', snapshots: [snapshot('2026-11-13-0300', '2026-11-13T03:00:01Z', { kept: ['newest', 'daily'] })] },
    { place: '/system/settings', snapshots: [snapshot('2026-11-13-0300', '2026-11-13T03:00:02Z')] },
  ]);
  assert.deepEqual(names(rows), ['2026-11-13-0300', '2026-11-12-0300']);
  assert.deepEqual(rows[0].places, ['/users', '/shared', '/system/settings']);
  assert.equal(rows[0].ms, at(2026, 11, 13, 3, 0), 'the time of a round is its first snapshot\'s');
  assert.deepEqual(rows[0].kept, ['newest', 'daily']);
  assert.deepEqual(rows[1].places, ['/users']);
});

test('a snapshot that was named is a row of its own by its name, and one that backup received says so', () => {
  const rows = mergeSnapshots([
    { place: '/users', snapshots: [snapshot('before-the-move', '2026-11-13T18:00:00Z', { named: true, kept: [] }), snapshot('restored-2026-11-10-020412', '2026-11-14T09:00:00Z', { named: true, kept: [], restored: true })] },
    { place: '/shared', snapshots: [snapshot('before-the-move', '2026-11-13T18:00:09Z', { named: true, kept: [] })] },
  ]);
  assert.deepEqual(names(rows), ['restored-2026-11-10-020412', 'before-the-move']);
  assert.equal(rows[0].restored, true);
  assert.equal(rows[1].named, true);
  assert.deepEqual(rows[1].places, ['/users', '/shared']);
});

test('the snapshot an update kept of the settings is one row, from its earliest time, and a snapshot with no time that reads is left out', () => {
  const rows = mergeSnapshots([
    { place: '/users', snapshots: [snapshot('2026-11-13-0300', '2026-11-13T03:00:00Z'), snapshot('bad', 'last tuesday'), snapshot('worse', undefined), { name: 7, made: '2026-11-13T03:00:00Z' }, null] },
  ], [{ name: 'settings', made: '2026-11-13T03:02:10Z' }, { name: 'state', made: '2026-11-13T03:02:11Z' }]);
  assert.deepEqual(names(rows), ['update', '2026-11-13-0300']);
  assert.equal(rows[0].update, true);
  assert.equal(rows[0].ms, at(2026, 11, 13, 3, 2) + 10_000);
  assert.deepEqual(rows[0].places, []);
  assert.deepEqual(mergeSnapshots([{ place: '/users', snapshots: 'none' }], 'none'), []);
});

test('what kept a snapshot is one word: its name, the longest rule, or that the next prune removes it', () => {
  const row = (more) => ({ name: '2026-11-13-0300', named: false, prunable: false, kept: [], ...more });
  assert.equal(whyKept(row({ kept: ['newest', 'daily', 'weekly'] })), 'daily');
  assert.equal(whyKept(row({ kept: ['weekly'] })), 'weekly');
  assert.equal(whyKept(row({ kept: ['newest'] })), 'newest');
  assert.equal(whyKept(row({ named: true, name: 'before-the-move', kept: ['named'] })), 'before-the-move');
  assert.equal(whyKept(row({ prunable: true })), 'removed at the next prune');
  assert.equal(whyKept(row()), '');
});

/** Rows made at `times` (UTC milliseconds). */
const made = (...times) => times.map((ms, number) => ({ id: `s${number}`, name: `s${number}`, ms, places: ['/users'], named: false, kept: ['daily'], prunable: false }));

test('the days are the person\'s own: a snapshot at 23:30 UTC on Friday is Saturday for Prague and is Friday for New York', () => {
  const rows = made(at(2026, 11, 13, 23, 30), at(2026, 11, 14, 8, 0));
  const titles = (zone) => byDay(rows, zone, NOW).map((group) => `${group.title}: ${names(group.rows)}`);
  assert.deepEqual(titles('UTC'), ['Today: s1', 'Yesterday: s0']);
  // Prague is an hour ahead in November: the late one is 00:30 on Saturday.
  assert.deepEqual(titles('Europe/Prague'), ['Today: s1,s0']);
  // New York is five hours behind: both are on Friday and Saturday's morning is 03:00.
  assert.deepEqual(titles('America/New_York'), ['Today: s1', 'Yesterday: s0']);
});

test('midnight is where the clocks of the zone say, across a change of summer time', () => {
  // Prague, Sunday March 29 2026 is 23 hours long: the clocks go from 02:00 to 03:00.
  const now = at(2026, 3, 29, 12, 0);
  const rows = made(at(2026, 3, 28, 22, 59), at(2026, 3, 28, 23, 0), at(2026, 3, 27, 23, 0), at(2026, 3, 27, 22, 59));
  const groups = byDay(rows, 'Europe/Prague', now);
  assert.deepEqual(groups.map((group) => `${group.id}: ${names(group.rows)}`), ['today: s1', 'yesterday: s0,s2', 'earlier: s3']);
});

test('the days of New York are cut at its midnight, 05:00 UTC in winter', () => {
  const now = at(2026, 11, 14, 3, 30);
  const rows = made(at(2026, 11, 13, 5, 0), at(2026, 11, 13, 4, 59), at(2026, 11, 12, 5, 0), at(2026, 11, 12, 4, 59));
  assert.deepEqual(byDay(rows, 'America/New_York', now).map((group) => `${group.id}: ${names(group.rows)}`), ['today: s0', 'yesterday: s1,s2', 'earlier: s3']);
});

test('a group with no rows is left out, a time after now is today\'s, and rows come newest first in each group', () => {
  const rows = made(at(2026, 11, 1), at(2026, 11, 14, 11, 0), at(2026, 11, 2), at(2026, 10, 1));
  const groups = byDay(rows, 'UTC', NOW);
  assert.deepEqual(groups.map((group) => group.id), ['today', 'earlier']);
  assert.deepEqual(names(groups[1].rows), ['s2', 's0', 's3']);
  assert.deepEqual(byDay([], 'UTC', NOW), []);
});

test('the rows of Earlier are counted by what keeps them, and by their number when the server does not say', () => {
  const row = (more) => ({ named: false, kept: [], ...more });
  assert.equal(earlierWords([...Array(14).fill(row({ kept: ['daily'] })), ...Array(4).fill(row({ kept: ['weekly'] }))]), '14 daily and 4 weekly');
  assert.equal(earlierWords([row({ kept: ['newest', 'daily', 'weekly'] })]), '1 daily');
  assert.equal(earlierWords([row({ kept: ['daily'] }), row({ kept: ['weekly'] }), row({ named: true })]), '1 daily, 1 weekly and 1 named');
  assert.equal(earlierWords([row(), row()]), '2 snapshots');
  assert.equal(earlierWords([row({ kept: ['daily'] }), row()]), '2 snapshots', 'a count that leaves one out is not said');
  assert.equal(earlierWords([row()]), '1 snapshot');
});

test('a day is told as the month and the number, with the year when it is another', () => {
  assert.equal(dayWords(at(2026, 9, 18, 3), 'UTC', NOW), 'Sep 18');
  assert.equal(dayWords(at(2025, 12, 31, 23, 30), 'UTC', NOW), 'Dec 31, 2025');
  assert.equal(dayWords(at(2025, 12, 31, 23, 30), 'Europe/Prague', NOW), 'Jan 1', 'in the zone\'s own day, which is in this year there');
  assert.equal(oldestWords(made(at(2026, 10, 2), at(2026, 9, 18, 3), at(2026, 11, 13)), 'UTC', NOW), 'Oldest Sep 18');
  assert.equal(oldestWords([], 'UTC', NOW), '');
});

test('a snapshot that backup received says which backup it came from', () => {
  const row = { name: 'restored-2026-11-10-020412' };
  assert.equal(restoredWords(row, 'UTC', 'nas', NOW), 'Restored from nas: its backup of Nov 10');
  assert.equal(restoredWords(row, 'UTC', null, NOW), 'Restored from a backup of Nov 10');
  assert.equal(restoredWords({ name: 'restored-2025-12-30-020412' }, 'UTC', null, NOW), 'Restored from a backup of Dec 30, 2025');
  assert.equal(restoredWords({ name: 'restored' }, 'UTC', 'nas', NOW), 'Restored from a backup');
});

test('the label of a restored file is the snapshot\'s time on the person\'s clocks, in plain characters', () => {
  assert.equal(beside(at(2026, 11, 13, 3, 2), 'UTC', NOW), 'Nov 13 3:02 AM');
  assert.equal(beside(at(2026, 11, 13, 3, 2), 'America/New_York', NOW), 'Nov 12 10:02 PM');
  assert.equal(beside(at(2026, 11, 13, 3, 2), 'Europe/Prague', NOW), 'Nov 13 4:02 AM');
  assert.equal(beside(at(2025, 3, 1, 15, 30), 'UTC', NOW), 'Mar 1, 2025 3:30 PM');
});

test('the label follows the clocks across a change of summer time, and not a fixed offset', () => {
  // Prague: the clocks go forward at 01:00 UTC on March 29 2026 and back at 01:00 UTC on October 25.
  assert.equal(beside(at(2026, 3, 29, 0, 30), 'Europe/Prague', NOW), 'Mar 29 1:30 AM');
  assert.equal(beside(at(2026, 3, 29, 1, 30), 'Europe/Prague', NOW), 'Mar 29 3:30 AM');
  assert.equal(beside(at(2026, 10, 24, 22, 30), 'Europe/Prague', NOW), 'Oct 25 12:30 AM');
  assert.equal(beside(at(2026, 10, 25, 1, 30), 'Europe/Prague', NOW), 'Oct 25 2:30 AM');
  assert.equal(beside(at(2026, 10, 25, 2, 30), 'Europe/Prague', NOW), 'Oct 25 3:30 AM');
  // New York: forward at 07:00 UTC on March 8 2026.
  assert.equal(beside(at(2026, 3, 8, 6, 30), 'America/New_York', NOW), 'Mar 8 1:30 AM');
  assert.equal(beside(at(2026, 3, 8, 7, 30), 'America/New_York', NOW), 'Mar 8 3:30 AM');
});

test('a label is printable ASCII of at most 40 characters with no slash, whatever the time and zone', () => {
  for (const zone of ['UTC', 'Europe/Prague', 'America/New_York', 'Asia/Kolkata', 'Pacific/Auckland']) {
    for (let step = 0; step < 400; step += 1) {
      const label = beside(at(2024, 1, 1) + step * 37 * 3_600_000 + 17 * 60_000, zone, NOW);
      assert.match(label, /^[ -~]{1,40}$/, `${zone}: ${JSON.stringify(label)}`);
      assert.equal(label.includes('/'), false);
    }
  }
});

/** What `snap browse --json` gives for the folder `/users/owner/photos`. */
const browse = (entries, more = {}) => ({ place: '/users', snapshot: '2026-11-13-0302', folder: '/users/owner/photos', entries, more: false, ...more });

test('the rows of a folder are in the command\'s order, each with its path, its size and how it compares with now', () => {
  const rows = rowsOf(browse([
    { name: '2026-11', kind: 'folder', size: 4096, modified_ms: 1, then: 'same' },
    { name: 'IMG_4410.heic', kind: 'file', size: 3_100_000, modified_ms: 2, then: 'deleted' },
    { name: 'album.json', kind: 'file', size: 4_000, modified_ms: 3, then: 'changed' },
    { name: 'latest', kind: 'link', size: 12, modified_ms: 4, then: 'same' },
    { name: 'fifo', kind: 'other', size: 0, modified_ms: 5, then: 'same' },
  ]));
  assert.deepEqual(rows.map((row) => row.name), ['2026-11', 'IMG_4410.heic', 'album.json', 'latest', 'fifo']);
  assert.deepEqual(rows.map((row) => row.path), ['/users/owner/photos/2026-11', '/users/owner/photos/IMG_4410.heic', '/users/owner/photos/album.json', '/users/owner/photos/latest', '/users/owner/photos/fifo']);
  assert.deepEqual(rows.map((row) => row.folder), [true, false, false, false, false]);
  assert.deepEqual(rows.map((row) => row.size), ['', '3.1 MB', '4 KB', '12 B', '0 B']);
  assert.deepEqual(rows.map((row) => row.then), [null, { word: 'deleted since', tone: 'warn' }, { word: 'changed since', tone: '' }, null, null]);
  assert.deepEqual(rows.map((row) => row.pickable), [true, true, true, true, false]);
});

test('a name with a line break cannot be a line of paths, so it cannot be chosen; and what the command does not say is left plain', () => {
  const rows = rowsOf(browse([
    { name: 'a\nb', kind: 'file', size: 1, then: 'deleted' },
    { name: 'x', kind: 'socket', size: 'big', then: 'constructor' },
    { kind: 'file' },
    null,
  ], { folder: '/users/' }));
  assert.deepEqual(rows.map((row) => row.pickable), [false, false]);
  assert.equal(rows[0].path, '/users/a\nb');
  assert.equal(rows[1].kind, 'other');
  assert.equal(rows[1].size, '');
  assert.equal(rows[1].then, null, 'a word that is not one of the three is not a property of the table');
  assert.deepEqual(rowsOf(null), []);
  assert.deepEqual(rowsOf({ folder: '/users', entries: 'many' }), []);
});

test('the folders from the top of a place down are each a link', () => {
  assert.deepEqual(trail('/users', '/users/owner/photos'), [
    { name: '/users', folder: '/users' },
    { name: 'owner', folder: '/users/owner' },
    { name: 'photos', folder: '/users/owner/photos' },
  ]);
  assert.deepEqual(trail('/users', '/users'), [{ name: '/users', folder: '/users' }]);
  assert.deepEqual(trail('/users', '/shared/x'), [{ name: '/users', folder: '/users' }], 'a folder outside the place is not followed');
});

test('a snapshot\'s time in a heading says today and yesterday in lower case and gives the day otherwise', () => {
  const time = makeTime('UTC', () => NOW);
  assert.equal(whenWords(at(2026, 11, 14, 3, 2), time), 'today 3:02 AM');
  assert.equal(whenWords(at(2026, 11, 13, 18, 41), time), 'yesterday 6:41 PM');
  assert.equal(whenWords(at(2026, 11, 10, 2, 4), time), 'Nov 10 2:04 AM');
  assert.match(whenWords(at(2025, 3, 1, 15, 30), time), /2025/, 'another year says its year');
});

test('the address of a snapshot\'s folder carries the place, the snapshot and the folder', () => {
  const href = browseHref('/users', '2026-11-13-0302', '/users/owner/my photos');
  const query = new URL(href, 'https://console.invalid').searchParams;
  assert.ok(href.startsWith('/snapshots/browse?'));
  assert.equal(query.get('place'), '/users');
  assert.equal(query.get('snapshot'), '2026-11-13-0302');
  assert.equal(query.get('folder'), '/users/owner/my photos');
});

test('a restored copy is named beside the original with the label before the extension', () => {
  const label = 'Nov 13 3:02 AM';
  assert.equal(besideName('/users/owner/report.odt', label), '/users/owner/report (Nov 13 3:02 AM).odt');
  assert.equal(besideName('/users/owner/archive.tar.gz', label), '/users/owner/archive.tar (Nov 13 3:02 AM).gz');
  assert.equal(besideName('/users/owner/notes', label), '/users/owner/notes (Nov 13 3:02 AM)');
  assert.equal(besideName('/users/owner/.profile', label), '/users/owner/.profile (Nov 13 3:02 AM)', 'a leading dot is not an extension');
  assert.equal(besideName('/users/owner/photos.2025', label, { folder: true }), '/users/owner/photos.2025 (Nov 13 3:02 AM)', 'a folder has no extension');
  assert.equal(besideName('/users/owner/a b.txt', label), '/users/owner/a b (Nov 13 3:02 AM).txt');
});

test('one restore takes at most 100 paths', () => {
  assert.equal(RESTORE_MOST, 100);
});
