// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Sizes and a filling disk in words (design 3.5), as plain values: a size in
// bytes written as the boards write it, how long a length of time is in a
// few rough words, and the straight line through a month of use that says
// when a filesystem will be full. Storage, Snapshots and Move use them.

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];

/**
 * `bytes` as "1.24 TB", "61 GB", "2 TB": the units are thousands, as the
 * disks and metrics(1) count, and a number has two places when it is under
 * 10 and none above, without a ".00".
 */
export function bytesWords(bytes) {
  let number = Number.isFinite(bytes) && bytes > 0 ? bytes : 0;
  let at = 0;
  // 999.5 would be written as 1000, which is 1 of the next unit.
  while (Math.round(number) >= 1000 && at < UNITS.length - 1) {
    number /= 1000;
    at += 1;
  }
  const rounded = Math.round(number * 100) / 100;
  const text = rounded >= 10 || at === 0 ? String(Math.round(number)) : String(rounded);
  return `${text} ${UNITS[at]}`;
}

const MINUTE = 60;
const HOUR = 3600;
const DAY = 86400;
const WEEK = 7 * DAY;
const MONTH = 2629800;
const YEAR = 31557600;

/** `count` and its noun, with an s when it is not one. */
export const counted = (count, noun) => `${count} ${noun}${count === 1 ? '' : 's'}`;

/**
 * How long `seconds` is, roughly, as a phrase that follows "in": "less than a
 * minute", "about 40 minutes", "about 5 hours", "about 3 weeks", "about 7
 * months", "more than 10 years".
 */
export function roughly(seconds) {
  const span = Number.isFinite(seconds) && seconds > 0 ? seconds : 0;
  if (span < MINUTE) return 'less than a minute';
  if (span < 1.5 * HOUR) return `about ${counted(Math.round(span / MINUTE), 'minute')}`;
  if (span < 36 * HOUR) return `about ${counted(Math.round(span / HOUR), 'hour')}`;
  if (span < 14 * DAY) return `about ${counted(Math.round(span / DAY), 'day')}`;
  if (span < 8 * WEEK) return `about ${counted(Math.round(span / WEEK), 'week')}`;
  if (span < 24 * MONTH) return `about ${counted(Math.round(span / MONTH), 'month')}`;
  if (span < 10 * YEAR) return `about ${counted(Math.round(span / YEAR), 'year')}`;
  return 'more than 10 years';
}

// ---- when a filesystem will be full ---------------------------------------

/** The days of history that a projection needs at least. */
export const MIN_DAYS = 7;

/** The days of history that a projection looks at, the last ones. */
export const WINDOW_DAYS = 30;

/** The points of `points` in the `WINDOW_DAYS` that end with the last one, and the days they span. */
function recent(points) {
  const found = (Array.isArray(points) ? points : []).filter((point) => Number.isFinite(point?.at) && Number.isFinite(point?.value));
  if (found.length === 0) return { found, days: 0 };
  const end = found[found.length - 1].at;
  const inside = found.filter((point) => point.at >= end - WINDOW_DAYS * DAY);
  return { found: inside, days: (end - inside[0].at) / DAY };
}

/** Whether `points` (`[{at, value}]`, oldest first, `at` in Unix seconds) cover `MIN_DAYS` days. */
export const enoughHistory = (points) => recent(points).days >= MIN_DAYS;

/**
 * When a filesystem of `size` bytes is full, if its use goes on as the last
 * `WINDOW_DAYS` days of `points` (`[{at, value}]`, oldest first, `at` in
 * Unix seconds, `value` in bytes) say: a straight line through them, which
 * the last point is the start of. Gives `{seconds, words, days}`: the time
 * left, "Full in about 7 months", and how many days the line is drawn
 * through. Gives null when use does not rise, or when there are fewer than
 * `MIN_DAYS` days of history (`enoughHistory` tells these apart).
 */
export function fullIn(points, size) {
  const { found, days } = recent(points);
  if (!(size > 0) || days < MIN_DAYS || found.length < 2) return null;
  const origin = found[0].at;
  const count = found.length;
  let sumX = 0;
  let sumY = 0;
  for (const point of found) {
    sumX += point.at - origin;
    sumY += point.value;
  }
  const meanX = sumX / count;
  const meanY = sumY / count;
  let along = 0;
  let across = 0;
  for (const point of found) {
    along += (point.at - origin - meanX) * (point.value - meanY);
    across += (point.at - origin - meanX) ** 2;
  }
  const slope = across === 0 ? 0 : along / across;
  if (!(slope > 0)) return null;
  const left = size - found[found.length - 1].value;
  const seconds = left > 0 ? left / slope : 0;
  return { seconds, words: seconds === 0 ? 'Full now' : `Full in ${roughly(seconds)}`, days: Math.min(WINDOW_DAYS, Math.ceil(days)) };
}
