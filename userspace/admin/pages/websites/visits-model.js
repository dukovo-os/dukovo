// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The chart of a site's visits, as plain values: the days `website list`
// counted turned into what lib/chart-model.js draws, the one chart and its 30
// days, and the words of a day. visits.js draws it; both are read when a
// site that has been counted is shown.

import { modelOf } from '../../lib/chart-model.js';
import { dayWords } from '../../lib/snapshot-words.js';

const DAY = 86_400;

/** The chart: visits, and under them the errors, which are the second line and drawn dashed. */
export const VISITS_CHART = Object.freeze({
  id: 'visits',
  name: 'Visits and errors',
  short: 'Visits',
  now: 'Visits and errors on the last day counted',
  column: 'Day (UTC)',
  kind: 'count',
  scale: 'peak',
  lines: [{ series: 'visits', label: 'Visits' }, { series: 'errors', label: 'Errors' }],
});

/** The range of the chart: a point for each day, 30 days that end where today begins. */
export const VISITS_RANGE = Object.freeze({
  id: '30d', label: '30 days', words: 'the last 30 days', span: 30 * DAY, every: DAY,
});

/** The start of the day `day` ("2026-11-15"), as the Unix seconds the chart takes, or null when it is not a day. */
export function dayStart(day) {
  if (typeof day !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(day)) return null;
  const at = Date.parse(`${day}T00:00:00Z`);
  return Number.isNaN(at) ? null : at / 1000;
}

/**
 * The days of `visits.days` (`[day, visits, errors, bytes]`) as the answer of
 * `metrics.usage` is shaped: a series for the visits and one for the errors,
 * with a point at the start of each day. A row that is not a day with two
 * numbers is left out.
 */
export function answerOf(visits) {
  const days = Array.isArray(visits?.days) ? visits.days : [];
  const series = (name, at) => ({
    series: name,
    points: days.flatMap((one) => {
      const start = Array.isArray(one) ? dayStart(one[0]) : null;
      return start !== null && Number.isFinite(one[at]) ? [[start, one[at]]] : [];
    }),
  });
  return [series('visits', 1), series('errors', 2)];
}

/** The model of the chart for `visits`, drawn at `now` (milliseconds since 1970) when the day is that of the UTC clock. */
export function visitsModel(visits, now) {
  return modelOf(VISITS_CHART, answerOf(visits), { range: VISITS_RANGE, now: Math.floor(now / 1000 / DAY) * DAY });
}

/** What the chart says of a time it was given: the UTC day, "Nov 14", since a log's days are counted by UTC. */
export const dayTime = (now) => ({ when: (at) => dayWords(at * 1000, 'UTC', now) });

/** What the chart says when some days were counted only as far as the limits went. */
export const PARTIAL_WORDS = 'Some days were counted only as far as the limits went, so their numbers are at least what is shown.';
