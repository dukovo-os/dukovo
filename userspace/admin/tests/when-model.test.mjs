// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The times of the Logs page (design 5.2): each range, the words log(1)
// reads, and a wall time in a zone turned into UTC across both changes of
// summer time, in zones that change on different days and by different gaps.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  addDays, dayOf, dayStart, dayWords, offsetWords, RANGES, rangeById, stampOf, timeOf, utcWords, wallOf, wallToUtc, windowOf,
} from '../lib/when-model.js';

const PRAGUE = 'Europe/Prague';
const NEW_YORK = 'America/New_York';
const utc = (...parts) => Date.UTC(...parts);
/** The Unix time of `date` and `time` in `zone` as an ISO string, or null. */
const at = (date, time, zone) => {
  const found = wallToUtc(date, time, zone);
  return found === null ? null : new Date(found).toISOString();
};

test('the ranges are listed in the order of the filter, with their lengths', () => {
  assert.deepEqual(RANGES.map((range) => range.id), ['15m', '1h', '24h', '7d', 'today', 'custom']);
  assert.deepEqual(RANGES.map((range) => range.seconds), [900, 3600, 86_400, 604_800, null, null]);
  assert.equal(rangeById('24h').label, 'Last 24 hours');
  assert.equal(rangeById('1h').label, 'Last hour');
  assert.equal(rangeById('2h'), undefined);
  assert.equal(rangeById(undefined), undefined);
});

test('a relative range reaches back from now, and has no end', () => {
  const now = utc(2026, 10, 14, 9, 30, 0);
  assert.deepEqual(windowOf('15m', now, PRAGUE), { since: now - 900_000, until: null });
  assert.deepEqual(windowOf('1h', now, PRAGUE), { since: now - 3_600_000, until: null });
  assert.deepEqual(windowOf('24h', now, PRAGUE), { since: now - 86_400_000, until: null });
  assert.deepEqual(windowOf('7d', now, PRAGUE), { since: now - 604_800_000, until: null });
});

test('today begins at midnight in the zone the times are shown in', () => {
  const now = utc(2026, 10, 14, 23, 30, 0);
  // 23:30 UTC is already the 15th in Prague, and still the 14th in New York.
  assert.deepEqual(windowOf('today', now, PRAGUE), { since: utc(2026, 10, 14, 23, 0, 0), until: null });
  assert.deepEqual(windowOf('today', now, NEW_YORK), { since: utc(2026, 10, 14, 5, 0, 0), until: null });
  assert.deepEqual(windowOf('today', now, 'UTC'), { since: utc(2026, 10, 14, 0, 0, 0), until: null });
});

test('today is the right start on the days the clocks change', () => {
  // Prague loses an hour on 29 March (a 23-hour day) and gains one on 25 October.
  assert.deepEqual(windowOf('today', utc(2026, 2, 29, 12), PRAGUE), { since: utc(2026, 2, 28, 23), until: null });
  assert.deepEqual(windowOf('today', utc(2026, 2, 30, 12), PRAGUE), { since: utc(2026, 2, 29, 22), until: null });
  assert.deepEqual(windowOf('today', utc(2026, 9, 25, 12), PRAGUE), { since: utc(2026, 9, 24, 22), until: null });
  assert.deepEqual(windowOf('today', utc(2026, 9, 26, 12), PRAGUE), { since: utc(2026, 9, 25, 23), until: null });
});

test('a custom range is the person\'s, and a word that is no range has none', () => {
  assert.equal(windowOf('custom', Date.now(), PRAGUE), null);
  assert.equal(windowOf('2h', Date.now(), PRAGUE), null);
});

test('utcWords writes the form log(1) and metrics(1) read, to the second', () => {
  assert.equal(utcWords(utc(2026, 10, 14, 9, 30, 0)), '2026-11-14 09:30:00 UTC');
  assert.equal(utcWords(0), '1970-01-01 00:00:00 UTC');
  assert.equal(utcWords(utc(2026, 0, 5, 3, 4, 5, 999)), '2026-01-05 03:04:05 UTC');
  assert.equal(utcWords(utc(2026, 11, 31, 23, 59, 59)), '2026-12-31 23:59:59 UTC');
});

test('wallOf and dayOf read the clocks of a zone', () => {
  const ms = utc(2026, 10, 14, 23, 30, 5);
  assert.deepEqual(wallOf(ms, PRAGUE), { date: '2026-11-15', time: '00:30:05' });
  assert.deepEqual(wallOf(ms, NEW_YORK), { date: '2026-11-14', time: '18:30:05' });
  assert.deepEqual(wallOf(ms, 'UTC'), { date: '2026-11-14', time: '23:30:05' });
  assert.equal(dayOf(ms, PRAGUE), '2026-11-15');
  assert.equal(dayOf(ms, 'Pacific/Kiritimati'), '2026-11-15');
  assert.equal(dayOf(utc(2026, 10, 14, 0, 0, 0), 'Pacific/Pago_Pago'), '2026-11-13');
});

test('a wall time that exists once is the same instant the clocks of its zone show', () => {
  assert.equal(at('2026-01-15', '10:30', PRAGUE), '2026-01-15T09:30:00.000Z');
  assert.equal(at('2026-07-15', '10:30', PRAGUE), '2026-07-15T08:30:00.000Z');
  assert.equal(at('2026-07-15', '10:30:15', PRAGUE), '2026-07-15T08:30:15.000Z');
  assert.equal(at('2026-11-14', '10:30', NEW_YORK), '2026-11-14T15:30:00.000Z');
  assert.equal(at('2026-07-04', '10:30', NEW_YORK), '2026-07-04T14:30:00.000Z');
  assert.equal(at('2026-11-14', '10:30', 'UTC'), '2026-11-14T10:30:00.000Z');
  assert.equal(at('2026-11-14', '10:30', 'Asia/Kolkata'), '2026-11-14T05:00:00.000Z');
});

test('every time of an ordinary day comes back as it was written', () => {
  for (const zone of [PRAGUE, NEW_YORK, 'Asia/Kolkata', 'Australia/Lord_Howe']) {
    for (const time of ['00:00:00', '03:15:09', '12:00:00', '23:59:59']) {
      const found = wallToUtc('2026-06-10', time, zone);
      assert.deepEqual(wallOf(found, zone), { date: '2026-06-10', time }, `${zone} ${time}`);
    }
  }
});

test('Prague, when summer time starts: a time that does not exist moves forward by the gap', () => {
  // On 29 March 2026, 02:00 CET becomes 03:00 CEST, at 01:00 UTC.
  assert.equal(at('2026-03-29', '01:59:59', PRAGUE), '2026-03-29T00:59:59.000Z');
  assert.equal(at('2026-03-29', '02:00', PRAGUE), '2026-03-29T01:00:00.000Z');
  assert.equal(at('2026-03-29', '02:30', PRAGUE), '2026-03-29T01:30:00.000Z');
  assert.equal(at('2026-03-29', '02:59:59', PRAGUE), '2026-03-29T01:59:59.000Z');
  assert.equal(at('2026-03-29', '03:00', PRAGUE), '2026-03-29T01:00:00.000Z');
  // 02:30 became 03:30 on the clocks.
  assert.deepEqual(wallOf(wallToUtc('2026-03-29', '02:30', PRAGUE), PRAGUE), { date: '2026-03-29', time: '03:30:00' });
});

test('Prague, when summer time ends: a time that exists twice is the first of the two', () => {
  // On 25 October 2026, 03:00 CEST becomes 02:00 CET, at 01:00 UTC: 02:00 to 02:59 are shown twice.
  assert.equal(at('2026-10-25', '01:59:59', PRAGUE), '2026-10-24T23:59:59.000Z');
  assert.equal(at('2026-10-25', '02:00', PRAGUE), '2026-10-25T00:00:00.000Z');
  assert.equal(at('2026-10-25', '02:30', PRAGUE), '2026-10-25T00:30:00.000Z');
  assert.equal(at('2026-10-25', '02:59:59', PRAGUE), '2026-10-25T00:59:59.000Z');
  assert.equal(at('2026-10-25', '03:00', PRAGUE), '2026-10-25T02:00:00.000Z');
});

test('New York, when summer time starts: 02:30 on 8 March is 03:30', () => {
  // At 07:00 UTC, 02:00 EST becomes 03:00 EDT.
  assert.equal(at('2026-03-08', '01:59:59', NEW_YORK), '2026-03-08T06:59:59.000Z');
  assert.equal(at('2026-03-08', '02:00', NEW_YORK), '2026-03-08T07:00:00.000Z');
  assert.equal(at('2026-03-08', '02:30', NEW_YORK), '2026-03-08T07:30:00.000Z');
  assert.equal(at('2026-03-08', '03:00', NEW_YORK), '2026-03-08T07:00:00.000Z');
  assert.deepEqual(wallOf(wallToUtc('2026-03-08', '02:30', NEW_YORK), NEW_YORK), { date: '2026-03-08', time: '03:30:00' });
});

test('New York, when summer time ends: 01:30 on 1 November is the first 01:30', () => {
  // At 06:00 UTC, 02:00 EDT becomes 01:00 EST: 01:00 to 01:59 are shown twice.
  assert.equal(at('2026-11-01', '00:59:59', NEW_YORK), '2026-11-01T04:59:59.000Z');
  assert.equal(at('2026-11-01', '01:00', NEW_YORK), '2026-11-01T05:00:00.000Z');
  assert.equal(at('2026-11-01', '01:30', NEW_YORK), '2026-11-01T05:30:00.000Z');
  assert.equal(at('2026-11-01', '01:59:59', NEW_YORK), '2026-11-01T05:59:59.000Z');
  assert.equal(at('2026-11-01', '02:00', NEW_YORK), '2026-11-01T07:00:00.000Z');
});

test('a gap of half an hour moves a time forward by half an hour', () => {
  // Lord Howe Island: at 02:00 on 4 October 2026, the clocks go to 02:30.
  assert.equal(at('2026-10-04', '02:15', 'Australia/Lord_Howe'), '2026-10-03T15:45:00.000Z');
  assert.deepEqual(wallOf(wallToUtc('2026-10-04', '02:15', 'Australia/Lord_Howe'), 'Australia/Lord_Howe'), { date: '2026-10-04', time: '02:45:00' });
  assert.equal(at('2026-10-04', '02:30', 'Australia/Lord_Howe'), '2026-10-03T15:30:00.000Z');
  assert.equal(at('2026-10-04', '01:59', 'Australia/Lord_Howe'), '2026-10-03T15:29:00.000Z');
});

test('a date or a time that is not one, and a zone that is not one, give null', () => {
  assert.equal(wallToUtc('2026-02-30', '10:00', PRAGUE), null);
  assert.equal(wallToUtc('2026-13-01', '10:00', PRAGUE), null);
  assert.equal(wallToUtc('2026-11-14', '24:00', PRAGUE), null);
  assert.equal(wallToUtc('2026-11-14', '12:60', PRAGUE), null);
  assert.equal(wallToUtc('2026-11-14', '12:00:60', PRAGUE), null);
  assert.equal(wallToUtc('2026-11-14', '9:00', PRAGUE), null);
  assert.equal(wallToUtc('14.11.2026', '09:00', PRAGUE), null);
  assert.equal(wallToUtc('2026-11-14', '', PRAGUE), null);
  assert.equal(wallToUtc('2026-11-14', '09:00', 'Mars/Olympus'), null);
  assert.equal(wallToUtc('2026-11-14', '09:00', ''), null);
  assert.equal(wallToUtc(undefined, '09:00', PRAGUE), null);
  assert.notEqual(wallToUtc('2028-02-29', '09:00', PRAGUE), null);
  assert.equal(wallToUtc('2027-02-29', '09:00', PRAGUE), null);
});

test('a day begins at its midnight, and a day of the clocks changing is 23 or 25 hours long', () => {
  assert.equal(dayStart('2026-11-14', PRAGUE), utc(2026, 10, 13, 23));
  assert.equal(dayStart('2026-11-14', NEW_YORK), utc(2026, 10, 14, 5));
  assert.equal(dayStart('2026-11-14', 'UTC'), utc(2026, 10, 14));
  const hours = (from, to, zone) => (dayStart(to, zone) - dayStart(from, zone)) / 3_600_000;
  assert.equal(hours('2026-03-29', '2026-03-30', PRAGUE), 23);
  assert.equal(hours('2026-10-25', '2026-10-26', PRAGUE), 25);
  assert.equal(hours('2026-03-08', '2026-03-09', NEW_YORK), 23);
  assert.equal(hours('2026-11-01', '2026-11-02', NEW_YORK), 25);
  assert.equal(hours('2026-06-10', '2026-06-11', PRAGUE), 24);
  assert.equal(dayStart('nope', PRAGUE), null);
});

test('a day whose midnight the clocks skip begins at the first minute they show', () => {
  // Beirut sets its clocks forward at midnight on 29 March 2026: 00:00 does not exist, and the day begins at 01:00.
  assert.equal(dayStart('2026-03-29', 'Asia/Beirut'), utc(2026, 2, 28, 22));
  assert.deepEqual(wallOf(dayStart('2026-03-29', 'Asia/Beirut'), 'Asia/Beirut'), { date: '2026-03-29', time: '01:00:00' });
});

test('addDays counts the calendar\'s days, across a month, a year and a leap day', () => {
  assert.equal(addDays('2026-11-14', 1), '2026-11-15');
  assert.equal(addDays('2026-11-30', 1), '2026-12-01');
  assert.equal(addDays('2026-12-31', 1), '2027-01-01');
  assert.equal(addDays('2028-02-28', 1), '2028-02-29');
  assert.equal(addDays('2027-02-28', 1), '2027-03-01');
  assert.equal(addDays('2026-01-01', -1), '2025-12-31');
  assert.equal(addDays('2026-11-14', 0), '2026-11-14');
  assert.equal(addDays('2026-11-14', 30), '2026-12-14');
  assert.equal(addDays('soon', 1), null);
});

test('offsetWords gives the offset of a zone at a time, as a saved line carries it', () => {
  assert.equal(offsetWords(utc(2026, 10, 14), PRAGUE), '+0100');
  assert.equal(offsetWords(utc(2026, 6, 14), PRAGUE), '+0200');
  assert.equal(offsetWords(utc(2026, 10, 14), NEW_YORK), '-0500');
  assert.equal(offsetWords(utc(2026, 6, 14), NEW_YORK), '-0400');
  assert.equal(offsetWords(utc(2026, 10, 14), 'Asia/Kolkata'), '+0530');
  assert.equal(offsetWords(utc(2026, 10, 14), 'Australia/Lord_Howe'), '+1100');
  assert.equal(offsetWords(utc(2026, 6, 14), 'Australia/Lord_Howe'), '+1030');
  assert.equal(offsetWords(utc(2026, 10, 14), 'UTC'), '+0000');
});

test('a record\'s time is read to the microsecond, and anything else is refused', () => {
  const found = timeOf('2026-11-14T09:30:00.123456Z');
  assert.deepEqual(found, { ms: utc(2026, 10, 14, 9, 30, 0, 123), seconds: utc(2026, 10, 14, 9, 30, 0) / 1000, micros: 123_456 });
  assert.equal(timeOf('2026-11-14T09:30:00Z').micros, 0);
  assert.equal(timeOf('2026-11-14T09:30:00.5Z').micros, 500_000);
  assert.equal(timeOf('2026-11-14T09:30:00.000007Z').micros, 7);
  assert.equal(timeOf('2026-11-14T09:30:00.123456789Z').micros, 123_456);
  for (const bad of ['2026-11-14T09:30:00', '2026-11-14 09:30:00Z', '2026-02-30T09:30:00Z', '2026-13-14T09:30:00Z', '2026-11-14T25:30:00Z', '2026-11-14T09:60:00Z', '', null, undefined, 5]) {
    assert.equal(timeOf(bad), null, String(bad));
  }
});

test('a record\'s time in each of the three zones, to the microsecond, with its offset', () => {
  const time = '2026-11-14T09:30:00.123456Z';
  assert.equal(stampOf(time, PRAGUE), '2026-11-14 10:30:00.123456 +0100');
  assert.equal(stampOf(time, NEW_YORK), '2026-11-14 04:30:00.123456 -0500');
  assert.equal(stampOf(time, 'UTC'), '2026-11-14 09:30:00.123456 +0000');
  assert.equal(stampOf('2026-07-14T09:30:00.000001Z', PRAGUE), '2026-07-14 11:30:00.000001 +0200');
  assert.equal(stampOf('2026-11-14T23:59:59.999999Z', PRAGUE), '2026-11-15 00:59:59.999999 +0100');
  assert.equal(stampOf('yesterday', PRAGUE), null);
});

test('a record made in the hour that is shown twice carries the offset that tells the two apart', () => {
  // 00:30 UTC and 01:30 UTC on 25 October 2026 are both 02:30 in Prague.
  assert.equal(stampOf('2026-10-25T00:30:00.000000Z', PRAGUE), '2026-10-25 02:30:00.000000 +0200');
  assert.equal(stampOf('2026-10-25T01:30:00.000000Z', PRAGUE), '2026-10-25 02:30:00.000000 +0100');
});

test('a day is put in words for the heading of a list, whatever zone the browser is in', () => {
  assert.equal(dayWords('2026-11-13'), 'Fri, Nov 13, 2026');
  assert.equal(dayWords('2026-01-01'), 'Thu, Jan 1, 2026');
  assert.equal(dayWords('2028-02-29'), 'Tue, Feb 29, 2028');
  assert.equal(dayWords('soon'), '');
  assert.equal(dayWords(undefined), '');
});
