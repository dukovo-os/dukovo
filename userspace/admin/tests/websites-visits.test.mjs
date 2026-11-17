// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The chart of a site's visits (pages/websites/visits-model.js): the days
// `website list` counted turned into the series of a chart, the 30 days it
// covers, and the words of a day and of what the chart says of them.

import assert from 'node:assert/strict';
import test from 'node:test';

import { labelOf, statOf, tableOf } from '../lib/chart-model.js';
import {
  answerOf, dayStart, dayTime, PARTIAL_WORDS, VISITS_CHART, VISITS_RANGE, visitsModel,
} from '../pages/websites/visits-model.js';
import { NOW, visits } from './websites-fixtures.mjs';

const DAY = 86_400;
const TODAY = Date.UTC(2026, 10, 14) / 1000;

test('a day is the Unix time of its start in UTC, and anything else is none', () => {
  assert.equal(dayStart('2026-11-14'), TODAY);
  assert.equal(dayStart('1970-01-02'), DAY);
  for (const none of ['', '2026-11-1', '2026-13-01', '2026-02-30x', 'yesterday', null, undefined, 20261114]) assert.equal(dayStart(none), null, String(none));
});

test('the days become a series of visits and one of errors, a point at the start of each day', () => {
  const [seen, wrong] = answerOf(visits());
  assert.equal(seen.series, 'visits');
  assert.equal(wrong.series, 'errors');
  assert.equal(seen.points.length, 30);
  assert.deepEqual(seen.points[0], [TODAY - DAY, 1240]);
  assert.deepEqual(wrong.points[0], [TODAY - DAY, 3]);
  assert.deepEqual(seen.points.at(-1)[0], TODAY - 30 * DAY);
});

test('a row that is not a day is left out, a number that is not one is left out of its own series, and a site with no days has empty series', () => {
  const [seen, wrong] = answerOf({ days: [['2026-11-13', 10, 1, 0], ['soon', 5, 5, 0], ['2026-11-12', 'many', 2, 0], null, ['2026-11-11', 8]] });
  assert.deepEqual(seen.points, [[TODAY - DAY, 10], [TODAY - 3 * DAY, 8]]);
  assert.deepEqual(wrong.points, [[TODAY - DAY, 1], [TODAY - 2 * DAY, 2]]);
  for (const none of [null, undefined, {}, { days: 'no' }, { days: [] }]) assert.deepEqual(answerOf(none).map((one) => one.points), [[], []]);
});

test('the chart is the 30 days that end where today begins, a point for each day', () => {
  assert.equal(VISITS_RANGE.span, 30 * DAY);
  assert.equal(VISITS_RANGE.every, DAY);
  const model = visitsModel(visits(), NOW);
  assert.equal(model.to, TODAY, 'the right edge is the start of today, in UTC');
  assert.equal(model.from, TODAY - 30 * DAY, 'the oldest day is the left edge');
  assert.deepEqual(model.lines.map((line) => [line.label, line.points.length]), [['Visits', 30], ['Errors', 30]]);
  assert.equal(model.empty, false);
  assert.equal(model.lines[0].runs.length, 1, 'days that follow one another are one line');
  assert.equal(visitsModel(visits(), NOW + 5 * 3_600_000).to, TODAY, 'any time of the day has the same edge');
  assert.equal(visitsModel(visits(), NOW + DAY * 1000).to, TODAY + DAY);
});

test('the line breaks where three or more days were not counted, and holds over fewer', () => {
  const [newest, ...older] = visits().days;
  const runsAfter = (missing) => visitsModel({ days: [newest, ...older.slice(missing)] }, NOW).lines[0].runs.length;
  assert.equal(runsAfter(2), 1);
  assert.equal(runsAfter(3), 2);
});

test('no day counted is an empty chart', () => {
  assert.equal(visitsModel({ days: [] }, NOW).empty, true);
  assert.equal(visitsModel(null, NOW).empty, true);
});

test('the chart is named in words, its first column is the UTC day, and a time is written as that day', () => {
  assert.equal(VISITS_CHART.name, 'Visits and errors');
  assert.equal(VISITS_CHART.kind, 'count');
  assert.equal(VISITS_CHART.column, 'Day (UTC)');
  assert.deepEqual(VISITS_CHART.lines.map((line) => line.label), ['Visits', 'Errors']);
  const when = dayTime(NOW).when;
  assert.equal(when(TODAY - DAY), 'Nov 13');
  assert.equal(when(Date.UTC(2025, 11, 30) / 1000), 'Dec 30, 2025');
  assert.equal(when(TODAY - 30 * DAY), 'Oct 15');
});

test('what the chart says in words: the day before is the number, the peak is a day, and the table has a row for each day', () => {
  const model = visitsModel(visits(), NOW);
  const time = dayTime(NOW);
  assert.equal(statOf(model, time).value, '1,243', 'the visits and errors of the last day counted');
  assert.match(statOf(model, time).note, /^Visits and errors on the last day counted · peak 1,8\d\d visits at [A-Z][a-z]{2} \d+$/);
  assert.match(labelOf(model, time), /^Visits and errors over the last 30 days: visits: average 1,\d{3}, peak 1,8\d\d at /);
  const table = tableOf(model, time);
  assert.deepEqual(table.head, ['Day (UTC)', 'Visits', 'Errors']);
  assert.equal(table.rows.length, 30);
  assert.deepEqual(table.rows[0], ['Nov 13', '1,240', '3']);
  assert.deepEqual(table.rows.at(-1).slice(0, 1), ['Oct 15']);
});

test('the words that say some days are at least what is shown are the chart\'s', () => {
  assert.equal(PARTIAL_WORDS, 'Some days were counted only as far as the limits went, so their numbers are at least what is shown.');
});
