// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/time.js: the forms of every time the console shows, in three zones
// and across a change of summer time. "Now" is a fixed instant that each
// test sets, so no answer depends on the day the tests run.

import assert from 'node:assert/strict';
import test from 'node:test';

import { browserZone, isZone, makeTime, zones } from '../lib/time.js';

/** Unix seconds of the UTC time given as year, month (1 to 12), day, hour and minute. */
const utc = (year, month, day, hour = 0, minute = 0) => Date.UTC(year, month - 1, day, hour, minute) / 1000;

/** Formatters for `zone` where now is the UTC time given. */
const at = (zone, ...now) => makeTime(zone, () => utc(...now) * 1000);

test('a time today is its clock, and yesterday\'s says so', () => {
  // 3:12 AM on Nov 14 in Denver, which is UTC-7 in November.
  const denver = at('America/Denver', 2026, 11, 14, 10, 12);
  assert.equal(denver.when(utc(2026, 11, 14, 10, 6)), '3:06 AM');
  assert.equal(denver.when(utc(2026, 11, 14, 8, 0)), '1:00 AM');
  assert.equal(denver.when(utc(2026, 11, 14, 7, 0)), '12:00 AM');
  assert.equal(denver.when(utc(2026, 11, 14, 2, 40)), 'Yesterday 7:40 PM');
  assert.equal(denver.clock(utc(2026, 11, 14, 22, 5)), '3:05 PM');
});

test('an earlier day of the year has its day and clock, another year its date alone', () => {
  const denver = at('America/Denver', 2026, 11, 14, 10, 12);
  assert.equal(denver.when(utc(2026, 11, 11, 14, 40)), 'Nov 11 7:40 AM');
  assert.equal(denver.when(utc(2026, 1, 2, 20, 0)), 'Jan 2 1:00 PM');
  assert.equal(denver.when(utc(2025, 11, 11, 14, 40)), 'Nov 11, 2025');
  assert.equal(denver.when(utc(2026, 12, 25, 14, 40)), 'Dec 25 7:40 AM', 'a time ahead in the same year');
});

test('the same instant is a different day in another zone', () => {
  const now = [2026, 11, 14, 10, 12];
  const prague = at('Europe/Prague', ...now);
  assert.equal(prague.when(utc(2026, 11, 14, 10, 6)), '11:06 AM');
  // 11:30 PM on Nov 13 in Prague is 10:30 PM UTC, and 3:30 PM in Denver.
  assert.equal(prague.when(utc(2026, 11, 13, 22, 30)), 'Yesterday 11:30 PM');
  assert.equal(at('America/Denver', ...now).when(utc(2026, 11, 13, 22, 30)), 'Yesterday 3:30 PM');
  // 12:15 AM on Nov 14 in Prague is still Nov 13 in UTC.
  assert.equal(prague.when(utc(2026, 11, 13, 23, 15)), '12:15 AM');
  assert.equal(at('UTC', ...now).when(utc(2026, 11, 13, 23, 15)), 'Yesterday 11:15 PM');
  // Kolkata is half an hour off the hour.
  assert.equal(at('Asia/Kolkata', ...now).when(utc(2026, 11, 14, 10, 6)), '3:36 PM');
});

test('the start of the day is the person\'s midnight, also across a change of summer time', () => {
  // Prague put its clocks back at 3:00 AM on Oct 25: the day has 25 hours.
  const fall = at('Europe/Prague', 2026, 10, 25, 12, 0);
  assert.equal(fall.midnight(), utc(2026, 10, 24, 22, 0));
  assert.equal(fall.when(utc(2026, 10, 24, 21, 30)), 'Yesterday 11:30 PM');
  assert.equal(fall.when(utc(2026, 10, 24, 22, 30)), '12:30 AM');
  // Denver put its clocks forward at 2:00 AM on Mar 8: the day has 23 hours.
  const spring = at('America/Denver', 2026, 3, 8, 20, 0);
  assert.equal(spring.midnight(), utc(2026, 3, 8, 7, 0));
  assert.equal(spring.when(utc(2026, 3, 8, 6, 30)), 'Yesterday 11:30 PM');
  // A time before the change, on the day of it, is read with the old offset.
  assert.equal(spring.when(utc(2026, 3, 8, 8, 30)), '1:30 AM');
  assert.equal(spring.when(utc(2026, 3, 8, 9, 30)), '3:30 AM');
  assert.equal(at('Asia/Kolkata', 2026, 11, 14, 10, 12).midnight(), utc(2026, 11, 13, 18, 30));
  assert.equal(at('UTC', 2026, 11, 14, 10, 12).midnight(), utc(2026, 11, 14, 0, 0));
});

test('the start of another day is found the same way', () => {
  const prague = at('Europe/Prague', 2026, 11, 14, 10, 12);
  assert.equal(prague.midnight(utc(2026, 10, 25, 12, 0)), utc(2026, 10, 24, 22, 0));
  assert.equal(prague.midnight(utc(2026, 10, 26, 12, 0)), utc(2026, 10, 25, 23, 0));
});

test('a duration is in seconds under a minute, then minutes, hours and days', () => {
  const { duration } = at('UTC', 2026, 11, 14, 10, 12);
  assert.equal(duration(0), '0 s');
  assert.equal(duration(45), '45 s');
  assert.equal(duration(60), '1 min');
  assert.equal(duration(119), '2 min');
  assert.equal(duration(125), '2 min');
  assert.equal(duration(3 * 3600 + 5 * 60), '3 h 5 min');
  assert.equal(duration(3 * 3600), '3 h');
  assert.equal(duration(4 * 3600 + 52 * 60), '4 h 52 min');
  assert.equal(duration(2 * 86400 + 3 * 3600), '2 d 3 h');
  assert.equal(duration(2 * 86400), '2 d');
  assert.equal(duration(-5), '0 s');
});

test('relative words are for lists, and give way to the time after a week', () => {
  const now = utc(2026, 11, 14, 10, 12);
  const time = at('America/Denver', 2026, 11, 14, 10, 12);
  assert.equal(time.relative(now - 20), 'just now');
  assert.equal(time.relative(now - 60), '1 minute ago');
  assert.equal(time.relative(now - 4 * 60 - 30), '4 minutes ago');
  assert.equal(time.relative(now - 3600), '1 hour ago');
  assert.equal(time.relative(now - 5 * 3600), '5 hours ago');
  assert.equal(time.relative(now - 86400), '1 day ago');
  assert.equal(time.relative(now - 3 * 86400), '3 days ago');
  assert.equal(time.relative(now - 8 * 86400), 'Nov 6 3:12 AM');
  assert.equal(time.relative(now + 2 * 3600), 'in 2 hours');
  assert.equal(time.relative(now + 90), 'in 1 minute');
});

test('the whole time, for a title, has the date and the seconds', () => {
  const denver = at('America/Denver', 2026, 11, 14, 10, 12);
  assert.equal(denver.full(utc(2026, 11, 11, 14, 40) + 7), 'Nov 11, 2026, 7:40:07 AM');
});

test('no time has the narrow no-break space that newer ICU writes before AM and PM', () => {
  const time = at('Europe/Prague', 2026, 11, 14, 10, 12);
  for (const text of [time.clock(utc(2026, 11, 14, 10, 6)), time.when(utc(2026, 11, 13, 20, 0)), time.full(utc(2026, 11, 13, 20, 0))]) {
    assert.doesNotMatch(text, /[  ]/);
  }
});

test('a zone that is empty or that the browser does not know is the browser\'s own', () => {
  assert.equal(makeTime('Europe/Prague').zone, 'Europe/Prague');
  assert.equal(makeTime('').zone, browserZone());
  assert.equal(makeTime('Mars/Olympus_Mons').zone, browserZone());
  assert.equal(isZone('Europe/Prague'), true);
  assert.equal(isZone('UTC'), true);
  assert.equal(isZone(''), false);
  assert.equal(isZone('Mars/Olympus_Mons'), false);
  assert.equal(isZone(undefined), false);
});

test('the list of zones to choose from has UTC and the places the browser knows', () => {
  const all = zones();
  assert.ok(all.includes('UTC'));
  assert.ok(all.includes('Europe/Prague'));
  assert.ok(all.every(isZone));
});
