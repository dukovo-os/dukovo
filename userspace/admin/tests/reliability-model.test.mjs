// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// pages/reliability/model.js, starts-model.js, reads-model.js and journal-model.js: the days and months of the
// range, the events of each and the problems a day, the starts and an update
// from a change of release, the failed start, one start beside another, and
// the journal's filters and the script of the changes a person chooses, from
// fixtures of what admind answers.

import assert from 'node:assert/strict';
import test from 'node:test';

import { makeTime } from '../lib/time.js';
import {
  addressOf, dayChoices, filterRows, howOf, queryOf, READ_PATH, scriptable, scriptOf, shellWord, whoChoices,
} from '../pages/reliability/journal-model.js';
import {
  alertRows, ALERTS_SHOWN, barsOf, bucketAt, bucketsOf, dumpRows, eventsOf, groupsOf, problemsOf, problemsWords, rangeOf, tableOf,
  timelineOf, TIMELINE_MAX,
} from '../pages/reliability/model.js';
import { onlyWords } from '../pages/overview/admin-reads.js';
import { cappedWords, pageOf } from '../pages/reliability/reads-model.js';
import {
  chainWords, changeWords, deltaWords, failedOf, hasNoTimes, msWords, restartsOf, resultOf, slowestOf, startRows, stepRows,
  tryNumbers, updatesOf,
} from '../pages/reliability/starts-model.js';
import * as fixture from './reliability-fixtures.mjs';

const { NOW_MS } = fixture;
const time = (zone = 'UTC') => makeTime(zone, () => NOW_MS);
const at = (text) => Date.parse(text);

// ---- days and months ---------------------------------------------------------------------

test('a word that is no range is the default one, and a year counts months', () => {
  assert.equal(rangeOf('30d').count, 30);
  assert.equal(rangeOf('1y').unit, 'month');
  assert.equal(rangeOf('2d').id, '7d');
  assert.equal(rangeOf(null).id, '7d');
});

test('7 days are the 7 days that end with today, each from its midnight to the next', () => {
  const days = bucketsOf(rangeOf('7d'), NOW_MS, 'UTC');
  assert.deepEqual(days.map((day) => day.key), ['2026-11-08', '2026-11-09', '2026-11-10', '2026-11-11', '2026-11-12', '2026-11-13', '2026-11-14']);
  assert.equal(days[6].since, at('2026-11-14T00:00:00Z'));
  assert.equal(days[6].until, at('2026-11-15T00:00:00Z'));
  assert.ok(days.every((day, number) => number === 0 || day.since === days[number - 1].until), 'no gap and no overlap');
  assert.deepEqual([days[6].short, days[6].words], ['Nov 14', 'Sat, Nov 14, 2026']);
});

test('30 days start 29 days before today, over the turn of a month', () => {
  const days = bucketsOf(rangeOf('30d'), NOW_MS, 'UTC');
  assert.equal(days.length, 30);
  assert.deepEqual([days[0].key, days[29].key, days[14].key], ['2026-10-16', '2026-11-14', '2026-10-30']);
});

test('a year is 12 months, the last the month that holds today, over the turn of a year', () => {
  const months = bucketsOf(rangeOf('1y'), NOW_MS, 'UTC');
  assert.equal(months.length, 12);
  assert.deepEqual([months[0].key, months[1].key, months[11].key], ['2025-12', '2026-01', '2026-11']);
  assert.deepEqual([months[11].short, months[11].words], ['Nov 2026', 'November 2026']);
  assert.equal(months[11].since, at('2026-11-01T00:00:00Z'));
  assert.equal(months[11].until, at('2026-12-01T00:00:00Z'));
  assert.equal(months[10].until, months[11].since);
});

test('the day of a time is the one on the person\'s clocks: 30 minutes after midnight in Prague is the evening before in New York', () => {
  const late = at('2026-11-13T23:30:00Z');
  const keyOf = (zone) => {
    const days = bucketsOf(rangeOf('7d'), NOW_MS, zone);
    return days[bucketAt(days, late)].key;
  };
  assert.equal(keyOf('UTC'), '2026-11-13');
  assert.equal(keyOf('Europe/Prague'), '2026-11-14');
  assert.equal(keyOf('America/New_York'), '2026-11-13');
  const days = bucketsOf(rangeOf('7d'), NOW_MS, 'UTC');
  assert.equal(bucketAt(days, at('2026-11-07T23:59:59Z')), -1, 'before the first day');
  assert.equal(bucketAt(days, at('2026-11-15T00:00:00Z')), -1, 'after the last');
});

test('the day the clocks go back is 25 hours long, and each month starts where its zone says', () => {
  const now = at('2026-10-27T10:00:00Z');
  const days = bucketsOf(rangeOf('7d'), now, 'Europe/Prague');
  const long = days.find((day) => day.key === '2026-10-25');
  assert.equal((long.until - long.since) / 3_600_000, 25);
  const months = bucketsOf(rangeOf('1y'), NOW_MS, 'Europe/Prague');
  assert.equal(months[11].since, at('2026-10-31T23:00:00Z'));
  assert.equal(months[10].until, months[11].since);
});

test('a length of time in the shortest honest words', () => {
  assert.deepEqual([850, 999.4, 1000, 41_200, 59_949, 60_000, 150_000, null, undefined].map(msWords), [
    '850 ms', '999 ms', '1.0 s', '41.2 s', '59.9 s', '1 min', '3 min', '–', '–',
  ]);
});

// ---- starts --------------------------------------------------------------------------------

test('a run of tries of changes bisect is numbered from its first, and a run after a good start begins again', () => {
  assert.deepEqual([...tryNumbers(fixture.starts)], [[42, 1], [43, 2]]);
  const again = [{ start: 9, result: 'bisect-try' }, { start: 8, result: 'good' }, { start: 7, result: 'bisect-try' }, { start: 6, result: 'bisect-try' }];
  assert.deepEqual([...tryNumbers(again)], [[6, 1], [7, 2], [9, 1]]);
});

test('each result says what it was, and the check that failed a start', () => {
  const tries = tryNumbers(fixture.starts);
  const [, try2, , fellBack, failedStart, good] = fixture.starts;
  assert.deepEqual(resultOf(good, tries), { words: 'Good', dot: 'info' });
  assert.deepEqual(resultOf(failedStart, tries), { words: 'Failed: BOOT-104', dot: 'bad' });
  assert.deepEqual(resultOf({ ...failedStart, check: null }, tries), { words: 'Failed', dot: 'bad' });
  assert.deepEqual(resultOf(fellBack, tries), { words: 'Fell back', dot: 'bad' });
  assert.deepEqual(resultOf(try2, tries), { words: 'Try 2 of changes bisect', dot: 'info' });
  assert.deepEqual(resultOf({ start: 1, result: 'unfinished' }, tries), { words: 'Unfinished', dot: 'info' });
  assert.deepEqual(resultOf({ start: 1, result: null }, tries), { words: 'Not finished', dot: 'info' });
  assert.deepEqual(resultOf({ start: 1, result: '' }, tries), { words: '', dot: 'info' });
});

test('the rows of the Starts table: when, the release and its slot, how long, how it ended, and a link', () => {
  const rows = startRows(fixture.starts, time());
  assert.deepEqual(rows[0], {
    number: 44, href: '/reliability/starts/44', started: '4:10 AM', version: '1.0.0-alpha.1 (slot A)', ready: '40.5 s', result: { words: 'Good', dot: 'info' },
  });
  assert.deepEqual([rows[4].ready, rows[4].result.words, rows[4].version], ['never', 'Failed: BOOT-104', '1.0.0-alpha.2 (slot B)']);
  assert.equal(rows[5].started, 'Nov 11 2:01 AM');
  assert.equal(rows[1].ready, '–', 'a try that has no time is not "never"');
  const unknown = startRows([{ start: 3, at_ms: null, release: null, slot: null, result: 'good', ready_ms: null }], time());
  assert.deepEqual([unknown[0].started, unknown[0].version, unknown[0].ready], ['Time not known', '', '–']);
  assert.equal(startRows([{ start: 3, release: '2.0', slot: null, result: 'good' }], time())[0].version, '2.0');
});

test('an update is a start whose release is not that of the start before, and none of a fall-back, a try or the oldest start', () => {
  assert.deepEqual(updatesOf(fixture.starts), [
    { start: 40, from: '1.0.0-alpha.1', to: '1.0.0-alpha.2' },
    { start: 39, from: '0.15.0', to: '1.0.0-alpha.1' },
  ]);
  assert.deepEqual(updatesOf([{ start: 1, release: '1.0', result: 'good' }]), []);
  const unknown = [{ start: 3, release: null, result: 'good' }, { start: 2, release: '1.0', result: 'good' }, { start: 1, release: null, result: 'good' }];
  assert.deepEqual(updatesOf(unknown), [], 'a release that is not known is no change of release');
});

// ---- the events of each day ----------------------------------------------------------------

const seven = () => bucketsOf(rangeOf('7d'), NOW_MS, 'UTC');
const groupsFor = (zone = 'UTC', range = '7d', sources = {}) => {
  const buckets = bucketsOf(rangeOf(range), NOW_MS, zone);
  const events = eventsOf({
    starts: fixture.starts, records: fixture.records, changes: fixture.changes, dumps: fixture.dumps, ...sources,
  }, time(zone));
  return { buckets, groups: groupsOf(buckets, events) };
};
const cells = (table, id) => table.rows.find((row) => row.id === id).cells;

test('the table counts each day\'s starts, errors, updates, changes and crashes in words, and says which went wrong', () => {
  const { buckets, groups } = groupsFor();
  const table = tableOf(buckets, groups);
  assert.deepEqual(table.rows.map((row) => row.title), ['Starts', 'Services', 'Updates', 'Changes', 'Crashes']);
  assert.deepEqual(cells(table, 'start')[3], { count: 1, dot: 'info', words: '1 start' });
  assert.deepEqual(cells(table, 'start')[6], { count: 5, dot: 'bad', words: '5 starts, 1 failed, 1 fell back' });
  assert.deepEqual(cells(table, 'error')[5], { count: 3, dot: 'warn', words: '3 errors' });
  assert.deepEqual(cells(table, 'update')[3], { count: 1, dot: 'info', words: '1 update' });
  assert.deepEqual(cells(table, 'change')[5], { count: 6, dot: 'bad', words: '6 changes, 1 did not work, 1 taken back' });
  assert.deepEqual(cells(table, 'change')[6], { count: 3, dot: 'bad', words: '3 changes, 1 taken back' });
  assert.deepEqual(cells(table, 'crash')[6], { count: 1, dot: 'bad', words: '1 crash' });
  assert.deepEqual(cells(table, 'crash')[5], { count: 0, dot: null, words: '' });
  assert.equal(cells(table, 'start').length, 7);
});

test('a source that was not read has no row, and one that was read and is empty has a row of empty cells', () => {
  const { buckets, groups } = groupsFor('UTC', '7d', { records: null, dumps: null });
  const table = tableOf(buckets, groups, ['error', 'crash']);
  assert.deepEqual(table.rows.map((row) => row.id), ['start', 'update', 'change']);
  const { buckets: quiet, groups: none } = groupsFor('UTC', '7d', { starts: [], records: [], changes: [], dumps: [] });
  assert.ok(tableOf(quiet, none).rows.every((row) => row.cells.every((cell) => cell.count === 0)));
});

test('a month holds all of its days\' events, and a start at the turn of a month is on the day its zone says', () => {
  const months = groupsFor('UTC', '1y');
  const counts = (groups) => groups.map((group) => group.start.length);
  assert.deepEqual(counts(months.groups).slice(-2), [2, 7], 'October has the starts of the 20th and the 31st, November seven');
  const prague = groupsFor('Europe/Prague', '1y');
  assert.deepEqual(counts(prague.groups).slice(-2), [1, 8], 'the start at 23:30 UTC on the 31st is November\'s in Prague');
  const york = groupsFor('America/New_York', '30d');
  const october31 = york.buckets.findIndex((bucket) => bucket.key === '2026-10-31');
  assert.equal(york.groups[october31].start.length, 1);
  const pragueDays = groupsFor('Europe/Prague', '30d');
  assert.equal(pragueDays.groups[pragueDays.buckets.findIndex((bucket) => bucket.key === '2026-11-01')].start.length, 1);
});

test('problems a day are the failed starts, the crashes and the errors, and a fall-back or a try is no new failure', () => {
  const { buckets, groups } = groupsFor();
  const problems = problemsOf(buckets, groups);
  assert.deepEqual(problems[6], {
    key: '2026-11-14', short: 'Nov 14', words: 'Sat, Nov 14, 2026', failed: 1, crashes: 1, errors: 3, total: 5,
  });
  assert.deepEqual([problems[5].failed, problems[5].crashes, problems[5].errors, problems[5].total], [0, 0, 3, 3]);
  assert.deepEqual(problems.slice(0, 5).map((one) => one.total), [0, 0, 0, 0, 0]);
  assert.equal(problemsWords(problems, rangeOf('7d')), 'Problems a day over the last 7 days: 8 in all, the worst being Sat, Nov 14, 2026 with 5');
  assert.equal(problemsWords(problems.map((one) => ({ ...one, total: 0 })), rangeOf('1y')), 'Problems a month over the last year: none');
});

test('the bars are as high as their count against the highest, none for a day with no problem', () => {
  const { buckets, groups } = groupsFor();
  const { most, bars } = barsOf(problemsOf(buckets, groups));
  assert.equal(most, 5);
  assert.deepEqual(bars.map((bar) => bar.height), [0, 0, 0, 0, 0, 43.2, 72]);
  assert.ok(bars.every((bar, number) => number === 0 || bar.x > bars[number - 1].x + bars[number - 1].width - 0.001), 'the bars do not touch');
  assert.ok(bars[6].x + bars[6].width <= 300);
  assert.deepEqual(barsOf(problemsOf(seven(), groupsOf(seven(), []))).bars.map((bar) => bar.height), [0, 0, 0, 0, 0, 0, 0]);
});

test('a day\'s timeline is oldest first, one item for each service\'s errors, with the last of its words', () => {
  const { groups } = groupsFor();
  const { items, left } = timelineOf(groups[6]);
  assert.equal(left, 0);
  assert.deepEqual(items.map((item) => item.title), [
    'Start failed: BOOT-104', 'Updated to 1.0.0-alpha.2', 'The kernel crashed', 'kernel: 1 error', 'nginx: 2 errors',
    'Went back to 1.0.0-alpha.1', 'network put back: it was being tried', 'Add a firewall rule', 'Try 1 of changes bisect',
    'Try 2 of changes bisect', 'Started', 'Change a setting',
  ]);
  assert.equal(items.at(-1).title, 'Change a setting', 'the newest is last');
  const nginx = items.find((item) => item.title === 'nginx: 2 errors');
  assert.equal(nginx.sub, 'The last: php-fpm answered 502');
  assert.equal(nginx.dot, 'warn');
  const failedStart = items.find((item) => item.title === 'Start failed: BOOT-104');
  assert.deepEqual([failedStart.dot, failedStart.href, failedStart.sub], ['bad', '/reliability/starts/40', '1.0.0-alpha.2 (slot B)']);
});

test('a long error text is cut, and only the newest of a crowded day are listed', () => {
  const { groups } = groupsFor();
  const long = timelineOf(groups[5]).items.find((item) => item.title === 'backup-sync: 2 errors');
  assert.equal(long.sub, 'The last: The NAS at 192.168.1.20 did not answer.');
  const wide = groupsFor('UTC', '7d', { records: [{ ...fixture.records[3], time: '2026-11-14T05:00:00.000000Z' }] });
  assert.equal(timelineOf(wide.groups[6]).items.find((item) => item.title === 'nginx: 1 error').sub.length, 'The last: '.length + 141);
  const many = Array.from({ length: TIMELINE_MAX + 20 }, (_, number) => ({
    kind: 'change', at: number, dot: 'info', title: `Change ${number}`, sub: '', failed: false, taken: false,
  }));
  const crowded = timelineOf({ start: [], update: [], change: many, crash: [], error: [] });
  assert.equal(crowded.items.length, TIMELINE_MAX);
  assert.equal(crowded.left, 20);
  assert.equal(crowded.items.at(-1).title, `Change ${TIMELINE_MAX + 19}`);
});

// ---- a failed start --------------------------------------------------------------------------

test('how many restarts a search takes: one for a lone change, one for each halving', () => {
  assert.deepEqual([0, 1, 2, 3, 4, 5, 8, 9].map(restartsOf), [0, 1, 1, 2, 2, 3, 3, 4]);
});

test('a change since the last good start in words, with its time when it has one', () => {
  assert.equal(changeWords({ what: 'Radix', version: '0.8.1', at_ms: at('2026-11-14T03:02:00Z') }, time()), 'Radix 0.8.1, 3:02 AM');
  assert.equal(changeWords({ what: 'firmware set', version: '2026.11.4', at_ms: null }, time()), 'Firmware set 2026.11.4');
  assert.equal(changeWords({ what: 'CPU microcode', version: '81c0e2d4', at_ms: null }, time()), 'CPU microcode 81c0e2d4');
  assert.equal(changeWords({ what: 'org.nginx.nginx', version: '1.30.6', at_ms: null }, time()), 'org.nginx.nginx 1.30.6');
  assert.equal(changeWords({ what: 'boot options', version: null, at_ms: null }, time()), 'Boot options, removed');
  assert.equal(changeWords({ what: null, version: '1', at_ms: null }, time()), 'Something 1');
});

test('the card of a failed start: what failed it, the changes since, and the offer to find which broke it', () => {
  const card = failedOf(fixture.failed, time());
  assert.equal(card.sentence, 'The start at 3:03 AM failed its health check (BOOT-104).');
  assert.equal(card.href, '/reliability/starts/40');
  assert.equal(card.lead, 'These changes came since the last good start:');
  assert.deepEqual(card.changes, ['Dukovo 1.0.0-alpha.2, 3:02 AM', 'Radix 0.8.1, 3:02 AM', 'org.blog.blog 2.1, Yesterday 9:10 PM']);
  assert.deepEqual(card.offer, {
    words: 'Find which change broke it. The machine restarts up to 2 times, and nothing is changed for good.',
    command: 'elevate changes bisect',
    later: 'Running it from the console comes with the Hardware page.',
  });
  assert.equal(card.running, null);
});

test('a search that runs shows its try and how to stop it, and offers no second one', () => {
  const card = failedOf(fixture.failedWhileSearching, time());
  assert.deepEqual(card.running, { words: 'Changes bisect is running: try 1 of 2.', stop: 'elevate changes bisect --stop' });
  assert.equal(card.offer, null);
  const unsure = failedOf({ ...fixture.failedWhileSearching, bisect: { running: true, try: null, of: null } }, time());
  assert.equal(unsure.running.words, 'Changes bisect is running.');
});

test('a failed start with fewer than two changes has no search to offer, and none failed has no card', () => {
  const one = failedOf({ ...fixture.failed, since_good: fixture.failed.since_good.slice(0, 1) }, time());
  assert.equal(one.offer, null);
  assert.equal(one.changes.length, 1);
  const none = failedOf({ ...fixture.failed, since_good: [], failed: { start: 5, at_ms: null, check: null } }, time());
  assert.equal(none.lead, 'No change is listed since the last good start.');
  assert.equal(none.sentence, 'The start failed its health check.');
  assert.equal(failedOf({ failed: null, since_good: [], bisect: { running: false } }, time()), null);
  assert.equal(failedOf(null, time()), null);
});

// ---- one start ---------------------------------------------------------------------------------

test('a change of time in words: nothing under a tenth of a second, a sign above', () => {
  assert.deepEqual([[900, 850], [1800, 1800], [7100, 2300], [900, 1000], [null, 1000], [5, null]].map(([now, then]) => deltaWords(now, then)), [
    'No change', 'No change', '+4.8 s', '-100 ms', '', '',
  ]);
});

test('the steps of a start beside those of the start it is compared with, and worse ones marked', () => {
  const rows = stepRows(fixture.startAnswer, fixture.startAnswer.compared_with);
  assert.deepEqual(rows.map((row) => row.name), ['Firmware', 'Boot loader', 'Kernel', 'Services', 'Sign-in']);
  assert.deepEqual(rows[3], {
    name: 'Services', now: '7.1 s', then: '2.3 s', change: '+4.8 s', worse: true,
  });
  assert.deepEqual([rows[0].change, rows[1].change, rows[1].worse], ['No change', 'No change', false]);
  assert.deepEqual(rows[4], {
    name: 'Sign-in', now: '–', then: '12.0 s', change: '', worse: false,
  });
});

test('without a start to compare with a step has its time alone, and a step the other start lacks has no time there', () => {
  const alone = stepRows(fixture.startAnswer, null);
  assert.deepEqual([alone[3].now, alone[3].then, alone[3].change, alone[3].worse], ['7.1 s', null, null, false]);
  const lacking = stepRows(fixture.startAnswer, { start: 3, steps: [{ name: 'firmware', ms: 1800 }] });
  assert.deepEqual([lacking[1].then, lacking[1].change], ['–', '']);
});

test('the slowest services and the chain that set when the start settled', () => {
  assert.deepEqual(slowestOf(fixture.startAnswer), [{ name: 'web', took: '6.6 s' }, { name: 'netd', took: '1.0 s' }]);
  assert.equal(chainWords(fixture.startAnswer), 'logd, then netd, then web');
  assert.equal(chainWords({ chain: [] }), null);
  assert.deepEqual(slowestOf({}), []);
});

test('startup ending other than with 0 is the answer that no times are kept, and no other error is', () => {
  assert.equal(hasNoTimes({ id: 'ADM-123', command: { status: 1 } }), true);
  assert.equal(hasNoTimes({ id: 'ADM-123', command: { status: 4 } }), true);
  assert.equal(hasNoTimes({ id: 'ADM-129', command: { status: 1 } }), false);
  assert.equal(hasNoTimes(null), false);
});

// ---- crashes and alerts --------------------------------------------------------------------------

test('the crash dumps, newest first, with the command that shows one and a dump that has no time said so', () => {
  assert.deepEqual(dumpRows([...fixture.dumps].reverse(), time()), [
    {
      name: 'dump-20261114-0305.txt', when: '3:05 AM', size: '48 KB', line: 'panic: kernel BUG at mm/slab.c:411', command: 'elevate dumps show dump-20261114-0305.txt',
    },
    {
      name: 'dump-old.txt', when: 'Time not known', size: '2 KB', line: 'not a dump: it has no header', command: 'elevate dumps show dump-old.txt',
    },
  ]);
  assert.deepEqual(dumpRows([], time()), []);
});

test('the alerts are the newest 20, newest first, with their state and how bad', () => {
  const rows = alertRows(fixture.alerts, time());
  assert.equal(rows.length, ALERTS_SHOWN);
  assert.deepEqual(rows[0], {
    when: 'Yesterday 9:00 PM', state: 'Cleared', severity: 'Warning', dot: 'warn', source: 'diskhealth', title: 'Disk 21',
  });
  assert.deepEqual([rows[1].state, rows[1].severity, rows[1].dot, rows[1].title], ['Raised', 'Critical', 'bad', 'Disk 20']);
  assert.equal(rows.at(-1).title, 'Disk 2');
  assert.deepEqual(alertRows([{ at: 5 }, null, { at: 'x', alert: {} }], time()), []);
  assert.equal(alertRows([{ at: 5, state: 'odd', alert: { severity: 'warning', title: 't' } }], time())[0].state, 'Odd');
});

test('what a card says in place of what only an unlocked administrator reads', () => {
  assert.equal(onlyWords('errors, crashes and alerts', true), 'Unlock to see errors, crashes and alerts.');
  assert.equal(onlyWords('errors, crashes and alerts', false), 'Administrators see errors, crashes and alerts.');
});

// ---- the journal -------------------------------------------------------------------------------------

test('how each row was made: here or with a token, by hand, put back, or undone by Spark', () => {
  assert.deepEqual(fixture.changes.map((one) => [one.id, howOf(one)]), [
    [96, 'console'], [95, 'console'], [94, 'undone'], [93, 'put-back'], [92, 'console'], [91, 'hand'], [90, 'console'], [89, 'console'], [88, 'console'],
  ]);
});

test('the filters are read from the address, and one that is no choice does not filter', () => {
  assert.deepEqual(queryOf(new URLSearchParams('who=alice&how=hand&day=2026-11-13')), { who: 'alice', how: 'hand', day: '2026-11-13' });
  assert.deepEqual(queryOf(new URLSearchParams('how=sideways&day=yesterday')), { who: '', how: 'any', day: '' });
  assert.deepEqual(queryOf(new URLSearchParams('')), { who: '', how: 'any', day: '' });
});

test('the address holds only the filters that filter, and reads back as they were', () => {
  assert.equal(addressOf({ who: '', how: 'any', day: '' }), '/reliability/changes');
  const query = { who: 'outside the console', how: 'put-back', day: '2026-11-13' };
  const address = addressOf(query);
  assert.equal(address, '/reliability/changes?who=outside+the+console&how=put-back&day=2026-11-13');
  assert.deepEqual(queryOf(new URL(address, 'https://x').searchParams), query);
});

test('the rows pass each filter alone and together, and a day is the person\'s day', () => {
  const ids = (query, zone = 'UTC') => filterRows(fixture.changes, { who: '', how: 'any', day: '', ...query }, zone).map((one) => one.id);
  assert.equal(ids({}).length, 9);
  assert.deepEqual(ids({ who: 'alice' }), [96, 93, 90, 89, 88]);
  assert.deepEqual(ids({ how: 'hand' }), [91]);
  assert.deepEqual(ids({ how: 'console' }), [96, 95, 92, 90, 89, 88]);
  assert.deepEqual(ids({ how: 'put-back' }), [93]);
  assert.deepEqual(ids({ how: 'undone' }), [94]);
  assert.deepEqual(ids({ day: '2026-11-14' }), [96, 95, 94]);
  assert.deepEqual(ids({ day: '2026-11-14' }, 'Europe/Prague'), [96, 95, 94, 93]);
  assert.deepEqual(ids({ who: 'alice', how: 'console', day: '2026-11-13' }), [90, 89, 88]);
  assert.deepEqual(ids({ who: 'nobody' }), []);
});

test('the choices of who and of day are those the rows have, by name and newest day first', () => {
  assert.deepEqual(whoChoices(fixture.changes), [
    { value: 'alice', label: 'alice' }, { value: 'bob', label: 'bob' }, { value: 'ci', label: 'ci' },
    { value: 'outside the console', label: 'Outside the console' }, { value: 'Spark', label: 'Spark' },
  ]);
  assert.deepEqual(dayChoices(fixture.changes, 'UTC'), [
    { value: '2026-11-14', label: 'Sat, Nov 14, 2026' }, { value: '2026-11-13', label: 'Fri, Nov 13, 2026' },
  ]);
  assert.deepEqual(dayChoices(fixture.changes, 'Europe/Prague').map((one) => one.value), ['2026-11-14', '2026-11-13']);
});

test('a count that a read cut short says so, with its number as a person writes it', () => {
  assert.equal(cappedWords(1000, 'errors'), 'Only the newest 1,000 errors are counted, so earlier days may have more.');
  assert.equal(cappedWords(200, 'changes'), 'Only the newest 200 changes are counted, so earlier days may have more.');
  assert.equal(READ_PATH, '/api/1/changes?since=0&limit=200');
});

test('rows are shown a page at a time, and how many are left is told', () => {
  const rows = Array.from({ length: 120 }, (_, number) => number);
  assert.deepEqual([pageOf(rows, 50).shown.length, pageOf(rows, 50).left], [50, 70]);
  assert.deepEqual([pageOf(rows, 100).shown.length, pageOf(rows, 150).left], [100, 0]);
  assert.deepEqual(pageOf([], 50), { shown: [], left: 0 });
});

// ---- the script ------------------------------------------------------------------------------------------

test('a word of a shell line is as admind\'s Show command writes it', () => {
  assert.deepEqual(['service', 'a/b_c-d.e:f,g@h%i+j=k', '', 'two words', 'it\'s', '$HOME', 'a;b', '*', 'é'].map(shellWord), [
    'service', 'a/b_c-d.e:f,g@h%i+j=k', '\'\'', '\'two words\'', '\'it\'\\\'\'s\'', '\'$HOME\'', '\'a;b\'', '\'*\'', '\'é\'',
  ]);
});

test('only a change made through the console that worked, of an action this server has, can be a line of a script', () => {
  const by = (id) => fixture.changes.find((one) => one.id === id);
  const scriptables = fixture.changes.filter((one) => scriptable(one, fixture.runs)).map((one) => one.id);
  assert.deepEqual(scriptables, [96, 95, 90, 89]);
  assert.equal(scriptable(by(92), fixture.runs), false, 'it did not work');
  assert.equal(scriptable(by(93), fixture.runs), false, 'a put back names a row of this machine\'s journal');
  assert.equal(scriptable(by(88), fixture.runs), false, 'this server has no such action');
  assert.equal(scriptable({ ...by(90), argv: [] }, fixture.runs), false);
  assert.equal(scriptable({ ...by(90), argv: null }, fixture.runs), false);
});

test('the script of the chosen rows is their commands oldest first, each with its time and title, and elevate where the action ran elevated', () => {
  assert.equal(scriptOf(fixture.changes, fixture.runs), [
    '#!/bin/sh',
    '# Changes made with the Dukovo console, oldest first.',
    'set -e',
    '',
    '# 2026-11-13 19:00:00 UTC: Copy settings to a folder',
    'admin export /users/alice/duke-nas-settings',
    '',
    '# 2026-11-13 20:00:00 UTC: Restart a service',
    'elevate service diskhealth restart',
    '',
    '# 2026-11-14 03:40:00 UTC: Add a firewall rule',
    'elevate firewall add allow tcp 8080',
    '',
    '# 2026-11-14 08:30:00 UTC: Change a setting',
    'elevate settings set motd \'it\'\\\'\'s a test\'',
    '',
  ].join('\n'));
});

test('a script of chosen rows takes only the ones that can be a line, whatever order they were chosen in, and a title stays on its line', () => {
  const chosen = [fixture.changes[0], fixture.changes[6], fixture.changes[3], fixture.changes[5]];
  const text = scriptOf(chosen, fixture.runs);
  assert.deepEqual(text.split('\n').filter((line) => !line.startsWith('#') && line !== '' && line !== 'set -e'), [
    'elevate service diskhealth restart', 'elevate settings set motd \'it\'\\\'\'s a test\'',
  ]);
  const wide = { ...fixture.changes[6], title: 'Restart\na service\r\nnow', at: fixture.changes[6].at };
  assert.ok(scriptOf([wide], fixture.runs).includes('\n# 2026-11-13 20:00:00 UTC: Restart a service now\n'));
  assert.equal(scriptOf([fixture.changes[5], fixture.changes[3]], fixture.runs), '', 'a hand edit and a put back give no script');
  assert.equal(scriptOf([], fixture.runs), '');
});

test('two rows at the same second keep the order of their numbers', () => {
  const same = [
    { ...fixture.changes[6], id: 12, argv: ['service', 'b', 'restart'] },
    { ...fixture.changes[6], id: 11, argv: ['service', 'a', 'restart'] },
  ];
  assert.deepEqual(scriptOf(same, fixture.runs).split('\n').filter((line) => line.startsWith('elevate')), ['elevate service a restart', 'elevate service b restart']);
});
