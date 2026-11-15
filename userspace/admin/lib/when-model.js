// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Times for the Logs page and the pages that link to it (design 5.2): the
// ranges it offers, a time on a wall in a zone turned into UTC across summer
// time, the days of a zone, and the words of a time that log(1) and
// metrics(1) read. Pure: nothing here touches the page.

import { isZone, offsetOf, partsOf } from './time.js';

const SECOND = 1000;
const DAY = 86_400_000;

/**
 * The ranges of the When filter, in the order it lists them: the word the
 * address carries, the words of the option, and how long a relative range
 * is in seconds (null for `today`, which starts at midnight, and `custom`).
 */
export const RANGES = Object.freeze([
  { id: '15m', label: 'Last 15 minutes', seconds: 15 * 60 },
  { id: '1h', label: 'Last hour', seconds: 3600 },
  { id: '24h', label: 'Last 24 hours', seconds: 86_400 },
  { id: '7d', label: 'Last 7 days', seconds: 7 * 86_400 },
  { id: 'today', label: 'Today', seconds: null },
  { id: 'custom', label: 'Custom range', seconds: null },
]);

/** The range with this word, or undefined. */
export const rangeById = (id) => RANGES.find((range) => range.id === id);

const pad = (number, size = 2) => String(number).padStart(size, '0');

/** `ms` in the form log(1) and metrics(1) read, to the second: "2026-11-14 09:30:00 UTC". */
export function utcWords(ms) {
  return `${new Date(Math.floor(ms / SECOND) * SECOND).toISOString().slice(0, 19).replace('T', ' ')} UTC`;
}

/** The date and the clock of `ms` in `zone`: `{date: '2026-11-14', time: '10:30:00'}`. */
export function wallOf(ms, zone) {
  const found = partsOf(zone, ms);
  return {
    date: `${pad(found.year, 4)}-${pad(found.month)}-${pad(found.day)}`,
    time: `${pad(found.hour)}:${pad(found.minute)}:${pad(found.second)}`,
  };
}

/** The day, "2026-11-14", that `ms` falls in, in `zone`. */
export const dayOf = (ms, zone) => wallOf(ms, zone).date;

const DATE = /^(\d{4})-(\d{2})-(\d{2})$/;
const CLOCK = /^(\d{2}):(\d{2})(?::(\d{2}))?$/;

/**
 * The Unix time in milliseconds of `date` ("2026-11-14") and `time`
 * ("09:30" or "09:30:15") on the clocks of `zone`, or null when they are not
 * a date and a time, or `zone` is not one. A time that the clocks skip when
 * summer time starts moves forward by the gap (02:30 where 02:00 becomes
 * 03:00 is 03:30); one they show twice when it ends is the first of the two.
 */
export function wallToUtc(date, time, zone) {
  const day = DATE.exec(date);
  const clock = CLOCK.exec(time);
  if (!day || !clock || !isZone(zone)) return null;
  const [year, month, of] = [day[1], day[2], day[3]].map(Number);
  const [hour, minute, second] = [clock[1], clock[2], clock[3] ?? '0'].map(Number);
  const naive = Date.UTC(year, month - 1, of, hour, minute, second);
  // Date.UTC carries a 30th of February into March, and a 24th hour into the next day: neither is a time.
  const back = new Date(naive);
  if (back.getUTCFullYear() !== year || back.getUTCMonth() !== month - 1 || back.getUTCDate() !== of
    || back.getUTCHours() !== hour || back.getUTCMinutes() !== minute) return null;
  // The offsets of the days either side hold every offset that the wall time can have.
  const before = offsetOf(zone, naive - DAY);
  const after = offsetOf(zone, naive + DAY);
  const fits = [...new Set([before, after])]
    .filter((offset) => offsetOf(zone, naive - offset) === offset)
    .map((offset) => naive - offset);
  return fits.length > 0 ? Math.min(...fits) : naive - before;
}

/** "Fri, Nov 13, 2026": `day` ("2026-11-13") in words, for the heading of a day in a list. */
export function dayWords(day) {
  const found = DATE.exec(day);
  if (!found) return '';
  const when = Date.UTC(Number(found[1]), Number(found[2]) - 1, Number(found[3]));
  return new Intl.DateTimeFormat('en-US', {
    timeZone: 'UTC', weekday: 'short', month: 'short', day: 'numeric', year: 'numeric',
  }).format(when);
}

/** The Unix time in milliseconds at which `day` ("2026-11-14") begins in `zone`, or null. */
export const dayStart = (day, zone) => wallToUtc(day, '00:00', zone);

/** The day `count` days after `day` (before it when negative): the calendar's, whatever the clocks do. */
export function addDays(day, count) {
  const found = DATE.exec(day);
  if (!found) return null;
  return new Date(Date.UTC(Number(found[1]), Number(found[2]) - 1, Number(found[3]) + count)).toISOString().slice(0, 10);
}

/**
 * The span of a relative range or `today` at `now` (milliseconds) as `{since,
 * until}` in milliseconds, `until` null for "up to now". Null for `custom`,
 * which a person chooses, and for a word that is no range.
 */
export function windowOf(id, now, zone) {
  const range = rangeById(id);
  if (!range || id === 'custom') return null;
  if (id === 'today') return { since: dayStart(dayOf(now, zone), zone), until: null };
  return { since: now - range.seconds * SECOND, until: null };
}

/** "+0100": how far `zone`'s clock is ahead of UTC at `ms`, in the form a saved line carries. */
export function offsetWords(ms, zone) {
  const minutes = Math.round(offsetOf(zone, ms) / 60_000);
  return `${minutes < 0 ? '-' : '+'}${pad(Math.floor(Math.abs(minutes) / 60))}${pad(Math.abs(minutes) % 60)}`;
}

const STAMP = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?Z$/;

/**
 * A record's `time`, RFC 3339 in UTC as logd writes it, as `{ms, seconds,
 * micros}`: the Unix time in milliseconds, in whole seconds, and the part of
 * a second in microseconds. Null for anything else.
 */
export function timeOf(text) {
  const found = typeof text === 'string' ? STAMP.exec(text) : null;
  if (!found) return null;
  const [year, month, day, hour, minute, second] = found.slice(1, 7).map(Number);
  const built = new Date(Date.UTC(year, month - 1, day, hour, minute, second));
  // The calendar carries a 30th of February into March: that is no day.
  if (built.getUTCFullYear() !== year || built.getUTCMonth() !== month - 1 || built.getUTCDate() !== day || hour > 23 || minute > 59 || second > 59) return null;
  const seconds = built.getTime() / SECOND;
  const micros = Number((found[7] ?? '').padEnd(6, '0').slice(0, 6));
  return { ms: seconds * SECOND + Math.floor(micros / 1000), seconds, micros };
}

/** The time of a record in `zone`, to the microsecond with its offset: "2026-11-14 10:30:00.123456 +0100". Null when `text` is not a time. */
export function stampOf(text, zone) {
  const at = timeOf(text);
  if (!at) return null;
  const { date, time } = wallOf(at.ms, zone);
  return `${date} ${time}.${pad(at.micros, 6)} ${offsetWords(at.ms, zone)}`;
}
