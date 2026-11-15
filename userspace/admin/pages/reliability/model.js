// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Reliability page says (design 7.3), as plain values so that a test
// reaches every branch: the ranges and the days or months they hold, the
// events of each one (starts, errors in the log, updates, changes of the
// journal, crashes) and the table and the problems a day that count them,
// the day's timeline, the crash dumps and the alerts. What it says of starts,
// the failed start and one start is starts-model.js. The file that draws these
// is days.js.

import { formatBytes } from '../../lib/chart-model.js';
import {
  addDays, dayOf, dayStart, dayWords, timeOf,
} from '../../lib/when-model.js';
import { changeRow } from '../overview/change-model.js';
import {
  msWords, resultOf, startHref, tryNumbers, updatesOf, versionWords,
} from './starts-model.js';

/** `words` with its first letter in capitals. */
const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

/** `count` and its noun, with an s (or `many`) when it is not one. */
const counted = (count, one, many = `${one}s`) => `${count} ${count === 1 ? one : many}`;

// ---- ranges, days and months -------------------------------------------------

/** The ranges of the head: the word the address carries, the button's words, how a sentence names it, and what the page counts by. */
export const RANGES = Object.freeze([
  { id: '7d', label: '7 days', words: 'the last 7 days', unit: 'day', count: 7 },
  { id: '30d', label: '30 days', words: 'the last 30 days', unit: 'day', count: 30 },
  { id: '1y', label: '1 year', words: 'the last year', unit: 'month', count: 12 },
]);

/** The range the page starts with. */
export const DEFAULT_RANGE = '7d';

/** The range with this word, or the default one for any other word. */
export const rangeOf = (id) => RANGES.find((range) => range.id === id) ?? RANGES.find((range) => range.id === DEFAULT_RANGE);

const UTC = { timeZone: 'UTC' };
const dayShort = new Intl.DateTimeFormat('en-US', { ...UTC, month: 'short', day: 'numeric' });
const monthShort = new Intl.DateTimeFormat('en-US', { ...UTC, month: 'short', year: 'numeric' });
const monthLong = new Intl.DateTimeFormat('en-US', { ...UTC, month: 'long', year: 'numeric' });

/** The month `count` months after `month` ("2026-11"), before it when negative. */
const monthAfter = (month, count) => {
  const [year, number] = month.split('-').map(Number);
  return new Date(Date.UTC(year, number - 1 + count, 1)).toISOString().slice(0, 7);
};

/** A day of `zone` as a bucket: its key, the words of a column and of a heading, and its span in milliseconds. */
function dayBucket(day, zone) {
  const at = Date.parse(`${day}T00:00:00Z`);
  return {
    key: day, short: dayShort.format(at), words: dayWords(day), since: dayStart(day, zone), until: dayStart(addDays(day, 1), zone),
  };
}

/** A month of `zone` as a bucket. */
function monthBucket(month, zone) {
  const at = Date.parse(`${month}-01T00:00:00Z`);
  return {
    key: month, short: monthShort.format(at), words: monthLong.format(at), since: dayStart(`${month}-01`, zone), until: dayStart(`${monthAfter(month, 1)}-01`, zone),
  };
}

/**
 * The days (or on a year the months) that `range` counts, oldest first, the
 * last the one that holds `now` (milliseconds) in `zone`: each `{key,
 * short, words, since, until}`. `key` is "2026-11-14" or "2026-11";
 * `since` and `until` are Unix milliseconds, and `until` is the first moment
 * of the next one.
 */
export function bucketsOf(range, now, zone) {
  const today = dayOf(now, zone);
  const steps = Array.from({ length: range.count }, (_, at) => at + 1 - range.count);
  if (range.unit === 'month') return steps.map((step) => monthBucket(monthAfter(today.slice(0, 7), step), zone));
  return steps.map((step) => dayBucket(addDays(today, step), zone));
}

/** The index in `buckets` of the one that holds the time `ms`, or -1. */
export const bucketAt = (buckets, ms) => buckets.findIndex((bucket) => ms >= bucket.since && ms < bucket.until);

/** What the page counts by, in a title: "a day" or "a month". */
export const perWords = (range) => `a ${range.unit}`;

// ---- the events of each day --------------------------------------------------

/** The rows of the events table, in the board's order. */
export const KINDS = Object.freeze([
  { id: 'start', title: 'Starts' },
  { id: 'error', title: 'Services' },
  { id: 'update', title: 'Updates' },
  { id: 'change', title: 'Changes' },
  { id: 'crash', title: 'Crashes' },
]);

/** The most characters of an error's text that the day shows. */
const TEXT_MAX = 140;

/** The words of a start as an item of a day: what it did, and what the start knew of it. */
function startItem(start, tries) {
  const result = resultOf(start, tries);
  const title = {
    good: 'Started', failed: `Start failed${start.check ? `: ${start.check}` : ''}`, 'fell-back': `Went back to ${start.release ?? 'the earlier release'}`,
  }[start.result] ?? (start.result === 'bisect-try' ? result.words : 'Started');
  const ready = Number.isFinite(start.ready_ms) ? `Ready ${msWords(start.ready_ms)} after the start` : null;
  return {
    kind: 'start', at: start.at_ms, dot: result.dot, title, sub: [versionWords(start), ready].filter(Boolean).join(' · '), href: startHref(start.start), result: start.result,
  };
}

/** The record's source as the day names it: a service, the kernel, or a program. */
const sourceOf = (record) => record.name || record.from || 'unknown';

/** Whether a change of the journal was taken back, or took another back. */
const takenBack = (change) => change.how === 'undone' || change.action === 'changes.put-back';

/**
 * The events of what was read, as one list: each `{kind, at, dot, title,
 * sub, ...}` with `at` in Unix milliseconds. A source that was not read
 * (a locked console, an error) is null, and has no events. `starts` are
 * newest first; `records` are the log's records; `changes` the journal's
 * rows; `dumps` what `dumps list` gives.
 */
export function eventsOf({
  starts, records, changes, dumps,
}, time) {
  const events = [];
  const tries = tryNumbers(starts ?? []);
  for (const start of starts ?? []) {
    if (Number.isFinite(start.at_ms)) events.push(startItem(start, tries));
  }
  for (const update of updatesOf(starts ?? [])) {
    const start = starts.find((one) => one.start === update.start);
    if (Number.isFinite(start.at_ms)) {
      events.push({
        kind: 'update', at: start.at_ms, dot: 'info', title: `Updated to ${update.to}`, sub: `From ${update.from}`, href: startHref(start.start),
      });
    }
  }
  for (const record of records ?? []) {
    const at = timeOf(record.time)?.ms;
    if (at !== undefined) events.push({ kind: 'error', at, dot: 'warn', service: sourceOf(record), text: record.text });
  }
  for (const change of changes ?? []) {
    const row = changeRow(change, time);
    events.push({
      kind: 'change', at: change.at * 1000, dot: row.dot, title: row.title, sub: row.sub, id: change.id, failed: row.dot === 'bad', taken: takenBack(change),
    });
  }
  for (const dump of dumps ?? []) {
    if (!Number.isFinite(dump.at_ms)) continue;
    events.push({
      kind: 'crash', at: dump.at_ms, dot: 'bad', title: 'The kernel crashed', sub: dump.first_line ?? '',
    });
  }
  return events;
}

/** `events` by bucket: for each of `buckets`, an object with a list for each kind. */
export function groupsOf(buckets, events) {
  const groups = buckets.map(() => Object.fromEntries(KINDS.map((kind) => [kind.id, []])));
  for (const event of events) {
    const at = bucketAt(buckets, event.at);
    if (at >= 0) groups[at][event.kind].push(event);
  }
  return groups;
}

/** What one cell says: `{count, dot, words}`, empty for a bucket with nothing of the kind. */
function cellOf(kind, list) {
  const count = list.length;
  if (count === 0) return { count: 0, dot: null, words: '' };
  const howMany = (test) => list.filter(test).length;
  if (kind === 'start') {
    const failed = howMany((event) => event.result === 'failed');
    const back = howMany((event) => event.result === 'fell-back');
    const words = [counted(count, 'start'), failed > 0 && `${failed} failed`, back > 0 && `${back} fell back`].filter(Boolean).join(', ');
    return { count, dot: failed + back > 0 ? 'bad' : 'info', words };
  }
  if (kind === 'error') return { count, dot: 'warn', words: counted(count, 'error') };
  if (kind === 'update') return { count, dot: 'info', words: counted(count, 'update') };
  if (kind === 'crash') return { count, dot: 'bad', words: counted(count, 'crash', 'crashes') };
  const failed = howMany((event) => event.failed);
  const back = howMany((event) => event.taken);
  const words = [counted(count, 'change'), failed > 0 && `${failed} did not work`, back > 0 && `${back} taken back`].filter(Boolean).join(', ');
  return { count, dot: failed + back > 0 ? 'bad' : 'info', words };
}

/**
 * The events table: `{head, rows}`. `head` is the buckets, and each row
 * `{id, title, cells}` has a cell for each bucket. A kind that was not read
 * (`unread` lists them) has no row.
 */
export function tableOf(buckets, groups, unread = []) {
  const rows = KINDS.filter((kind) => !unread.includes(kind.id)).map((kind) => ({
    id: kind.id, title: kind.title, cells: groups.map((group) => cellOf(kind.id, group[kind.id])),
  }));
  return { head: buckets, rows };
}

/**
 * What goes wrong in each bucket: `[{key, short, words, failed, crashes,
 * errors, total}]`, the failed starts, the crashes and the errors in the
 * log, which the page counts in place of a stability figure (design 12).
 */
export function problemsOf(buckets, groups) {
  return buckets.map((bucket, at) => {
    const group = groups[at];
    const failed = group.start.filter((event) => event.result === 'failed').length;
    const crashes = group.crash.length;
    const errors = group.error.length;
    return {
      key: bucket.key, short: bucket.short, words: bucket.words, failed, crashes, errors, total: failed + crashes + errors,
    };
  });
}

/** The width and height of the bars' picture, in the units of its `viewBox`. */
export const BARS = Object.freeze({ width: 300, height: 72, gap: 0.25 });

/**
 * The bars of the problems a day: `{most, bars}`, a bar `{key, x, width,
 * height, total}` for each bucket, the tallest as high as the picture. A
 * bucket with no problem has a bar of no height.
 */
export function barsOf(problems) {
  const most = Math.max(0, ...problems.map((one) => one.total));
  const slot = BARS.width / problems.length;
  return {
    most,
    bars: problems.map((one, at) => ({
      key: one.key,
      x: Math.round((at * slot + (slot * BARS.gap) / 2) * 10) / 10,
      width: Math.round(slot * (1 - BARS.gap) * 10) / 10,
      height: most === 0 ? 0 : Math.round((one.total / most) * BARS.height * 10) / 10,
      total: one.total,
    })),
  };
}

/** The words of the problems chart for a screen reader: "Problems a day over the last 7 days: 8 in all, the worst being Sat, Nov 14, 2026 with 5". */
export function problemsWords(problems, range) {
  const head = `Problems ${perWords(range)} over ${range.words}`;
  const all = problems.reduce((sum, one) => sum + one.total, 0);
  if (all === 0) return `${head}: none`;
  const worst = problems.reduce((best, one) => (one.total > best.total ? one : best));
  return `${head}: ${all} in all, the worst being ${worst.words} with ${worst.total}`;
}

/** The most items of a day or month that the timeline lists. */
export const TIMELINE_MAX = 100;

/**
 * The timeline of one bucket's group, oldest first, so that the newest is
 * last: `{items, left}`, each item `{at, dot, title, sub, href}`. The
 * errors of a service are one item, at its last error, so that a service
 * that failed a hundred times is one line. Only the newest `TIMELINE_MAX`
 * are listed; `left` is how many older ones are not.
 */
export function timelineOf(group) {
  const items = [];
  for (const kind of ['start', 'update', 'change', 'crash']) {
    for (const event of group[kind]) items.push({ at: event.at, dot: event.dot, title: event.title, sub: event.sub, href: event.href ?? null });
  }
  const byService = new Map();
  for (const event of group.error) {
    const held = byService.get(event.service) ?? { count: 0, last: event };
    held.count += 1;
    if (event.at >= held.last.at) held.last = event;
    byService.set(event.service, held);
  }
  for (const [service, held] of byService) {
    const text = held.last.text.length > TEXT_MAX ? `${held.last.text.slice(0, TEXT_MAX)}…` : held.last.text;
    items.push({
      at: held.last.at, dot: 'warn', title: `${service}: ${counted(held.count, 'error')}`, sub: `The last: ${text}`, href: null,
    });
  }
  items.sort((a, b) => a.at - b.at);
  return { items: items.slice(-TIMELINE_MAX), left: Math.max(0, items.length - TIMELINE_MAX) };
}

// ---- crashes and alerts ------------------------------------------------------

/** The rows of the Crashes card, newest first: `{name, when, size, line, command}`. A dump with no time says so. */
export function dumpRows(dumps, time) {
  return [...dumps]
    .sort((a, b) => (b.at_ms ?? 0) - (a.at_ms ?? 0))
    .map((dump) => ({
      name: dump.name,
      when: Number.isFinite(dump.at_ms) ? time.when(dump.at_ms / 1000) : 'Time not known',
      size: Number.isFinite(dump.size) ? formatBytes(dump.size) : '',
      line: dump.first_line ?? '',
      command: `elevate dumps show ${dump.name}`,
    }));
}

/** How many alerts the card lists. */
export const ALERTS_SHOWN = 20;

const ALERT_STATES = Object.freeze({
  raised: 'Raised', cleared: 'Cleared', event: 'Told', test: 'Test',
});

/** The rows of the Alerts card from the history's lines, newest first: `{at, state, severity, dot, source, title}`. */
export function alertRows(lines, time) {
  return lines
    .filter((line) => Number.isFinite(line?.at) && line.alert)
    .sort((a, b) => b.at - a.at)
    .slice(0, ALERTS_SHOWN)
    .map((line) => ({
      when: time.when(line.at),
      state: ALERT_STATES[line.state] ?? capital(String(line.state ?? '')),
      severity: capital(String(line.alert.severity ?? '')),
      dot: line.alert.severity === 'critical' ? 'bad' : 'warn',
      source: line.source ?? '',
      title: line.alert.title ?? '',
    }));
}
