// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/chart-model.js: series summed over their instances, broken at gaps,
// peaks and averages (also of an empty series), the drawing's numbers, and
// the words of a chart's label in three time zones.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  CHARTS, combine, DEFAULT_RANGE, formatBits, formatBytes, formatPercent, formatRate, GAP_STEPS, modelOf, RANGES, rangeOf,
  labelOf, runs, shapes, statOf, summarize, tableOf, totalMemory, VIEW,
} from '../lib/chart-model.js';
import { makeTime } from '../lib/time.js';

/** Unix seconds of a UTC time. */
const utc = (year, month, day, hour = 0, minute = 0) => Date.UTC(year, month - 1, day, hour, minute) / 1000;

const NOW = utc(2026, 11, 14, 10, 12);
const range = rangeOf('24h');
const points = (...pairs) => pairs.map(([at, value]) => ({ at, value }));
const chart = (id) => CHARTS.find((one) => one.id === id);

test('the four ranges are the ones the route takes, and an unknown word is the 24 hours', () => {
  assert.deepEqual(RANGES.map((one) => one.id), ['1h', '24h', '7d', '1y']);
  assert.equal(DEFAULT_RANGE, '24h');
  assert.equal(rangeOf('7d').words, 'the last 7 days');
  assert.equal(rangeOf('2h').id, '24h');
  assert.equal(rangeOf(null).id, '24h');
  assert.deepEqual(RANGES.map((one) => one.every), [60, 900, 7200, 86400], 'the steps metrics.usage makes');
});

test('the instances of a series are added, and another series of the same start is not one', () => {
  const answer = [
    { series: 'disk.read{vda}', points: [[100, 5], [160, 6]] },
    { series: 'disk.read{vdb}', points: [[100, 1], [160, 2], [220, 3]] },
    { series: 'disk.write{vda}', points: [[100, 99]] },
    { series: 'disk.readahead', points: [[100, 99]] },
  ];
  assert.deepEqual(combine(answer, 'disk.read'), points([100, 6], [160, 8], [220, 3]));
  assert.deepEqual(combine([{ series: 'cpu', points: [[10, 1]] }, { series: 'cpu.user', points: [[10, 50]] }], 'cpu'), points([10, 1]));
});

test('a value the machine could not read is a gap, and an answer that is not a list is nothing', () => {
  const answer = [{ series: 'cpu', points: [[10, 1], [70, null], [130, 3], [190, 'x'], [250, Number.NaN]] }];
  assert.deepEqual(combine(answer, 'cpu'), points([10, 1], [130, 3]));
  const both = [{ series: 'net.rx{a}', points: [[10, null]] }, { series: 'net.rx{b}', points: [[10, 4]] }];
  assert.deepEqual(combine(both, 'net.rx'), points([10, 4]), 'one instance with a value is enough');
  for (const nothing of [null, undefined, {}, [], [{ series: 'cpu' }], 'x']) assert.deepEqual(combine(nothing, 'cpu'), []);
});

test('the points come oldest first whatever order the instances give them', () => {
  const answer = [{ series: 'cpu', points: [[300, 3], [100, 1], [200, 2]] }];
  assert.deepEqual(combine(answer, 'cpu').map((point) => point.at), [100, 200, 300]);
});

test('a line is broken where the history has a gap of more than three steps', () => {
  assert.equal(GAP_STEPS, 3);
  const every = 900;
  const steady = points([0, 1], [900, 1], [1800, 1], [2700, 1]);
  assert.equal(runs(steady, every).length, 1);
  const exactly = points([0, 1], [2700, 1]);
  assert.equal(runs(exactly, every).length, 1, 'three steps apart is not a gap');
  const gap = points([0, 1], [900, 1], [900 + 2701, 1], [900 + 2701 + 900, 1]);
  assert.deepEqual(runs(gap, every).map((one) => one.length), [2, 2]);
  const lone = points([0, 1], [100000, 1], [200000, 1]);
  assert.deepEqual(runs(lone, every).map((one) => one.length), [1, 1, 1]);
  assert.deepEqual(runs([], every), []);
});

test('the peak, the average and the newest value of a series', () => {
  const found = summarize(points([10, 2], [20, 9], [30, 4], [40, 5]));
  assert.deepEqual(found, { count: 4, average: 5, peak: { at: 20, value: 9 }, last: { at: 40, value: 5 } });
  assert.deepEqual(summarize(points([10, 7], [20, 7])).peak, { at: 10, value: 7 }, 'the earliest of equal peaks');
  assert.equal(summarize(points([10, 0], [20, 0])).average, 0);
});

test('an empty series has no peak, no average and nothing newest', () => {
  assert.deepEqual(summarize([]), { count: 0, average: null, peak: null, last: null });
  const model = modelOf(chart('cpu'), [], { range, now: NOW });
  assert.equal(model.empty, true);
  assert.equal(model.ceiling, 100);
  assert.deepEqual(shapes(model, model.lines[0]), []);
  assert.deepEqual(tableOf(model, makeTime('UTC', () => NOW * 1000)).rows, []);
});

test('the numbers are written in the units a person reads', () => {
  assert.equal(formatPercent(0), '0%');
  assert.equal(formatPercent(7.46), '7.5%');
  assert.equal(formatPercent(18.4), '18%');
  assert.equal(formatPercent(100), '100%');
  assert.equal(formatBytes(0), '0 B');
  assert.equal(formatBytes(512), '512 B');
  assert.equal(formatBytes(7_400_000_000), '7.4 GB');
  assert.equal(formatBytes(16_000_000_000), '16 GB');
  assert.equal(formatBytes(999_600), '1 MB', 'not 1000 KB');
  assert.equal(formatBytes(2_100_000), '2.1 MB');
  assert.equal(formatBytes(3e15), '3000 TB', 'the largest unit goes on');
  assert.equal(formatRate(2_100_000), '2.1 MB/s');
  assert.equal(formatBits(4_750_000), '38 Mbit/s');
  assert.equal(formatBits(0), '0 bit/s');
  assert.equal(formatBits(125), '1 kbit/s');
});

test('the memory the machine has is the kernel\'s count, else the last used and available added', () => {
  const answer = [
    { series: 'memory.used', points: [[10, 4e9], [20, 5e9]] },
    { series: 'memory.available', points: [[10, 12e9], [20, 11e9]] },
  ];
  assert.equal(totalMemory(answer, { memory: 16_000_000_000 }), 16_000_000_000);
  assert.equal(totalMemory(answer, { memory: null }), 16_000_000_000);
  assert.equal(totalMemory(answer, null), 16_000_000_000);
  assert.equal(totalMemory([], { memory: null }), null);
  assert.equal(totalMemory(answer, { memory: 0 }), 16_000_000_000, 'a count of none is not a count');
});

test('each chart has the scale of its kind: 0 to 100, the memory, or a little over the peak', () => {
  const answer = [
    { series: 'cpu', points: [[NOW - 100, 30]] },
    { series: 'memory.used', points: [[NOW - 100, 7e9]] },
    { series: 'disk.read{vda}', points: [[NOW - 100, 1000]] },
    { series: 'disk.write{vda}', points: [[NOW - 100, 4000]] },
  ];
  assert.equal(modelOf(chart('cpu'), answer, { range, now: NOW }).ceiling, 100);
  assert.equal(modelOf(chart('memory'), answer, { range, now: NOW, total: 16e9 }).ceiling, 16e9);
  assert.equal(modelOf(chart('memory'), answer, { range, now: NOW, total: null }).ceiling, 7e9 * 1.1);
  assert.equal(modelOf(chart('memory'), answer, { range, now: NOW, total: 5e9 }).ceiling, 7e9, 'never under the highest value');
  assert.equal(modelOf(chart('disk'), answer, { range, now: NOW }).ceiling, 4000 * 1.1);
  assert.equal(modelOf(chart('network'), answer, { range, now: NOW }).ceiling, 1, 'no peak at all');
});

test('a line is drawn inside the picture, from the oldest time to now, with its peak highest', () => {
  // A point every 15 minutes, as the 24 hour range gives, rising from 0 to 100.
  const steps = 96;
  const rising = Array.from({ length: steps + 1 }, (_, index) => [NOW - 86400 + index * 900, (index * 100) / steps]);
  const model = modelOf(chart('cpu'), [{ series: 'cpu', points: rising }], { range, now: NOW });
  const [shape] = shapes(model, model.lines[0]);
  const found = shape.points.split(' ').map((pair) => pair.split(',').map(Number));
  assert.equal(found.length, steps + 1);
  assert.equal(found[0][0], 0);
  assert.equal(found[steps][0], 300);
  assert.equal(found[steps / 2][0], 150);
  assert.equal(found[0][1], VIEW.height - VIEW.pad, 'zero is the bottom, kept off the edge');
  assert.equal(found[steps][1], VIEW.pad, '100 is the top, kept off the edge');
  assert.equal(found[steps / 2][1], VIEW.height / 2);
  for (const [x, y] of found) {
    assert.ok(x >= 0 && x <= VIEW.width && y >= 0 && y <= VIEW.height, `${x},${y}`);
  }
  assert.ok(shape.area.endsWith(`300,${VIEW.height} 0,${VIEW.height}`), shape.area);
});

test('a gap makes two shapes and a lone point makes a short flat stroke', () => {
  const answer = [{ series: 'cpu', points: [[NOW - 80000, 10], [NOW - 79100, 10], [NOW - 30000, 20], [NOW - 600, 5]] }];
  const model = modelOf(chart('cpu'), answer, { range, now: NOW });
  const found = shapes(model, model.lines[0]);
  assert.equal(found.length, 3);
  const lone = found[1].points.split(' ');
  assert.equal(lone.length, 2);
  assert.equal(lone[0].split(',')[1], lone[1].split(',')[1], 'flat');
  assert.notEqual(lone[0], lone[1]);
});

test('a point from before the range or after now is held at the edge, not outside the picture', () => {
  const model = modelOf(chart('cpu'), [{ series: 'cpu', points: [[NOW - 86400 - 400, 10], [NOW - 86400 + 500, 20], [NOW + 300, 150]] }], { range, now: NOW });
  const found = shapes(model, model.lines[0]);
  const pairs = found.flatMap((shape) => shape.points.split(' ')).map((pair) => pair.split(',').map(Number));
  assert.ok(pairs.every(([x, y]) => x >= 0 && x <= 300 && y >= 0 && y <= VIEW.height));
  assert.equal(pairs[0][0], 0, 'before the range is the left edge');
  assert.equal(pairs[pairs.length - 1][0], 300, 'after now is the right edge');
  assert.equal(pairs[pairs.length - 1][1], VIEW.pad, 'a value over the scale is held at the top');
});

// The same instant, 2:14 AM in UTC, is a different clock and day elsewhere.
const PEAK = utc(2026, 11, 14, 2, 14);
const answer = [{ series: 'cpu', points: [[PEAK - 900, 10], [PEAK, 72], [NOW - 900, 14]] }];
const cpu = (zone) => {
  const model = modelOf(chart('cpu'), answer, { range, now: NOW });
  return { model, time: makeTime(zone, () => NOW * 1000) };
};

test('the label says what the chart shows and when its peak was, in the person\'s zone: UTC', () => {
  const { model, time } = cpu('UTC');
  assert.equal(labelOf(model, time), 'CPU over the last 24 hours: average 32%, peak 72% at 2:14 AM');
});

test('the label in Prague, an hour ahead, says 3:14 AM', () => {
  const { model, time } = cpu('Europe/Prague');
  assert.equal(labelOf(model, time), 'CPU over the last 24 hours: average 32%, peak 72% at 3:14 AM');
});

test('the label in Denver, where the peak was the evening before, says so', () => {
  const { model, time } = cpu('America/Denver');
  assert.equal(labelOf(model, time), 'CPU over the last 24 hours: average 32%, peak 72% at Yesterday 7:14 PM');
});

test('a chart with no numbers says so in its label and its note', () => {
  const time = makeTime('UTC', () => NOW * 1000);
  const model = modelOf(chart('network'), [], { range: rangeOf('7d'), now: NOW });
  assert.equal(labelOf(model, time), 'Network in and out over the last 7 days: no numbers yet');
  assert.deepEqual(statOf(model, time), { value: '–', note: 'Network now · no numbers yet' });
});

test('a chart of two lines names each in its label, and says when one has no numbers', () => {
  const time = makeTime('UTC', () => NOW * 1000);
  const both = modelOf(chart('disk'), [
    { series: 'disk.read{vda}', points: [[PEAK, 2_100_000], [NOW - 900, 100_000]] },
    { series: 'disk.write{vda}', points: [[PEAK, 40_000]] },
  ], { range, now: NOW });
  assert.equal(labelOf(both, time),
    'Disk reads and writes over the last 24 hours: read: average 1.1 MB/s, peak 2.1 MB/s at 2:14 AM; written: average 40 KB/s, peak 40 KB/s at 2:14 AM');
  const one = modelOf(chart('disk'), [{ series: 'disk.read{vda}', points: [[PEAK, 5000]] }], { range, now: NOW });
  assert.match(labelOf(one, time), /; written: no numbers$/);
});

test('the words above a chart are the newest value and the peak', () => {
  const { model, time } = cpu('UTC');
  assert.deepEqual(statOf(model, time), { value: '14%', note: 'CPU now · peak 72% at 2:14 AM' });
});

test('the words above the memory chart say how much there is, and above two lines name the one that peaked', () => {
  const time = makeTime('UTC', () => NOW * 1000);
  const memory = modelOf(chart('memory'), [{ series: 'memory.used', points: [[PEAK, 7_000_000_000], [NOW - 900, 7_400_000_000]] }], { range, now: NOW, total: 16e9 });
  assert.deepEqual(statOf(memory, time), { value: '7.4 GB', note: 'Memory of 16 GB · average 7.2 GB' });
  const unknown = modelOf(chart('memory'), [{ series: 'memory.used', points: [[PEAK, 7e9]] }], { range, now: NOW });
  assert.deepEqual(statOf(unknown, time), { value: '7 GB', note: 'Memory used · peak 7 GB at 2:14 AM' });
  const net = modelOf(chart('network'), [
    { series: 'net.rx{eth0}', points: [[PEAK, 1_000_000], [NOW - 900, 500_000]] },
    { series: 'net.tx{eth0}', points: [[PEAK, 250_000], [NOW - 900, 250_000]] },
  ], { range, now: NOW });
  assert.deepEqual(statOf(net, time), { value: '6 Mbit/s', note: 'Network now · peak 8 Mbit/s in at 2:14 AM' });
});

test('the table has a row for each time, newest first, with an empty cell where a line has nothing', () => {
  const time = makeTime('UTC', () => NOW * 1000);
  const model = modelOf(chart('network'), [
    { series: 'net.rx{eth0}', points: [[PEAK, 125_000], [PEAK + 900, 250_000]] },
    { series: 'net.tx{eth0}', points: [[PEAK + 900, 125_000]] },
  ], { range, now: NOW });
  assert.deepEqual(tableOf(model, time), {
    head: ['Time', 'In', 'Out'],
    rows: [['2:29 AM', '2 Mbit/s', '1 Mbit/s'], ['2:14 AM', '1 Mbit/s', '']],
  });
});
