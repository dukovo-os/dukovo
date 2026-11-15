// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Scheduled tasks page and the Timer card of a service say (design
// 4.4 and 4.5), as plain values so that a test reaches every branch: a
// timer's schedule in words, the times of its next and last run, and the
// clock they are on. `service.timers` answers `[{service, schedule, next_ms,
// last_ms, last_result, zone}]`, `schedule` as the service's file writes
// it, the times in Unix milliseconds, and any of `schedule`, `next_ms`,
// `last_result` and `zone` may be null. The pages that draw them are
// scheduled.js and timer-card.js.

import { makeTime } from '../../lib/time.js';
import { serviceHref } from './needs.js';

/** `words` with its first letter in capitals. */
const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

/** `items` as one phrase: "Monday", "Monday and Friday", "1st, 5th and 9th". */
const listed = (items) => (items.length < 2 ? items.join('') : `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`);

/** `count` and its unit, with an s when it is not one. */
const counted = (count, unit) => `${count} ${unit}${Number(count) === 1 ? '' : 's'}`;

// ---- the schedule in words -----------------------------------------------

const UNITS = Object.freeze({
  w: 'week', d: 'day', h: 'hour', m: 'minute', s: 'second',
});

/** A duration of a timer, "1d12h", as "1 day and 12 hours"; null when it is none. */
function durationWords(text) {
  const found = [...text.matchAll(/(\d+)([wdhms])/g)];
  if (found.length === 0 || found.map((one) => one[0]).join('') !== text) return null;
  return listed(found.map(([, count, unit]) => counted(count, UNITS[unit])));
}

const DAYS = Object.freeze(['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday']);

/** `text` as a whole number up to `max`, or null. */
const number = (text, max) => (/^\d{1,2}$/.test(text) && Number(text) <= max ? Number(text) : null);

/** `text`, "3,15", as its numbers; null when one is not a number up to `max`. */
function numbersOf(text, max) {
  const found = text.split(',').map((one) => number(one, max));
  return found.includes(null) ? null : found;
}

/** 7 as "7th", 1 as "1st". */
function ordinal(day) {
  const suffix = day % 100 >= 11 && day % 100 <= 13 ? 'th' : { 1: 'st', 2: 'nd', 3: 'rd' }[day % 10] ?? 'th';
  return `${day}${suffix}`;
}

/** The clock of an hour and a minute, as the console writes it: "3:05 AM". */
const clockWords = (hour, minute) => `${hour % 12 === 0 ? 12 : hour % 12}:${String(minute).padStart(2, '0')} ${hour < 12 ? 'AM' : 'PM'}`;

/** The days of a crontab's weekday field, "1-5" or "1,3": "Monday to Friday", "Every Monday and Wednesday"; null when it is neither. */
function daysWords(text) {
  const range = /^([0-7])-([0-7])$/.exec(text);
  if (range) return `${DAYS[range[1] % 7]} to ${DAYS[range[2] % 7]}`;
  const days = numbersOf(text, 7);
  return days === null ? null : `Every ${listed(days.map((day) => DAYS[day % 7]))}`;
}

/** The five fields of a crontab line as words, for the shapes a person writes most; null for any other, which is then shown as written. */
function cronWords([minute, hour, day, month, weekday]) {
  if (month !== '*') return null;
  const past = number(minute, 59);
  const step = (text) => /^\*\/([2-9]|[1-5]\d)$/.exec(text)?.[1];
  if (hour === '*' && day === '*' && weekday === '*') {
    if (minute === '*') return 'Every minute';
    if (step(minute)) return `Every ${step(minute)} minutes`;
    if (past === null) return null;
    return past === 0 ? 'Every hour' : `Every hour, ${counted(past, 'minute')} past`;
  }
  if (past === null) return null;
  if (step(hour) && day === '*' && weekday === '*') return `Every ${step(hour)} hours${past === 0 ? '' : `, ${counted(past, 'minute')} past`}`;
  const hours = numbersOf(hour, 23);
  if (hours === null) return null;
  const at = listed(hours.map((one) => clockWords(one, past)));
  if (day === '*' && weekday === '*') return `Every day at ${at}`;
  if (day === '*') return daysWords(weekday) === null ? null : `${daysWords(weekday)} at ${at}`;
  const date = weekday === '*' ? number(day, 31) : null;
  return date === null || date < 1 ? null : `On the ${ordinal(date)} of every month at ${at}`;
}

const NAMED = Object.freeze({
  hourly: 'Every hour', daily: 'Every day', weekly: 'Every week', monthly: 'Every month', yearly: 'Every year',
});

/** One part of a schedule as words: a time of day, `every 6h` or `first 10m after boot`. A part this does not know stays as written. */
function partWords(part) {
  const every = /^every (\S+)$/.exec(part);
  const first = /^first (\S+) after boot$/.exec(part);
  if (every && durationWords(every[1])) {
    const words = durationWords(every[1]);
    return `Every ${/^1 \w+$/.test(words) ? words.slice(2) : words}`;
  }
  if (first && durationWords(first[1])) return `The first start comes ${durationWords(first[1])} after the machine starts`;
  if (Object.hasOwn(NAMED, part)) return NAMED[part];
  const fields = part.split(' ');
  return (fields.length === 5 ? cronWords(fields) : null) ?? part;
}

/**
 * A schedule as the file writes it ("0 3 * * *, every 6h, first 10m after
 * boot") in words: "Every day at 3:00 AM, every 6 hours, the first start
 * comes 10 minutes after the machine starts". Null when the file says none.
 */
export function scheduleWords(schedule) {
  if (typeof schedule !== 'string' || schedule === '') return null;
  const parts = schedule.split(', ').map(partWords);
  return capital(parts.map((part, at) => (at === 0 ? part : part.charAt(0).toLowerCase() + part.slice(1))).join(', '));
}

// ---- the rows ---------------------------------------------------------------

const RESULTS = Object.freeze({
  ok: { words: 'Worked', kind: 'ok' },
  failed: { words: 'Failed', kind: 'bad' },
});

/** What the page says under its table of what a dash and "Not known" mean. */
export const TIMER_NOTES = 'A dash under Next means the service runs from its timer now, or that no time comes within five years. Spark keeps the result of a run only since the machine started, so an earlier one is not known.';

/** The words of the Result column. */
export const resultWords = (result) => RESULTS[result] ?? { words: 'Not known', kind: 'off' };

/**
 * A time as `{words, full, iso}` for the clock `clock` is on, with `zone`
 * after the words when the clock is not the person's; null when there is
 * no time.
 */
function stampOf(ms, clock, zone) {
  if (!Number.isFinite(ms)) return null;
  return {
    words: zone ? `${clock.when(ms / 1000)} ${zone}` : clock.when(ms / 1000),
    full: zone ? `${clock.full(ms / 1000)} ${zone}` : clock.full(ms / 1000),
    iso: new Date(ms).toISOString(),
  };
}

/**
 * The rows of the Scheduled tasks table, soonest next run first (a timer with
 * none last), each `{service, href, when, written, next, last, result,
 * server}`: `next` and `last` as `stampOf` gives them, and `server` true when
 * they are on a clock that is not the person's, since a timer runs by its
 * own. `time` is the person's formatters and `nowMs` the browser's time.
 */
export function timerRows(timers, { time, nowMs }) {
  const clocks = new Map();
  const clockOf = (zone) => {
    if (!clocks.has(zone)) clocks.set(zone, makeTime(zone, () => nowMs));
    return clocks.get(zone);
  };
  const rows = (Array.isArray(timers) ? timers : []).map((timer) => {
    const server = typeof timer.zone === 'string' && timer.zone !== '' && timer.zone !== time.zone;
    const clock = server ? clockOf(timer.zone) : time;
    const words = scheduleWords(timer.schedule);
    return {
      service: timer.service,
      href: serviceHref(timer.service),
      when: words ?? 'Its file does not read',
      written: words !== null && words !== timer.schedule ? timer.schedule : null,
      next: stampOf(timer.next_ms, clock, server ? timer.zone : null),
      last: stampOf(timer.last_ms, clock, server ? timer.zone : null),
      result: resultWords(timer.last_result),
      server,
      order: Number.isFinite(timer.next_ms) ? timer.next_ms : Infinity,
    };
  });
  rows.sort((a, b) => (a.order === b.order ? a.service.localeCompare(b.service) : a.order - b.order));
  return rows.map(({ order, ...row }) => row);
}

/** The column headers of the table: Next and Last say whose clock they are on when any row is not the person's. */
export function timerHeaders(rows) {
  const note = rows.some((row) => row.server) ? ', on the server\'s clock' : '';
  return ['Task', 'When', `Next${note}`, `Last${note}`, 'Result'];
}

/** The rows of the Timer card from the row of the service in `timerRows`: `[{label, value}]`, the schedule first. */
export function timerCardRows(row) {
  return [
    { label: 'When', value: row.written ? `${row.when} (${row.written})` : row.when },
    { label: 'Next', value: row.next?.words ?? '–' },
    { label: 'Last', value: row.last?.words ?? 'Never' },
    { label: 'Result', value: row.result.words },
  ];
}
