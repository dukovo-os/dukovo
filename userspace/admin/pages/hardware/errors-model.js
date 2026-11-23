// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Hardware errors card says, from `hardware errors --json` and with no
// DOM: the newest errors of this start, the memory devices that have errors and
// how many, and the one calm line for a machine that has none. An error of the
// firmware's record from before this start (source `bert`) says so in place of a
// time, since its own time is of no use.

import { amountWords, upWords } from './model.js';

/** The most errors the card lists. */
export const MOST = 10;
export const NO_ERRORS = 'No hardware errors since this boot.';
export const NOT_READ = 'The hardware errors could not be read.';
export const BEFORE = 'Before this start';
export const CAPTION = 'The last ten hardware errors';

/** The severities of the answer: the word and the tone of the state classes. */
const SEVERITIES = Object.freeze({
  fatal: { word: 'Fatal', tone: 'bad' },
  recoverable: { word: 'Not corrected', tone: 'bad' },
  corrected: { word: 'Corrected', tone: 'warn' },
  informational: { word: 'Informational', tone: 'off' },
});

const counted = (count, noun) => `${count} ${noun}${count === 1 ? '' : 's'}`;
const whole = (value) => (Number.isFinite(value) && value > 0 ? Math.floor(value) : 0);

/** Whether `answer` is a document of `hardware errors`. */
export const isAnswer = (answer) => answer !== null && typeof answer === 'object' && Array.isArray(answer.errors);

/** The severity of an error as `{word, tone}`; one that is not known is said as it is, with no tone. */
export function severityOf(name) {
  return SEVERITIES[name] ?? { word: typeof name === 'string' && name !== '' ? name : 'Unknown', tone: 'off' };
}

/** Whether an error is one that was not corrected, or the firmware's from before this start. */
const grave = (error) => error.severity === 'fatal' || error.severity === 'recoverable' || error.source === 'bert';

/**
 * When an error came: from the start of the machine, which `up` seconds ago
 * (`GET /api/1/console/about`) puts on the clock when `time` (lib/time.js) and
 * `now` (Unix seconds) are given, and else as how long after the start. An error
 * of the firmware's record from before this start has no time of its own: its
 * cell is blank unless the answer gives a date (`at`, Unix seconds).
 */
export function whenWords(error, { up = null, now = null, time = null } = {}) {
  if (error.source === 'bert') return time && Number.isFinite(error.at) ? time.when(error.at) : '';
  const seconds = Number(error.seconds);
  if (!Number.isFinite(seconds) || seconds < 0) return 'At an unknown time';
  if (time && Number.isFinite(up) && Number.isFinite(now)) return time.when(Math.floor(now - up + seconds));
  const words = upWords(seconds);
  return words === 'less than a minute' ? 'In the first minute after the start' : `${words} after the start`;
}

/** Who found an error: the firmware reports through its own error sources and its record of the last run, the kernel through the processors' banks and PCIe. Any other source is shown as it is. */
export function foundBy(source) {
  if (source === 'bert' || /^ghes\d*$/.test(source ?? '')) return 'The firmware';
  if (source === 'mce' || source === 'aer') return 'The kernel';
  return typeof source === 'string' && source !== '' ? source : 'Unknown';
}

/**
 * The errors of `answer` as rows, newest first, in two groups: the firmware's
 * record from before this start, and the rest. Without `all`, only the newest
 * `MOST` are taken, then grouped. A group with no row is left out. Gives
 * `[{key: 'before' | 'since', title, rows}]`, each row `{key, when, before,
 * part, what, count, severity: {word, tone}, found}`. `context` is what
 * `whenWords` takes.
 */
export function errorGroups(answer, context = {}, all = false) {
  if (!isAnswer(answer)) return [];
  const sorted = [...answer.errors].sort((a, b) => b.seq - a.seq);
  const rows = (all ? sorted : sorted.slice(0, MOST)).map((error) => ({
    key: String(error.seq),
    when: whenWords(error, context),
    before: error.source === 'bert',
    part: error.part || 'Not named',
    what: error.what || '',
    count: whole(error.count) || 1,
    severity: severityOf(error.severity),
    found: foundBy(error.source),
  }));
  const up = upWords(context.up);
  const groups = [
    { key: 'before', title: BEFORE, rows: rows.filter((row) => row.before) },
    { key: 'since', title: up ? `Since this start, ${up} ago` : 'Since this start', rows: rows.filter((row) => !row.before) },
  ];
  return groups.filter((group) => group.rows.length > 0);
}

/** The rows alone, in the order of the groups. */
export const errorRows = (answer, context = {}, all = false) => errorGroups(answer, context, all).flatMap((group) => group.rows);

/** How many errors the answer lists beyond the rows. */
export const hiddenCount = (answer) => (isAnswer(answer) ? Math.max(0, answer.errors.length - MOST) : 0);

/**
 * The memory devices that have errors, as `{key, name, size, words}`: the
 * ones the tables name by their place on the board, else by their handle;
 * the devices the tables do not list come last, as one.
 */
export function deviceRows(answer) {
  const counts = isAnswer(answer) ? answer.counts : null;
  const dimms = Array.isArray(counts?.dimms) ? counts.dimms : [];
  const say = (corrected, uncorrected) => [
    uncorrected > 0 && `${uncorrected} not corrected`,
    corrected > 0 && `${corrected} corrected`,
  ].filter(Boolean).join(', ');
  const rows = dimms
    .map((dimm) => ({ dimm, corrected: whole(dimm.corrected), uncorrected: whole(dimm.uncorrected) }))
    .filter(({ corrected, uncorrected }) => corrected + uncorrected > 0)
    .map(({ dimm, corrected, uncorrected }) => ({
      key: String(dimm.handle),
      name: dimm.locator || `Memory device ${typeof dimm.handle === 'number' ? `0x${dimm.handle.toString(16).padStart(4, '0')}` : ''}`.trim(),
      size: amountWords(dimm.bytes),
      words: say(corrected, uncorrected),
    }));
  const other = { corrected: whole(counts?.other_dimms?.corrected), uncorrected: whole(counts?.other_dimms?.uncorrected) };
  if (other.corrected + other.uncorrected > 0) {
    rows.push({ key: 'other', name: 'Other memory devices', size: null, words: say(other.corrected, other.uncorrected) });
  }
  return rows;
}

/** The severities in the order of how bad they are, worst first. */
const ORDER = ['fatal', 'recoverable', 'corrected', 'informational'];

/**
 * The card's lead and its pill. `calm` is a machine with none to tell of,
 * which has the one line. The lead of the rest says what the worst errors of
 * this start were and how many came before it, as "Corrected, and 1 fatal
 * before this start".
 */
export function summaryOf(answer) {
  if (!isAnswer(answer)) return { calm: false, lead: NOT_READ, pill: null };
  const counts = answer.counts ?? {};
  const kept = answer.errors.length;
  if (kept === 0 && whole(counts.recorded) === 0) return { calm: true, lead: NO_ERRORS, pill: null };
  const since = answer.errors.filter((error) => error.source !== 'bert');
  const before = answer.errors.filter((error) => error.source === 'bert');
  const worst = ORDER.find((name) => since.some((error) => error.severity === name));
  const earlier = ORDER
    .map((name) => [name, before.filter((error) => error.severity === name).reduce((sum, error) => sum + (whole(error.count) || 1), 0)])
    .filter(([, count]) => count > 0)
    .map(([name, count]) => `${count} ${name === 'recoverable' ? 'not corrected' : name}`);
  const head = worst ? severityOf(worst).word : null;
  const tail = earlier.length > 0 ? `${earlier.join(' and ')} before this start` : null;
  let lead = [head, tail].filter(Boolean).join(', and ');
  if (!head && !tail) lead = `${counted(Math.max(whole(counts.recorded), kept), 'hardware error')} since this boot`;
  lead = lead.charAt(0).toUpperCase() + lead.slice(1);
  const pill = answer.errors.some(grave) || whole(counts.uncorrected) > 0
    ? { word: 'Not corrected', tone: 'bad' }
    : { word: 'Corrected', tone: 'warn' };
  return { calm: false, lead, pill };
}
