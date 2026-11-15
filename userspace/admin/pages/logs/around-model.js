// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the view from a chart says about a moment (design 6.4), as plain
// values: the step that holds the moment, the time `metrics.around` is asked
// for, the services that were busiest in the step, the ones that moved most
// against the hour before it, and the timeline of the journal's rows and the
// log's worst lines. pages/logs/chart-view.js draws them.
//
// `metrics.around` answers `[{series, points: [[unix seconds, value]]}]`
// with the series `service.cpu{NAME}` and `service.memory{NAME}`, a point
// for each step of the read, shown at its step's start.

import { formatBytes, formatPercent } from '../../lib/chart-model.js';
import { timeOf } from '../../lib/when-model.js';

/** The services each table shows. */
export const TOP = 5;

/** The hour that the step is compared with, in seconds. */
export const HOUR = 3600;

/** The most that the timeline lists. */
export const TIMELINE_ITEMS = 12;

/** How long a point of the read is for each range, in seconds, and how many of them `metrics.around` reads before and after the time it is given. */
const READ = Object.freeze({ '1h': 60, '24h': 60, '7d': 900, '1y': 3600 });
const STEPS_BEFORE = 60;
const STEPS_AFTER = 10;

/**
 * A series that holds less than this in the step and in the hour before it
 * is noise, not a move: 0.1 to 0.4 percent of CPU would otherwise rank above
 * a real rise because the share is large.
 */
const FLOOR = Object.freeze({ cpu: 0.5, memory: 1_000_000 });

const KINDS = Object.freeze({
  cpu: { words: 'CPU', format: formatPercent },
  memory: { words: 'memory', format: formatBytes },
});

// ---- the moment ----------------------------------------------------------------

/**
 * The step that holds the moment `at` (Unix seconds): `{from, to}`. `every`
 * is the length of a point of the chart in seconds and `times` the times of
 * its points, which tell where a step begins; without a point that holds
 * `at`, the steps are whole multiples of `every`.
 */
export function stepOf(at, every, times = []) {
  const held = times.filter((time) => time <= at && at < time + every).at(-1);
  const from = held ?? Math.floor(at / every) * every;
  return { from, to: from + every };
}

/**
 * The time to give `metrics.around` so that what it reads holds the step:
 * it reads 60 of its own steps before the time and 10 after, so the time is
 * the step's start, or later when the step is longer than those 10 steps,
 * and never later than `now`, which the server does not accept. `range` is
 * the chart's range word and the step is `stepOf`'s.
 */
export function aroundAt(step, range, now) {
  const each = READ[range] ?? READ['24h'];
  return Math.floor(Math.min(now, Math.max(step.from, step.to - STEPS_AFTER * each)));
}

// ---- the services ---------------------------------------------------------------

const SERIES = /^service\.(cpu|memory)\{(.+)\}$/;

/**
 * The service series of an answer: `[{service, kind, points: [{at, value}]}]`,
 * `kind` being `cpu` or `memory`, the points oldest first. Anything that is
 * no service series, and a point that is no number, is left out.
 */
export function serviceSeries(answer) {
  const found = [];
  for (const one of Array.isArray(answer) ? answer : []) {
    const named = SERIES.exec(String(one?.series));
    if (!named) continue;
    const points = (Array.isArray(one.points) ? one.points : [])
      .filter((pair) => Array.isArray(pair) && Number.isFinite(pair[0]) && typeof pair[1] === 'number' && Number.isFinite(pair[1]))
      .map(([at, value]) => ({ at, value }))
      .sort((a, b) => a.at - b.at);
    found.push({ service: named[2], kind: named[1], points });
  }
  return found;
}

/** The mean of the points of `points` from `from` up to, not including, `to`; null when none is there. */
export function meanOf(points, from, to) {
  let sum = 0;
  let count = 0;
  for (const point of points) {
    if (point.at < from || point.at >= to) continue;
    sum += point.value;
    count += 1;
  }
  return count === 0 ? null : sum / count;
}

/** The rows `[{service, value}]` of the `count` services with the most of `kind` in `step`, most first; a tie is in the order of the names. */
function leaders(series, kind, step, count) {
  return series
    .filter((one) => one.kind === kind)
    .map((one) => ({ service: one.service, value: meanOf(one.points, step.from, step.to) }))
    .filter((row) => row.value !== null)
    .sort((a, b) => b.value - a.value || a.service.localeCompare(b.service))
    .slice(0, count);
}

/**
 * The table of "Busiest at": a row for each service that is among the
 * `count` with the most CPU or among the `count` with the most memory in
 * `step`, as `{service, cpu, memory}` (a number, or null where the service
 * has none in the step), the most CPU first and a tie in the order of the
 * memory and then the names.
 */
export function busiest(series, step, count = TOP) {
  const rows = new Map();
  for (const kind of ['cpu', 'memory']) {
    for (const { service } of leaders(series, kind, step, count)) {
      if (!rows.has(service)) rows.set(service, { service, cpu: null, memory: null });
    }
  }
  for (const one of series) {
    const row = rows.get(one.service);
    if (row) row[one.kind] = meanOf(one.points, step.from, step.to);
  }
  return [...rows.values()].sort((a, b) => (b.cpu ?? -1) - (a.cpu ?? -1) || (b.memory ?? -1) - (a.memory ?? -1) || a.service.localeCompare(b.service));
}

/** The words of one measure of a row of `moved`: "from 1% to 40%", or "new, 40%" for a service that had no numbers in the hour before. */
export function movedWords({ kind, before, during }) {
  const { format } = KINDS[kind];
  return before === null ? `new, ${format(during)}` : `from ${format(before)} to ${format(during)}`;
}

/**
 * The mean of `points` over the times of `kept` from `from` up to `to`, the
 * times at which the history has a point of any service: a kept time with no
 * point of this one counts as nothing, since the service was not running
 * then. Null when it has no point there.
 */
function meanWhileKept(points, from, to, kept) {
  const mean = meanOf(points, from, to);
  if (mean === null) return null;
  const own = points.filter((point) => point.at >= from && point.at < to).length;
  return (mean * own) / Math.max(own, kept);
}

/**
 * What moved most: for each service series, the mean over `step` against the
 * mean over the hour before it, ranked by the change over the larger of the
 * two, so that a service that went from 1 to 40 percent ranks above one that
 * went from 50 to 60. Gives the top `count` as `{service, kind, name, before,
 * during, score, words}`. A series with no point in the step is left out, as
 * is one that held less than `FLOOR` in both. A series with no point before
 * the step is new, and ranks first, but only where the history has numbers
 * before the step for some other series: with none, nothing is known to
 * compare with and nothing is ranked. The hour's mean counts the times the
 * history has numbers for other services and none for this one as nothing
 * (`meanWhileKept`): a service that began a minute before the step did not
 * run the whole hour.
 */
export function moved(series, step, count = TOP) {
  const hour = { from: step.from - HOUR, to: step.from };
  const kept = new Set(series.flatMap((one) => one.points.filter((point) => point.at >= hour.from && point.at < hour.to).map((point) => point.at))).size;
  const known = kept > 0;
  const rows = [];
  for (const one of series) {
    const during = meanOf(one.points, step.from, step.to);
    if (during === null) continue;
    const before = meanWhileKept(one.points, hour.from, hour.to, kept);
    if (before === null && !known) continue;
    const larger = Math.max(during, before ?? 0);
    if (larger < FLOOR[one.kind]) continue;
    const score = Math.abs(during - (before ?? 0)) / larger;
    if (score === 0) continue;
    rows.push({
      service: one.service, kind: one.kind, name: `${one.service} ${KINDS[one.kind].words}`, before, during, score, words: movedWords({ kind: one.kind, before, during }),
    });
  }
  const order = (kind) => (kind === 'cpu' ? 0 : 1);
  return rows
    .sort((a, b) => b.score - a.score || order(a.kind) - order(b.kind) || b.during - a.during || a.service.localeCompare(b.service))
    .slice(0, count);
}

// ---- the journal and the log -----------------------------------------------------

/** A change as a few words: "Restart a service by alice", or "A file was changed by hand". */
export function changeWords(change) {
  if (change.how === 'by hand') return 'A file was changed by hand';
  if (change.how === 'undone') return 'A change being tried was undone by Spark';
  const title = change.title ?? change.action ?? 'A change';
  return change.who ? `${title} by ${change.who}` : title;
}

/** The marks of the chart for the journal's rows: `[{at, label}]` with `at` in Unix seconds, the rows that have no time left out. */
export function marksOf(changes) {
  return (Array.isArray(changes) ? changes : [])
    .filter((change) => Number.isFinite(change?.at))
    .map((change) => ({ at: change.at, label: changeWords(change) }));
}

/** The most of a log line that the timeline shows. */
const LINE_WORDS = 140;

/**
 * The timeline of "What happened around": the journal's rows and the log's
 * lines (the ones at warning or worse) from `from` up to `to`, in seconds,
 * oldest first so that the newest is last, and the newest `limit` of them.
 * Each is `{at, kind, title, sub, dot}`; `dot` is `bad`, `warn` or `info`,
 * the colour on the rail. `more` is how many older ones were left out.
 */
export function timelineOf({ changes = [], records = [] }, { from, to, limit = TIMELINE_ITEMS }) {
  const items = [];
  for (const change of Array.isArray(changes) ? changes : []) {
    if (!Number.isFinite(change?.at) || change.at < from || change.at >= to) continue;
    const failed = Number.isInteger(change.status) && change.status !== 0;
    items.push({
      at: change.at, kind: 'change', title: changeWords(change), sub: failed ? 'In the journal. It did not work.' : 'In the journal', dot: failed ? 'bad' : 'info',
    });
  }
  for (const record of records) {
    const at = timeOf(record.time)?.seconds;
    if (at === undefined || at < from || at >= to) continue;
    const text = record.text.length > LINE_WORDS ? `${record.text.slice(0, LINE_WORDS)}…` : record.text;
    const where = record.from === 'kernel' ? 'the machine itself' : record.name;
    items.push({
      at, kind: 'log', title: text, sub: `${record.level} from ${where}`, dot: record.level === 'error' ? 'bad' : 'warn',
    });
  }
  // The sort is stable, so the journal's rows come before the log's lines of the same second.
  items.sort((a, b) => a.at - b.at);
  const more = Math.max(0, items.length - limit);
  return { items: items.slice(more), more };
}
