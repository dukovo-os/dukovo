// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const calendar = require('./calendar.js');

const day = (text) => new Date(`${text}T00:00:00Z`);
const shown = (date) => date && calendar.formatDay(date);

test('the second Tuesday is found in months that start on each day of the week', () => {
  // 2026-09-01 is a Tuesday, 2026-11-01 a Sunday, 2027-01-01 a Friday and
  // 2027-05-01 a Saturday; the second Tuesday is never before the 8th.
  const second = (year, month) => shown(calendar.secondTuesday(year, month));
  assert.equal(second(2026, 8), '2026-09-08');
  assert.equal(second(2026, 10), '2026-11-10');
  assert.equal(second(2026, 11), '2026-12-08');
  assert.equal(second(2027, 0), '2027-01-12');
  assert.equal(second(2027, 4), '2027-05-11');
  for (let month = 0; month < 12; month += 1) {
    const found = calendar.secondTuesday(2027, month);
    assert.equal(found.getUTCDay(), 2);
    assert.ok(found.getUTCDate() >= 8 && found.getUTCDate() <= 14);
  }
});

test('a leap year has its February second Tuesday, and its cut day on the leap day', () => {
  assert.equal(shown(calendar.secondTuesday(2028, 1)), '2028-02-08');
  // 2028-03-14 is the release day, and 14 days before it is February 29.
  assert.equal(shown(calendar.secondTuesday(2028, 2)), '2028-03-14');
  assert.equal(shown(calendar.cutDayOf(day('2028-03-14'))), '2028-02-29');
  assert.equal(shown(calendar.releaseCutOn(day('2028-02-29'))), '2028-03-14');
  // In a year that is not a leap year, the same step lands on February 28.
  assert.equal(shown(calendar.cutDayOf(day('2027-03-09'))), '2027-02-23');
});

test('the cut day of a January release is in the December before', () => {
  assert.equal(shown(calendar.cutDayOf(day('2027-01-12'))), '2026-12-29');
  assert.equal(shown(calendar.releaseCutOn(day('2026-12-29'))), '2027-01-12');
  assert.equal(shown(calendar.nextCutDay(day('2026-12-10'))), '2026-12-29');
});

test('only the cut days cut, and only release days release', () => {
  assert.equal(calendar.releaseCutOn(day('2026-11-23')), null);
  assert.equal(shown(calendar.releaseCutOn(day('2026-11-24'))), '2026-12-08');
  assert.equal(calendar.releaseCutOn(day('2026-11-25')), null);
  assert.equal(calendar.isReleaseDay(day('2026-12-08')), true);
  assert.equal(calendar.isReleaseDay(day('2026-12-15')), false);
  assert.equal(calendar.isReleaseDay(day('2026-12-09')), false);
});

test('the time of day does not move the day', () => {
  assert.equal(calendar.isReleaseDay(new Date('2026-12-08T23:59:59.999Z')), true);
  assert.equal(calendar.isReleaseDay(new Date('2026-12-09T00:00:00Z')), false);
  assert.equal(shown(calendar.utcDay(new Date('2026-12-08T23:59:59Z'))), '2026-12-08');
  assert.equal(shown(calendar.releaseCutOn(new Date('2026-11-24T18:30:00Z'))), '2026-12-08');
});

test('the next release day is after today, and the one on or after is today when it is one', () => {
  assert.equal(shown(calendar.nextReleaseDay(day('2026-12-08'))), '2027-01-12');
  assert.equal(shown(calendar.releaseDayOnOrAfter(day('2026-12-08'))), '2026-12-08');
  assert.equal(shown(calendar.nextReleaseDay(day('2026-12-07'))), '2026-12-08');
  assert.equal(shown(calendar.nextReleaseDay(day('2026-12-09'))), '2027-01-12');
  // From the 15th of November to the second Tuesday of December.
  assert.equal(shown(calendar.nextReleaseDay(day('2026-11-15'))), '2026-12-08');
  // A release day late in the year crosses into the next year.
  assert.equal(shown(calendar.nextReleaseDay(day('2027-12-14'))), '2028-01-11');
});

test('the next cut day is today on a cut day and the following one after', () => {
  assert.equal(shown(calendar.nextCutDay(day('2026-11-24'))), '2026-11-24');
  assert.equal(shown(calendar.nextCutDay(day('2026-11-25'))), '2026-12-29');
  assert.equal(shown(calendar.nextCutDay(day('2026-11-15'))), '2026-11-24');
});

test('a day ends at its last millisecond', () => {
  const end = calendar.endOfDay(new Date('2026-12-08T10:15:00Z'));
  assert.equal(end.toISOString(), '2026-12-08T23:59:59.999Z');
  assert.equal(calendar.endOfDay(new Date('2028-02-29T00:00:00Z')).toISOString(), '2028-02-29T23:59:59.999Z');
  assert.equal(calendar.endOfDay(new Date('2026-12-31T12:00:00Z')).toISOString(), '2026-12-31T23:59:59.999Z');
});

test('days and times are written in UTC', () => {
  assert.equal(calendar.formatDay(new Date('2026-12-08T23:59:59Z')), '2026-12-08');
  assert.equal(calendar.formatTime(new Date('2026-12-08T09:05:59Z')), '2026-12-08 09:05 UTC');
});

test('a date is read only when it exists', () => {
  assert.equal(shown(calendar.parseDay('2028-02-29')), '2028-02-29');
  assert.equal(calendar.parseDay('2027-02-29'), null);
  assert.equal(calendar.parseDay('2026-13-01'), null);
  assert.equal(calendar.parseDay('2026-12-8'), null);
  assert.equal(calendar.parseDay('tomorrow'), null);
  assert.equal(calendar.parseDay(''), null);
});
