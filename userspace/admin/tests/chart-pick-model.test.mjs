// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/chart-pick-model.js: the point a pointer or a key picks on a chart, the
// words of a point in the person's zone, and where the journal's marks fall.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  CHARTS, modelOf, rangeOf, VIEW, xOf,
} from '../lib/chart-model.js';
import {
  indexOfTime, markPlaces, PICK_HINT, pickAt, pickLabel, pickWords, sliderName, step, timeAt, timesOf,
} from '../lib/chart-pick-model.js';
import { makeTime } from '../lib/time.js';

/** Unix seconds of a UTC time. */
const utc = (year, month, day, hour = 0, minute = 0) => Date.UTC(year, month - 1, day, hour, minute) / 1000;
const NOW = utc(2026, 11, 14, 12, 0);
const chart = (id) => CHARTS.find((one) => one.id === id);

/** The CPU chart over the last hour, with a point each minute from `first` to `last` minutes ago. */
function cpu(first = 59, last = 0, value = (minutesAgo) => 100 - minutesAgo) {
  const points = [];
  for (let minutesAgo = first; minutesAgo >= last; minutesAgo -= 1) points.push([NOW - minutesAgo * 60, value(minutesAgo)]);
  return modelOf(chart('cpu'), [{ series: 'cpu', points }], { range: rangeOf('1h'), now: NOW });
}

const time = makeTime('UTC', () => NOW * 1000);

test('the points of a chart are the times it has a value at, oldest first', () => {
  const model = cpu(2, 0);
  assert.deepEqual(timesOf(model), [NOW - 120, NOW - 60, NOW]);
  assert.equal(timeAt(model, 0), NOW - 120);
  assert.equal(timeAt(model, 2), NOW);
  assert.equal(timeAt(model, 3), null);
  assert.equal(timeAt(model, -1), null);
});

test('a chart of two lines has a point wherever either line has one', () => {
  const answer = [
    { series: 'disk.read{vda}', points: [[NOW - 120, 5], [NOW - 60, 6]] },
    { series: 'disk.write{vda}', points: [[NOW - 60, 1], [NOW, 2]] },
  ];
  const model = modelOf(chart('disk'), answer, { range: rangeOf('1h'), now: NOW });
  assert.deepEqual(timesOf(model), [NOW - 120, NOW - 60, NOW]);
});

test('a chart with no history has no point to pick', () => {
  const model = modelOf(chart('cpu'), [], { range: rangeOf('24h'), now: NOW });
  assert.deepEqual(timesOf(model), []);
  assert.equal(pickAt(model, 100), null);
  assert.equal(step(model, null, 'ArrowLeft'), null);
  assert.equal(step(model, null, 'Home'), null);
  assert.equal(step(model, 0, 'End'), null);
  assert.equal(indexOfTime(model, NOW), null);
  assert.equal(pickLabel(model, 0, time), '');
  assert.deepEqual(markPlaces(model, []), []);
});

test('a pointer at the left edge picks the oldest point and at the right edge the newest', () => {
  const model = cpu();
  assert.equal(pickAt(model, 0), 0);
  assert.equal(pickAt(model, VIEW.width), 59);
  // Beyond the picture, the end.
  assert.equal(pickAt(model, -40), 0);
  assert.equal(pickAt(model, VIEW.width + 400), 59);
});

test('a pointer between two points picks the nearer one, and the earlier when it is half way', () => {
  const model = cpu();
  // 60 minutes across 300 units: 5 units a minute, the first point at 5 and the last at 300.
  assert.equal(pickAt(model, xOf(model, NOW - 30 * 60)), 29);
  assert.equal(pickAt(model, xOf(model, NOW - 30 * 60) + 2.4), 29);
  assert.equal(pickAt(model, xOf(model, NOW - 30 * 60) + 2.6), 30);
  assert.equal(pickAt(model, xOf(model, NOW - 30 * 60) - 2.4), 29);
  assert.equal(pickAt(model, xOf(model, NOW - 30 * 60) - 2.6), 28);
  // Half way between the points at 29 and 30, to the second: the earlier.
  assert.equal(indexOfTime(model, NOW - 29.5 * 60), 29);
});

test('a pointer over a gap in the history picks the nearer side of it', () => {
  // Points for the first ten minutes and the last ten, with 40 minutes of nothing between.
  const points = [];
  for (let minutesAgo = 59; minutesAgo >= 50; minutesAgo -= 1) points.push([NOW - minutesAgo * 60, 1]);
  for (let minutesAgo = 9; minutesAgo >= 0; minutesAgo -= 1) points.push([NOW - minutesAgo * 60, 1]);
  const model = modelOf(chart('cpu'), [{ series: 'cpu', points }], { range: rangeOf('1h'), now: NOW });
  assert.equal(pickAt(model, xOf(model, NOW - 40 * 60)), 9, 'nearer the older side');
  assert.equal(pickAt(model, xOf(model, NOW - 20 * 60)), 10, 'nearer the newer side');
});

test('a chart with one point picks it from anywhere', () => {
  const model = cpu(5, 5);
  for (const x of [0, 150, 300]) assert.equal(pickAt(model, x), 0);
  for (const key of ['ArrowLeft', 'ArrowRight', 'PageUp', 'PageDown', 'Home', 'End']) assert.equal(step(model, 0, key), 0, key);
});

test('a pointer place that is not a number picks nothing', () => {
  const model = cpu();
  assert.equal(pickAt(model, Number.NaN), null);
  assert.equal(pickAt(model, undefined), null);
});

test('the arrows move one point, and stop at the ends', () => {
  const model = cpu();
  assert.equal(step(model, 10, 'ArrowRight'), 11);
  assert.equal(step(model, 10, 'ArrowLeft'), 9);
  assert.equal(step(model, 10, 'ArrowUp'), 11);
  assert.equal(step(model, 10, 'ArrowDown'), 9);
  assert.equal(step(model, 59, 'ArrowRight'), 59);
  assert.equal(step(model, 0, 'ArrowLeft'), 0);
});

test('Page Up and Page Down move ten points, and stop at the ends', () => {
  const model = cpu();
  assert.equal(step(model, 20, 'PageUp'), 30);
  assert.equal(step(model, 20, 'PageDown'), 10);
  assert.equal(step(model, 55, 'PageUp'), 59);
  assert.equal(step(model, 4, 'PageDown'), 0);
  assert.equal(step(model, 0, 'PageDown'), 0);
  assert.equal(step(model, 59, 'PageUp'), 59);
});

test('Home goes to the first point and End to the last, from anywhere', () => {
  const model = cpu();
  for (const from of [0, 17, 59, null]) {
    assert.equal(step(model, from, 'Home'), 0);
    assert.equal(step(model, from, 'End'), 59);
  }
});

test('with nothing picked yet, a key starts from the newest point', () => {
  const model = cpu();
  assert.equal(step(model, null, 'ArrowLeft'), 58);
  assert.equal(step(model, undefined, 'ArrowLeft'), 58);
  assert.equal(step(model, null, 'ArrowRight'), 59);
  assert.equal(step(model, null, 'PageDown'), 49);
});

test('a point that is past the end, from a chart that has shrunk, is held inside it', () => {
  const model = cpu(4, 0);
  assert.equal(step(model, 30, 'ArrowLeft'), 4);
  assert.equal(step(model, 30, 'ArrowRight'), 4);
  assert.equal(step(model, -3, 'ArrowRight'), 0);
});

test('a key that is not a slider\'s is left alone', () => {
  const model = cpu();
  for (const key of ['Enter', ' ', 'Tab', 'Escape', 'a', 'Shift', '', 'constructor', 'toString', undefined]) assert.equal(step(model, 5, key), null, String(key));
});

test('walking along the chart with the arrows reaches every point in order', () => {
  const model = cpu(9, 0);
  const seen = [step(model, null, 'Home')];
  while (seen[seen.length - 1] < 9) seen.push(step(model, seen[seen.length - 1], 'ArrowRight'));
  assert.deepEqual(seen, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
});

test('the words of a point are its time in the person\'s zone and its value', () => {
  const model = cpu();
  // The newest point, 12:00, has 100 - 0 = 100; the one 5 minutes before it 95.
  assert.equal(pickLabel(model, 59, time), '12:00 PM, 100%');
  assert.equal(pickLabel(model, 54, time), '11:55 AM, 95%');
  assert.equal(pickLabel(model, 99, time), '');
  const prague = makeTime('Europe/Prague', () => NOW * 1000);
  assert.equal(pickLabel(model, 59, prague), '1:00 PM, 100%');
});

test('a screen reader is told percent, not read a sign', () => {
  const model = cpu(10, 0, (minutesAgo) => 63 - minutesAgo / 2);
  assert.equal(pickWords(model, 10, time), '12:00 PM, 63 percent');
  assert.equal(pickLabel(model, 10, time), '12:00 PM, 63%');
  assert.equal(pickWords(model, 99, time), '');
});

test('a memory chart says its value in bytes, and a chart of two lines says each', () => {
  const memory = modelOf(chart('memory'), [{ series: 'memory.used', points: [[NOW, 3.5e9]] }], { range: rangeOf('1h'), now: NOW });
  assert.equal(pickWords(memory, 0, time), '12:00 PM, 3.5 GB');
  const answer = [
    { series: 'disk.read{vda}', points: [[NOW - 60, 1.2e6], [NOW, 2e6]] },
    { series: 'disk.write{vda}', points: [[NOW, 300e3]] },
  ];
  const disk = modelOf(chart('disk'), answer, { range: rangeOf('1h'), now: NOW });
  assert.equal(pickWords(disk, 1, time), '12:00 PM, read 2 MB/s, written 300 KB/s');
  assert.equal(pickWords(disk, 0, time), '11:59 AM, read 1.2 MB/s, written no number');
});

test('the slider is named for the chart, and the hint says the keys', () => {
  assert.equal(sliderName(cpu()), 'Choose a moment on the CPU chart');
  assert.equal(sliderName(modelOf(chart('memory'), [], { range: rangeOf('1h'), now: NOW })), 'Choose a moment on the Memory used chart');
  assert.equal(PICK_HINT, 'Arrow keys move along the chart; Enter chooses a moment.');
});

test('the point nearest to a time, for a mark that falls between points', () => {
  const model = cpu();
  assert.equal(indexOfTime(model, NOW), 59);
  assert.equal(indexOfTime(model, NOW - 59 * 60), 0);
  assert.equal(indexOfTime(model, NOW - 10 * 60 - 20), 49);
  assert.equal(indexOfTime(model, NOW - 10 * 60 - 40), 48);
  assert.equal(indexOfTime(model, NOW + 9999), 59);
  assert.equal(indexOfTime(model, NOW - 99_999), 0);
});

test('the marks inside the range are placed on the picture, oldest first, and the others are left out', () => {
  const model = cpu();
  const marks = [
    { at: NOW - 10 * 60, label: 'nginx restarted by alice' },
    { at: NOW - 50 * 60, label: 'update installed' },
    { at: NOW - 3 * 3600, label: 'too old' },
    { at: NOW + 3600, label: 'in the future' },
  ];
  const placed = markPlaces(model, marks);
  assert.deepEqual(placed.map((mark) => mark.label), ['update installed', 'nginx restarted by alice']);
  assert.equal(placed[0].x, xOf(model, NOW - 50 * 60));
  assert.ok(placed[0].x > 0 && placed[0].x < placed[1].x && placed[1].x < VIEW.width);
  assert.equal(placed[1].at, NOW - 10 * 60);
  assert.deepEqual(markPlaces(model, [{ at: NOW, label: 'now' }, { at: model.from, label: 'start' }]).map((mark) => mark.x), [0, VIEW.width]);
});

test('the x of a time is what the chart draws its points at', () => {
  const model = cpu();
  assert.equal(xOf(model, model.from), 0);
  assert.equal(xOf(model, model.to), VIEW.width);
  assert.equal(xOf(model, (model.from + model.to) / 2), VIEW.width / 2);
  assert.equal(xOf(model, model.from - 1000), 0);
  assert.equal(xOf(model, model.to + 1000), VIEW.width);
});
