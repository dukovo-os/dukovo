// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// pages/logs/around-model.js: the step that holds a moment, the time
// `metrics.around` is given, the busiest services, what moved most against
// the hour before, and the timeline (design 6.4).

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  aroundAt, busiest, changeWords, marksOf, meanOf, moved, movedWords, serviceSeries, stepOf, timelineOf, TIMELINE_ITEMS, TOP,
} from '../pages/logs/around-model.js';

const AT = Date.UTC(2026, 10, 14, 3, 4) / 1000;

/** A series of `service.KIND{NAME}`, one point every `every` seconds from `from`, each with `value(index)`. */
const series = (kind, name, from, count, every, value) => ({
  series: `service.${kind}{${name}}`,
  points: Array.from({ length: count }, (_, index) => [from + index * every, typeof value === 'function' ? value(index) : value]),
});

/**
 * The answer of `metrics.around` for a step of `stepEvery` that begins at
 * `AT`, read at one point a minute: the hour before holds `before`, the step
 * holds `during`.
 */
function answerOf(name, kind, before, during, { stepEvery = 900 } = {}) {
  const hour = Array.from({ length: 60 }, (_, index) => [AT - 3600 + index * 60, before]);
  const step = Array.from({ length: stepEvery / 60 }, (_, index) => [AT + index * 60, during]);
  return { series: `service.${kind}{${name}}`, points: [...hour, ...step] };
}

const STEP = { from: AT, to: AT + 900 };

test('the step is the chart\'s point that holds the moment', () => {
  const times = [AT - 900, AT, AT + 900];
  assert.deepEqual(stepOf(AT, 900, times), { from: AT, to: AT + 900 });
  assert.deepEqual(stepOf(AT + 899, 900, times), { from: AT, to: AT + 900 });
  assert.deepEqual(stepOf(AT + 900, 900, times), { from: AT + 900, to: AT + 1800 });
  assert.deepEqual(stepOf(AT - 1, 900, times), { from: AT - 900, to: AT });
});

test('a moment with no point that holds it is in a whole step of the chart\'s length', () => {
  assert.deepEqual(stepOf(AT + 100, 900, []), { from: AT - (AT % 900), to: AT - (AT % 900) + 900 });
  // A gap in the history: the points that are there do not hold the moment.
  const found = stepOf(AT + 450, 900, [AT - 1800, AT + 3600]);
  assert.equal(found.to - found.from, 900);
  assert.ok(found.from <= AT + 450 && AT + 450 < found.to);
  assert.equal(found.from % 900, 0);
});

test('the read is given a time that holds the whole step, and never one that is ahead', () => {
  const now = AT + 10 * 86400;
  // 1 h and 24 h read at a point a minute: 10 minutes after the time.
  assert.equal(aroundAt({ from: AT, to: AT + 60 }, '1h', now), AT);
  assert.equal(aroundAt({ from: AT, to: AT + 900 }, '24h', now), AT + 300);
  // 7 d reads at 15 minutes: 2.5 hours after, which holds the 2 hours of the step.
  assert.equal(aroundAt({ from: AT, to: AT + 7200 }, '7d', now), AT);
  // 1 y reads at an hour: 10 hours after, so a day's step needs the time moved on by 14 hours.
  assert.equal(aroundAt({ from: AT, to: AT + 86400 }, '1y', now), AT + 86400 - 10 * 3600);
  // Each holds the step: the time plus the read's reach is at or past its end.
  for (const [range, every, each] of [['1h', 60, 60], ['24h', 900, 60], ['7d', 7200, 900], ['1y', 86400, 3600]]) {
    const at = aroundAt({ from: AT, to: AT + every }, range, now);
    assert.ok(at >= AT && at + 10 * each >= AT + every, range);
    // And it reads at least 55 minutes of the hour before the step (the 24 h chart's step of 15 minutes leaves 55).
    assert.ok(at - 60 * each <= AT - 3300, range);
  }
  // The step that is running now: the time is not ahead of the clock.
  assert.equal(aroundAt({ from: AT, to: AT + 900 }, '24h', AT + 100), AT + 100);
  assert.ok(Number.isInteger(aroundAt({ from: AT, to: AT + 900 }, '24h', AT + 100.7)));
  // A word that is no range reads as the 24 hours.
  assert.equal(aroundAt({ from: AT, to: AT + 900 }, 'soon', now), AT + 300);
});

test('service series are read from an answer, and nothing else is', () => {
  const found = serviceSeries([
    { series: 'cpu', points: [[1, 5]] },
    { series: 'service.cpu{nginx}', points: [[20, 3], [10, 1], [30, null], [40, 'x'], [50, Number.NaN], 'bad'] },
    { series: 'service.memory{php-fpm}', points: [[10, 100]] },
    { series: 'service.cpu{}', points: [[1, 1]] },
    { series: 'service.disk{vda}', points: [[1, 1]] },
    { series: 'service.memory{a{b}}', points: [] },
    null,
  ]);
  assert.deepEqual(found.map((one) => `${one.kind}:${one.service}`), ['cpu:nginx', 'memory:php-fpm', 'memory:a{b}']);
  assert.deepEqual(found[0].points, [{ at: 10, value: 1 }, { at: 20, value: 3 }]);
  assert.deepEqual(serviceSeries(null), []);
  assert.deepEqual(serviceSeries({ series: 'service.cpu{a}' }), []);
  assert.deepEqual(serviceSeries([{ series: 'service.cpu{a}' }])[0].points, []);
});

test('a mean is over the points of the step, which holds its start and not its end', () => {
  const points = [{ at: 0, value: 10 }, { at: 60, value: 20 }, { at: 120, value: 60 }];
  assert.equal(meanOf(points, 0, 120), 15);
  assert.equal(meanOf(points, 60, 121), 40);
  assert.equal(meanOf(points, 121, 500), null);
  assert.equal(meanOf([], 0, 100), null);
});

test('a service that went from 1 to 40 percent ranks above one that went from 50 to 60', () => {
  const found = moved(serviceSeries([
    answerOf('big', 'cpu', 50, 60),
    answerOf('small', 'cpu', 1, 40),
  ]), STEP);
  assert.deepEqual(found.map((row) => row.service), ['small', 'big']);
  assert.equal(found[0].words, 'from 1% to 40%');
  assert.equal(found[1].words, 'from 50% to 60%');
  assert.ok(found[0].score > found[1].score);
  assert.equal(found[0].name, 'small CPU');
});

test('the change is over the larger of the two, so a fall ranks as a rise does', () => {
  const found = moved(serviceSeries([
    answerOf('rises', 'cpu', 10, 20),
    answerOf('falls', 'cpu', 80, 10),
  ]), STEP);
  assert.deepEqual(found.map((row) => row.service), ['falls', 'rises']);
  assert.equal(found[0].words, 'from 80% to 10%');
  assert.equal(found[0].score, 70 / 80);
  assert.equal(found[1].score, 0.5);
});

test('memory is compared by its share too, and written in bytes', () => {
  const found = moved(serviceSeries([
    answerOf('postgres', 'memory', 1_900_000_000, 2_400_000_000),
    answerOf('nginx', 'cpu', 1, 4),
  ]), STEP);
  assert.deepEqual(found.map((row) => row.name), ['nginx CPU', 'postgres memory']);
  assert.equal(found[1].words, 'from 1.9 GB to 2.4 GB');
});

test('the mean of the step is against the mean of the hour before it, whatever the points between hold', () => {
  const points = [
    // An hour of 10 percent, with a spike inside it, then the step at 30.
    ...Array.from({ length: 60 }, (_, index) => [AT - 3600 + index * 60, index === 30 ? 100 : 10]),
    ...Array.from({ length: 15 }, (_, index) => [AT + index * 60, 30]),
    // After the step: the reading goes on, and is not part of it.
    [AT + 900, 99], [AT + 960, 99],
  ];
  const [row] = moved(serviceSeries([{ series: 'service.cpu{db}', points }]), STEP);
  assert.equal(row.during, 30);
  assert.ok(Math.abs(row.before - (59 * 10 + 100) / 60) < 1e-9);
});

test('series with gaps are compared by what they have', () => {
  const sparse = {
    series: 'service.cpu{sparse}',
    // Three points in the hour before, and two in the step.
    points: [[AT - 3000, 2], [AT - 1800, 4], [AT - 600, 6], [AT + 100, 30], [AT + 700, 50]],
  };
  const [row] = moved(serviceSeries([sparse]), STEP);
  assert.equal(row.before, 4);
  assert.equal(row.during, 40);
  assert.equal(row.words, 'from 4% to 40%');
  // A series that has nothing in the step is not ranked: nothing says how it did.
  const gone = { series: 'service.cpu{gone}', points: [[AT - 600, 90]] };
  assert.deepEqual(moved(serviceSeries([sparse, gone]), STEP).map((one) => one.service), ['sparse']);
  // A point with no value is no point.
  const hole = { series: 'service.cpu{hole}', points: [[AT - 600, 10], [AT + 60, null], [AT + 120, 20]] };
  assert.equal(moved(serviceSeries([hole]), STEP)[0].during, 20);
});

test('a service that began within the hour counts as nothing for the rest of it', () => {
  // steady has a point each minute of the hour; late began ten minutes
  // before the step, at 90 percent, and is at 90 in the step.
  const late = { series: 'service.cpu{late}', points: [...Array.from({ length: 10 }, (_, index) => [AT - 600 + index * 60, 90]), [AT, 90]] };
  const found = moved(serviceSeries([answerOf('steady', 'cpu', 20, 22), late]), STEP);
  assert.deepEqual(found.map((row) => row.service), ['late', 'steady']);
  assert.ok(Math.abs(found[0].before - (90 * 10) / 60) < 1e-9);
  assert.equal(found[0].words, 'from 15% to 90%');
});

test('a service with no numbers before the step is new, where the history has numbers for others', () => {
  const found = moved(serviceSeries([
    answerOf('steady', 'cpu', 20, 22),
    { series: 'service.cpu{fresh}', points: Array.from({ length: 15 }, (_, index) => [AT + index * 60, 40]) },
  ]), STEP);
  assert.deepEqual(found.map((row) => row.service), ['fresh', 'steady']);
  assert.equal(found[0].words, 'new, 40%');
  assert.equal(found[0].before, null);
  assert.equal(found[0].score, 1);
  // With no earlier numbers at all there is nothing to compare with.
  assert.deepEqual(moved(serviceSeries([{ series: 'service.cpu{fresh}', points: [[AT, 40]] }]), STEP), []);
});

test('there can be nothing to rank', () => {
  assert.deepEqual(moved([], STEP), []);
  assert.deepEqual(moved(serviceSeries(null), STEP), []);
  assert.deepEqual(moved(serviceSeries([{ series: 'service.cpu{a}', points: [] }]), STEP), []);
  // Nothing moved.
  assert.deepEqual(moved(serviceSeries([answerOf('flat', 'cpu', 12, 12), answerOf('also', 'memory', 5e8, 5e8)]), STEP), []);
  // Noise: a share that is large of almost nothing.
  assert.deepEqual(moved(serviceSeries([answerOf('idle', 'cpu', 0.1, 0.4), answerOf('small', 'memory', 100, 900_000)]), STEP), []);
  // From nothing to something is a move once it is more than noise.
  assert.deepEqual(moved(serviceSeries([answerOf('woke', 'cpu', 0, 12)]), STEP).map((row) => row.words), ['from 0% to 12%']);
});

test('the top five are the answer, most first, and a tie is in a fixed order', () => {
  const answer = Array.from({ length: 8 }, (_, index) => answerOf(`s${index}`, 'cpu', 10, 10 + (index + 1) * 5));
  const found = moved(serviceSeries(answer), STEP);
  assert.equal(found.length, TOP);
  assert.deepEqual(found.map((row) => row.service), ['s7', 's6', 's5', 's4', 's3']);
  assert.equal(moved(serviceSeries(answer), STEP, 2).length, 2);
  const ties = moved(serviceSeries([
    answerOf('b', 'cpu', 10, 20), answerOf('a', 'cpu', 10, 20), answerOf('m', 'memory', 1e9, 2e9),
  ]), STEP);
  assert.deepEqual(ties.map((row) => row.name), ['a CPU', 'b CPU', 'm memory']);
});

test('the words of a move', () => {
  assert.equal(movedWords({ kind: 'cpu', before: 1, during: 40 }), 'from 1% to 40%');
  assert.equal(movedWords({ kind: 'cpu', before: 0.5, during: 12.4 }), 'from 0.5% to 12%');
  assert.equal(movedWords({ kind: 'memory', before: 210_000_000, during: 1_600_000_000 }), 'from 210 MB to 1.6 GB');
  assert.equal(movedWords({ kind: 'memory', before: null, during: 64_000_000 }), 'new, 64 MB');
});

test('the busiest are the five with the most CPU and the five with the most memory, in one table', () => {
  const names = ['a', 'b', 'c', 'd', 'e', 'f', 'g'];
  const answer = [
    // CPU rises with the letter, memory falls with it.
    ...names.map((name, index) => series('cpu', name, AT, 15, 60, (index + 1) * 10)),
    ...names.map((name, index) => series('memory', name, AT, 15, 60, (names.length - index) * 1e8)),
  ];
  const rows = busiest(serviceSeries(answer), STEP);
  // g, f, e, d, c have the most CPU; a, b, c, d, e the most memory: the union is all seven.
  assert.equal(rows.length, 7);
  assert.deepEqual(rows.map((row) => row.service), ['g', 'f', 'e', 'd', 'c', 'b', 'a']);
  assert.deepEqual(rows[0], { service: 'g', cpu: 70, memory: 1e8 });
  const few = busiest(serviceSeries(answer), STEP, 2);
  assert.deepEqual(few.map((row) => row.service), ['g', 'f', 'b', 'a']);
});

test('the busiest service is the one with the most in the step, not at its edge, and a tie goes to the one with more memory, then to the name', () => {
  const rows = busiest(serviceSeries([
    // `late` is high only after the step ends.
    { series: 'service.cpu{late}', points: [[AT, 1], [AT + 900, 99]] },
    { series: 'service.cpu{zed}', points: [[AT, 5]] },
    { series: 'service.cpu{alpha}', points: [[AT, 5]] },
    { series: 'service.memory{zed}', points: [[AT, 7]] },
  ]), STEP);
  assert.deepEqual(rows.map((row) => row.service), ['zed', 'alpha', 'late']);
  assert.equal(rows[0].memory, 7);
  assert.equal(rows[1].memory, null);
  assert.deepEqual(busiest([], STEP), []);
});

test('a change is a few words and a mark of the chart', () => {
  const rows = [
    { id: 1, at: AT - 120, who: 'alice', how: 'console', title: 'Restart a service' },
    { id: 2, at: AT - 60, who: 'outside the console', how: 'by hand', title: null },
    { id: 3, at: AT, who: 'ci', how: 'token ci', action: 'service.reload' },
    { id: 4, at: AT + 60 },
    { id: 5, who: 'x' },
  ];
  assert.equal(changeWords(rows[0]), 'Restart a service by alice');
  assert.equal(changeWords(rows[1]), 'A file was changed by hand');
  assert.equal(changeWords(rows[2]), 'service.reload by ci');
  assert.equal(changeWords(rows[3]), 'A change');
  assert.equal(changeWords({ who: 'Spark', how: 'undone' }), 'A change being tried was undone by Spark');
  assert.deepEqual(marksOf(rows).map((mark) => mark.at), [AT - 120, AT - 60, AT, AT + 60]);
  assert.deepEqual(marksOf(rows)[0], { at: AT - 120, label: 'Restart a service by alice' });
  assert.deepEqual(marksOf(null), []);
});

const record = (seconds, level, text, more = {}) => ({
  time: new Date(seconds * 1000).toISOString().replace('Z', '123456Z').replace('.000123456Z', '.123456Z'), boot: 'b', seq: seconds, level, from: 'service', name: 'nginx', text, ...more,
});

test('the timeline has the journal\'s rows and the log\'s lines of the step and the one before, the newest last', () => {
  const changes = [
    { at: AT - 800, who: 'alice', how: 'console', title: 'Install an update', status: 0 },
    { at: AT + 200, who: 'alice', how: 'console', title: 'Restart a service', status: 1 },
    { at: AT - 5000, who: 'alice', how: 'console', title: 'Too early' },
    { at: AT + 900, who: 'alice', how: 'console', title: 'Too late' },
  ];
  const records = [
    record(AT - 100, 'warning', 'upstream timed out'),
    record(AT + 50, 'error', 'connect() failed', { from: 'kernel', name: 'kernel' }),
    record(AT - 5000, 'error', 'long ago'),
  ];
  const { items, more } = timelineOf({ changes, records }, { from: AT - 900, to: AT + 900 });
  assert.equal(more, 0);
  assert.deepEqual(items.map((item) => item.title), ['Install an update by alice', 'upstream timed out', 'connect() failed', 'Restart a service by alice']);
  assert.deepEqual(items.map((item) => item.dot), ['info', 'warn', 'bad', 'bad']);
  assert.equal(items[1].sub, 'warning from nginx');
  assert.equal(items[2].sub, 'error from the machine itself');
  assert.equal(items[3].sub, 'In the journal. It did not work.');
  assert.deepEqual(items.map((item) => item.kind), ['change', 'log', 'log', 'change']);
});

test('the timeline keeps the newest items when there are many, and says how many it left out', () => {
  const records = Array.from({ length: TIMELINE_ITEMS + 5 }, (_, index) => record(AT + index, 'warning', `line ${index}`));
  const { items, more } = timelineOf({ records }, { from: AT, to: AT + 100 });
  assert.equal(items.length, TIMELINE_ITEMS);
  assert.equal(more, 5);
  assert.equal(items.at(-1).title, `line ${TIMELINE_ITEMS + 4}`);
  assert.equal(items[0].title, 'line 5');
  assert.deepEqual(timelineOf({}, { from: 0, to: 10 }), { items: [], more: 0 });
  assert.equal(timelineOf({ records }, { from: AT, to: AT + 100 }, 3).items.length, TIMELINE_ITEMS);
});

test('a long line is cut in the timeline, a line with no readable time is left out', () => {
  const long = 'x'.repeat(500);
  const { items } = timelineOf({ records: [record(AT, 'error', long), { ...record(AT, 'error', 'no time'), time: 'soon' }] }, { from: AT - 10, to: AT + 10 });
  assert.equal(items.length, 1);
  assert.equal(items[0].title.length, 141);
  assert.ok(items[0].title.endsWith('…'));
});
