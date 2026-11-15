// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The full journal (design 7.3), as plain values: how a change was made, the
// filters of who, how and day, the rows shown 50 at a time, and the script
// of the changes a person chooses, which another machine can run to make the
// same changes. The page that draws them is journal.js.
//
// A row of `GET /api/1/changes` is `{id, at, who, how, action, title, argv,
// status, task, note, files, seen}`; `at` is in Unix seconds.

import { dayOf, dayWords, utcWords } from '../../lib/when-model.js';
import { CHANGES_READ } from './reads-model.js';

/** The rows shown at a time. */
export const PAGE = 50;

/** The route that gives the rows: the newest the route holds, since it has no way to ask for older ones. */
export const READ_PATH = `/api/1/changes?since=0&limit=${CHANGES_READ}`;

/** The ways to filter by, with the words of the choice. */
export const HOWS = Object.freeze([
  { id: 'any', label: 'Any way' },
  { id: 'console', label: 'In this console' },
  { id: 'hand', label: 'By hand' },
  { id: 'put-back', label: 'Put back' },
  { id: 'undone', label: 'Undone by Spark' },
]);

/** The ID the journal gives a put back. */
const PUT_BACK = 'changes.put-back';

/** How a change was made: `console` (here or with a token), `hand`, `put-back` or `undone`. */
export function howOf(change) {
  if (change.how === 'by hand') return 'hand';
  if (change.how === 'undone') return 'undone';
  return change.action === PUT_BACK ? 'put-back' : 'console';
}

const DAY = /^\d{4}-\d{2}-\d{2}$/;

/** The filters in the address: `{who, how, day}`, each empty or `any` when it does not filter, and any word that is no choice ignored. */
export function queryOf(params) {
  const how = params.get('how');
  const day = params.get('day');
  return {
    who: params.get('who') ?? '',
    how: HOWS.some((one) => one.id === how) ? how : 'any',
    day: DAY.test(day ?? '') ? day : '',
  };
}

/** The address of the journal with `query`, its words that filter only. */
export function addressOf(query) {
  const params = new URLSearchParams();
  if (query.who !== '') params.set('who', query.who);
  if (query.how !== 'any') params.set('how', query.how);
  if (query.day !== '') params.set('day', query.day);
  const text = params.toString();
  return `/reliability/changes${text === '' ? '' : `?${text}`}`;
}

/** The rows that pass `query`, in their order. `zone` is the one the day is in. */
export function filterRows(changes, query, zone) {
  return changes.filter((change) => (query.who === '' || change.who === query.who)
    && (query.how === 'any' || howOf(change) === query.how)
    && (query.day === '' || dayOf(change.at * 1000, zone) === query.day));
}

/** The people (or "outside the console") that made the rows, as the filter's choices `{value, label}`, by name. */
export function whoChoices(changes) {
  return [...new Set(changes.map((change) => change.who))].sort((a, b) => a.localeCompare(b))
    .map((who) => ({ value: who, label: who === 'outside the console' ? 'Outside the console' : who }));
}

/** The days the rows were made on, newest first, as the filter's choices `{value, label}`. */
export function dayChoices(changes, zone) {
  return [...new Set(changes.map((change) => dayOf(change.at * 1000, zone)))].sort().reverse()
    .map((day) => ({ value: day, label: dayWords(day) }));
}

// ---- the script ----------------------------------------------------------------

/**
 * `word` as one word of a shell line, as admind's Show command writes it:
 * left as it is when it holds only letters, digits and `@%+=:,./_-`, else in
 * single quotes with each single quote written as `'\''`.
 */
export function shellWord(word) {
  return /^[A-Za-z0-9@%+=:,./_-]+$/.test(word) ? word : `'${word.replaceAll("'", "'\\''")}'`;
}

/**
 * Whether the change can be a line of a script: made through the console,
 * not a put back (it names a row of this machine's journal), and it worked.
 * `runs` maps an action's ID to who runs it, as `GET /api/1/actions` says,
 * so that a change of an action this server no longer has is left out.
 */
export const scriptable = (change, runs) => howOf(change) === 'console'
  && change.status === 0
  && Array.isArray(change.argv) && change.argv.length > 0
  && runs.has(change.action);

/** The line of a change: its command as Show command gives it, with `elevate` where the action ran as an administrator. */
function commandOf(change, runs) {
  const line = change.argv.map(shellWord).join(' ');
  return runs.get(change.action) === 'as-administrator' ? `elevate ${line}` : line;
}

/**
 * The script of `chosen` changes: each a comment with its time and title,
 * then its command, the oldest first. Only the changes that can be a line
 * are in it; none gives an empty text.
 */
export function scriptOf(chosen, runs) {
  const lines = [...chosen].filter((change) => scriptable(change, runs)).sort((a, b) => a.at - b.at || a.id - b.id)
    .flatMap((change) => ['', `# ${utcWords(change.at * 1000)}: ${String(change.title ?? change.action).replace(/\s+/g, ' ')}`, commandOf(change, runs)]);
  if (lines.length === 0) return '';
  return ['#!/bin/sh', '# Changes made with the Dukovo console, oldest first.', 'set -e', ...lines, ''].join('\n');
}
