// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The release calendar of ADR 0534 and the clock of ADR 0458, as functions
// of dates. Every date is a Date at 00:00 UTC unless a function says
// otherwise, and every calculation is in UTC: a runner's local time zone
// changes no answer.
//
// A release is on the second Tuesday of a month, and its branch is cut 14
// days before. The bots in this folder (release-train.js,
// security-clock.js) and their tests start here.

'use strict';

const HOUR = 60 * 60 * 1000;
const DAY = 24 * HOUR;

/** Days before its release day that a release branch is cut. */
const CUT_DAYS_BEFORE = 14;

const TUESDAY = 2;

/** The UTC midnight that starts the day `date` is in. */
function utcDay(date) {
  return new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate()));
}

/** `date` moved by `days` whole days (negative for earlier). */
function addDays(date, days) {
  return new Date(date.getTime() + days * DAY);
}

/** The second Tuesday of `month` (0 for January) of `year`. */
function secondTuesday(year, month) {
  const first = new Date(Date.UTC(year, month, 1));
  const toFirstTuesday = (TUESDAY - first.getUTCDay() + 7) % 7;
  return new Date(Date.UTC(year, month, 1 + toFirstTuesday + 7));
}

/** Whether the UTC day of `date` is the second Tuesday of its month. */
function isReleaseDay(date) {
  const day = utcDay(date);
  return secondTuesday(day.getUTCFullYear(), day.getUTCMonth()).getTime() === day.getTime();
}

/** The first release day on `date`'s UTC day or after it. */
function releaseDayOnOrAfter(date) {
  const day = utcDay(date);
  const thisMonth = secondTuesday(day.getUTCFullYear(), day.getUTCMonth());
  if (thisMonth.getTime() >= day.getTime()) {
    return thisMonth;
  }
  return secondTuesday(day.getUTCFullYear(), day.getUTCMonth() + 1);
}

/** The first release day after `date`'s UTC day. */
function nextReleaseDay(date) {
  return releaseDayOnOrAfter(addDays(utcDay(date), 1));
}

/** The day the branch of the release on `releaseDay` is cut. */
function cutDayOf(releaseDay) {
  return addDays(utcDay(releaseDay), -CUT_DAYS_BEFORE);
}

/**
 * The release day whose branch is cut on `date`'s UTC day, or null when
 * no branch is cut that day.
 */
function releaseCutOn(date) {
  const release = addDays(utcDay(date), CUT_DAYS_BEFORE);
  return isReleaseDay(release) ? release : null;
}

/** The first day on or after `date`'s UTC day on which a branch is cut. */
function nextCutDay(date) {
  const release = releaseDayOnOrAfter(addDays(utcDay(date), CUT_DAYS_BEFORE));
  return cutDayOf(release);
}

/** The last millisecond of the UTC day that `date` is in. */
function endOfDay(date) {
  return new Date(addDays(utcDay(date), 1).getTime() - 1);
}

/** `date` as YYYY-MM-DD, in UTC. */
function formatDay(date) {
  return date.toISOString().slice(0, 10);
}

/** `date` as YYYY-MM-DD HH:MM UTC. */
function formatTime(date) {
  return `${date.toISOString().slice(0, 16).replace('T', ' ')} UTC`;
}

/**
 * The UTC midnight of a YYYY-MM-DD string, or null when the text is not
 * a date that exists (2027-02-29, 2026-13-01).
 */
function parseDay(text) {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text);
  if (!match) {
    return null;
  }
  const [year, month, day] = match.slice(1).map(Number);
  const date = new Date(Date.UTC(year, month - 1, day));
  return formatDay(date) === text ? date : null;
}

module.exports = {
  CUT_DAYS_BEFORE,
  DAY,
  HOUR,
  addDays,
  cutDayOf,
  endOfDay,
  formatDay,
  formatTime,
  isReleaseDay,
  nextCutDay,
  nextReleaseDay,
  parseDay,
  releaseCutOn,
  releaseDayOnOrAfter,
  secondTuesday,
  utcDay,
};
