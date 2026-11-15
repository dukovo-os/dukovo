// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Sizes and a filling disk in words (design 3.5): each unit's edges, how
// long a time is roughly, and the projection of use over a month, rising,
// flat, falling, with gaps and with too little history.

import assert from 'node:assert/strict';
import test from 'node:test';

import { bytesWords, counted, enoughHistory, fullIn, MIN_DAYS, roughly } from '../lib/size-words.js';

const DAY = 86400;
const GB = 1e9;

/** `[{at, value}]` a day apart from the Unix second 1,000,000, one point for each of `values`. */
const days = (values) => values.map((value, at) => ({ at: 1_000_000 + at * DAY, value }));

test('a count has its noun, with an s when it is not one', () => {
  assert.equal(counted(1, 'file'), '1 file');
  assert.equal(counted(0, 'file'), '0 files');
  assert.equal(counted(92, 'file'), '92 files');
});

test('bytes are written in thousands, two places under 10 and none above', () => {
  assert.equal(bytesWords(0), '0 B');
  assert.equal(bytesWords(512), '512 B');
  assert.equal(bytesWords(999), '999 B');
  assert.equal(bytesWords(1000), '1 KB');
  assert.equal(bytesWords(1536), '1.54 KB');
  assert.equal(bytesWords(41_943_040), '42 MB');
  assert.equal(bytesWords(61 * GB), '61 GB');
  assert.equal(bytesWords(236 * GB), '236 GB');
  assert.equal(bytesWords(1.24e12), '1.24 TB');
  assert.equal(bytesWords(2e12), '2 TB');
  assert.equal(bytesWords(4e12), '4 TB');
  assert.equal(bytesWords(2_000_398_934_016), '2 TB');
  assert.equal(bytesWords(1.5e15), '1.5 PB');
});

test('a size that rounds up to the next unit is written in it', () => {
  assert.equal(bytesWords(999_499), '999 KB');
  assert.equal(bytesWords(999_500), '1 MB');
  assert.equal(bytesWords(999_500_000), '1 GB');
  assert.equal(bytesWords(9_996), '10 KB');
  assert.equal(bytesWords(9_994), '9.99 KB');
});

test('a count that is not a size is zero bytes', () => {
  assert.equal(bytesWords(-5), '0 B');
  assert.equal(bytesWords(null), '0 B');
  assert.equal(bytesWords(Number.NaN), '0 B');
  assert.equal(bytesWords(undefined), '0 B');
});

test('a length of time is rounded to the unit it is nearest to', () => {
  assert.equal(roughly(0), 'less than a minute');
  assert.equal(roughly(59), 'less than a minute');
  assert.equal(roughly(60), 'about 1 minute');
  assert.equal(roughly(40 * 60), 'about 40 minutes');
  assert.equal(roughly(3 * 3600), 'about 3 hours');
  assert.equal(roughly(10512), 'about 3 hours');
  assert.equal(roughly(35 * 3600), 'about 35 hours');
  assert.equal(roughly(2 * DAY), 'about 2 days');
  assert.equal(roughly(13 * DAY), 'about 13 days');
  assert.equal(roughly(14 * DAY), 'about 2 weeks');
  assert.equal(roughly(21 * DAY), 'about 3 weeks');
  assert.equal(roughly(55 * DAY), 'about 8 weeks');
  assert.equal(roughly(56 * DAY), 'about 2 months');
  assert.equal(roughly(213 * DAY), 'about 7 months');
  assert.equal(roughly(729 * DAY), 'about 24 months');
  assert.equal(roughly(731 * DAY), 'about 2 years');
  assert.equal(roughly(9 * 365 * DAY), 'about 9 years');
  assert.equal(roughly(11 * 365 * DAY), 'more than 10 years');
  assert.equal(roughly(Number.NaN), 'less than a minute');
});

test('use that rises by a steady amount gives the time to full', () => {
  // 3 GB a day: 580 GB left is 193 days, about 6 months.
  const points = days(Array.from({ length: 30 }, (_, at) => 1240 * GB - (29 - at) * 3 * GB));
  const full = fullIn(points, 1820 * GB);
  assert.equal(full.words, 'Full in about 6 months');
  assert.equal(full.days, 29);
  assert.ok(Math.abs(full.seconds - (580 * GB) / (3 * GB) * DAY) < 1);
});

test('the line is drawn through the last 30 days only', () => {
  // 60 days that fell by a lot, then 30 that rise by 10 GB a day.
  const values = [...Array.from({ length: 60 }, (_, at) => 900 * GB - at * 10 * GB), ...Array.from({ length: 30 }, (_, at) => 300 * GB + at * 10 * GB)];
  const full = fullIn(days(values), 1000 * GB);
  assert.ok(full);
  assert.equal(full.words, 'Full in about 6 weeks');
});

test('use that is flat or falls gives nothing', () => {
  assert.equal(fullIn(days(Array.from({ length: 20 }, () => 500 * GB)), 1000 * GB), null);
  assert.equal(fullIn(days(Array.from({ length: 20 }, (_, at) => 500 * GB - at * GB)), 1000 * GB), null);
});

test('a few days of history give nothing, and say so', () => {
  const six = days(Array.from({ length: 6 }, (_, at) => 100 * GB + at * GB));
  assert.equal(enoughHistory(six), false);
  assert.equal(fullIn(six, 1000 * GB), null);
  const eight = days(Array.from({ length: MIN_DAYS + 1 }, (_, at) => 100 * GB + at * GB));
  assert.equal(enoughHistory(eight), true);
  assert.ok(fullIn(eight, 1000 * GB));
  assert.equal(enoughHistory([]), false);
  assert.equal(enoughHistory(null), false);
  assert.equal(fullIn([], 1000 * GB), null);
});

test('a gap in the history does not bend the line', () => {
  // The same 3 GB a day with the middle 10 days missing.
  const all = Array.from({ length: 30 }, (_, at) => ({ at: 1_000_000 + at * DAY, value: 1000 * GB + at * 3 * GB }));
  const gap = all.filter((_, at) => at < 10 || at >= 20);
  const whole = fullIn(all, 1500 * GB);
  const broken = fullIn(gap, 1500 * GB);
  assert.equal(broken.words, whole.words);
  assert.ok(Math.abs(broken.seconds - whole.seconds) / whole.seconds < 0.001);
});

test('points that are not numbers are left out', () => {
  const points = days(Array.from({ length: 20 }, (_, at) => 100 * GB + at * 2 * GB));
  points[5] = { at: points[5].at, value: null };
  points.splice(8, 1, null);
  assert.ok(fullIn(points, 1000 * GB));
});

test('a filesystem already as big as its size is full now', () => {
  const full = fullIn(days(Array.from({ length: 10 }, (_, at) => 990 * GB + at * 2 * GB)), 1000 * GB);
  assert.equal(full.words, 'Full now');
  assert.equal(full.seconds, 0);
});

test('a size that is not known gives nothing', () => {
  const points = days(Array.from({ length: 20 }, (_, at) => 100 * GB + at * GB));
  assert.equal(fullIn(points, 0), null);
  assert.equal(fullIn(points, null), null);
});

test('a rise too small to count is more than ten years away', () => {
  const points = days(Array.from({ length: 20 }, (_, at) => 100 * GB + at * 1000));
  assert.equal(fullIn(points, 1000 * GB).words, 'Full in more than 10 years');
});
