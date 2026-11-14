// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Where an error goes (design 9.1) and what the error view says, as plain
// values. admind's errors are `{id, title, detail, fix?, field?, command?}`
// with an HTTP status; `placeOf` picks the place from the ID and the
// status, and lib/problem.js draws it there. Each ID named here has a page
// in docs/errors, which problem-model.test.mjs checks.

import { duration } from './time.js';

/** The places an error can go. */
export const PLACE = Object.freeze({
  // Under the field that held the value that cannot work.
  field: 'field',
  // A warning in the page's head: this account may look but not change.
  banner: 'banner',
  // The unlock dialog, and then the same call again.
  unlock: 'unlock',
  // The sign-in page, which api.js's handler for an ended session shows.
  signIn: 'sign-in',
  // Too many tries: a countdown, and the button waits.
  wait: 'wait',
  // The console's helper for the session stopped: try again.
  helper: 'helper',
  // No HTTP answer: the shell's offline banner.
  offline: 'offline',
  // The server lacks a command that a later update brings: plain words in place.
  inPlace: 'in-place',
  // The error view.
  view: 'view',
});

// The IDs of admind's errors that have a place of their own.
const NOT_AN_ADMINISTRATOR = 'ADM-122';
const LOCKED = 'ADM-106';
const SIGNED_OUT = 'ADM-107';
const TOO_MANY = 'ADM-105';
const HELPER_STOPPED = ['ADM-111', 'ADM-112'];
const BAD_VALUE = 'ADM-125';
const MISSING_PROGRAM = 'ADM-129';

/** The IDs this module names, for the test that each has a page. */
export const NAMED_IDS = [
  NOT_AN_ADMINISTRATOR, LOCKED, SIGNED_OUT, TOO_MANY, ...HELPER_STOPPED, BAD_VALUE, MISSING_PROGRAM, 'ADM-123',
];

/** The place for `error`, an ApiError or any object with the same words. */
export function placeOf(error) {
  if (error.status === 0 && error.id === null) return PLACE.offline;
  if (error.id === BAD_VALUE && error.field) return PLACE.field;
  if (error.id === NOT_AN_ADMINISTRATOR) return PLACE.banner;
  if (error.id === LOCKED) return PLACE.unlock;
  if (error.id === SIGNED_OUT) return PLACE.signIn;
  if (error.id === TOO_MANY || error.status === 429) return PLACE.wait;
  if (HELPER_STOPPED.includes(error.id) || error.status === 502) return PLACE.helper;
  if (error.id === MISSING_PROGRAM) return PLACE.inPlace;
  return PLACE.view;
}

/** What a person is told while they wait `seconds` for another try. */
export function waitWords(seconds) {
  return `Too many tries. Try again in ${duration(seconds)}.`;
}

/** What is said when the console's helper for the session stopped. */
export const HELPER_WORDS = 'The console\'s helper for your session stopped.';

/**
 * The program an ADM-129 names in its detail, which puts it in backticks;
 * null when the detail names none that can be told.
 */
export function missingProgram(error) {
  return /`([^`]+)`/.exec(error.detail ?? '')?.[1] ?? null;
}

/** The plain words shown in place of an action's result when its program is not on this server. */
export function inPlaceWords(error) {
  const program = missingProgram(error);
  return program
    ? `This needs ${program}, which this server does not have yet.`
    : 'This needs a part of Dukovo that this server does not have yet.';
}

/** The path of the help page for an error ID, which the console shows itself. */
export function helpPath(id) {
  return `/help/errors/${id}`;
}

/**
 * The parts of the error view, in the Errors board's order: its title; what
 * happened and what to do, as admind says them; the program's own message,
 * when there is a command; and the ID with the words of the link to its
 * page. A part that admind gave no words for is null.
 */
export function errorParts(error) {
  const own = error.command?.stderr?.trim();
  return {
    title: error.title,
    happened: error.detail || null,
    todo: error.fix || null,
    ownWords: own ? own : null,
    id: error.id,
    helpPath: error.id ? helpPath(error.id) : null,
    helpWords: error.id ? `What ${error.id} means` : null,
  };
}

/**
 * The details to copy for a report: the ID, title and detail, the command
 * and its status, the page's `path` and the time as text, one fact a line.
 * `at` is the time in words, from lib/time.js's `full`.
 */
export function detailsText(error, { path, at }) {
  const lines = [
    error.id ? `${error.id}: ${error.title}` : error.title,
    error.detail,
    error.fix,
  ].filter(Boolean);
  if (error.command) {
    lines.push(`Command: ${error.command.argv?.join(' ') ?? ''}`.trimEnd());
    if (error.command.status !== undefined && error.command.status !== null) {
      lines.push(`Status: ${error.command.status}`);
    }
    if (error.command.stderr) lines.push('Its message:', error.command.stderr.trimEnd());
  }
  if (error.status) lines.push(`HTTP status: ${error.status}`);
  lines.push(`Page: ${path}`, `Time: ${at}`);
  return lines.join('\n');
}
