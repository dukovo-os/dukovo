// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What happened after a start that failed, and the search for the change that
// broke it, as plain values: the heading and lines of the card from `changes
// --failed --json`, the words of each try and of the result from `changes
// bisect --status --json`. No DOM.

import { counted } from '../../lib/size-words.js';
import { atWords, ended, slotName } from './model.js';

const list = (value) => (Array.isArray(value) ? value : []);
const text = (value) => (typeof value === 'string' && value.trim() !== '' ? value.trim() : null);

/** The address of the page that says what an error ID means. */
export const errorHref = (id) => `/help/errors/${id}`;

/** Where Report it goes for a start that failed. */
export const REPORT_HREF = '/report?about=failure';

/** Whether the failed start was an update that went back to the version before it. */
export const fellBack = (failed) => failed?.result === 'fell-back';

/** "The update to 1.0.0-alpha.2 was taken back", or "The start on Nov 13 at 3:05 AM failed". */
export function happenedHeading(failed, time) {
  if (fellBack(failed)) return `The update to ${failed.release ?? 'the new version'} was taken back`;
  return `The start ${Number.isFinite(failed?.at_ms) ? atWords(failed.at_ms, time) : `number ${failed?.start ?? ''}`.trim()} failed`;
}

/** "org.nginx.nginx 1.30.6, installed Nov 12": a change since the last good start. */
export function sinceWords(change, time) {
  const version = text(change.version);
  const when = Number.isFinite(change.at_ms) ? `, installed ${time.when(change.at_ms / 1000)}` : '';
  return `${change.what}${version ? ` ${version}` : ''}${when}`;
}

/**
 * What the card holds: `{when, check, failure, since, did}`. `notice` is the
 * lines of the failure notice the machine kept (`console.notices`), `slots`
 * is what an administrator reads of the slots (null for anyone else).
 */
export function happenedOf(failed, { notice, slots, time }) {
  const slot = failed.slot ? `, slot ${slotName(failed.slot)}` : '';
  const update = slots?.last_update;
  return {
    when: `Start ${failed.start}${Number.isFinite(failed.at_ms) ? ` ${atWords(failed.at_ms, time)}` : ''}${failed.release ? `, release ${failed.release}` : ''}${slot}`,
    check: text(failed.check),
    failure: list(notice).filter((line) => typeof line === 'string'),
    since: list(failed.since_good).map((change) => sinceWords(change, time)),
    did: fellBack(failed) && update
      ? `The machine went back to ${update.from} with ${update.settings_restored ? 'the settings from before the update. Nothing was lost' : 'today\'s settings'}.` : null,
  };
}

// ---- the search --------------------------------------------------------------

/** The most restarts a search of `count` changes takes: the base 2 logarithm of the count, rounded up. */
export const mostTries = (count) => (count < 2 ? 0 : Math.ceil(Math.log2(count)));

/** Whether the search can be started here: it is not running and has no result, and two changes or more can be tried apart. */
export const canStart = (status, failed) => Boolean(failed) && !fellBack(failed) && !status?.running && !status?.result && list(status?.levers).length >= 2;

/** "the system 1.0.0-alpha.2" or "app org.nginx.nginx 1.30.6": a change the search can leave out. */
export const leverWords = (lever) => `${lever.kind === 'app' ? 'app ' : 'the '}${lever.what}${text(lever.version) ? ` ${lever.version}` : ''}`;

/** The words before the search starts. */
export function beforeWords(levers) {
  const most = mostTries(levers.length);
  return `Each try is one restart that leaves some of the changes out. ${counted(levers.length, 'change')} take at most ${counted(most, 'restart')}. The machine restarts 60 seconds after each try's result, and you can stop the search between tries.`;
}

const VERDICT = {
  good: (n, of) => `Try ${n}${of} started well: the cause is in the half it left out.`,
  failed: (n, of) => `Try ${n}${of} failed as the first start did: the cause is in the half it kept.`,
  unfinished: (n, of) => `Try ${n}${of} did not finish starting. It counts as failed unless it starts well the second time.`,
  otherwise: (n, of) => `Try ${n}${of} failed in another way, so its halves cannot be judged and the search ends.`,
  'not-started': (n, of) => `Try ${n}${of} did not start.`,
};

/** One try as a row: `{try, words, verdict}`; `words` says what it left out. */
export function tryWords(one, of) {
  const left = list(one.left_out).filter((what) => typeof what === 'string');
  const out = left.length > 0 ? `left out ${left.join(', ')}` : 'left nothing out';
  const verdict = (VERDICT[one.verdict] ?? ((n, total) => `Try ${n}${total} is waiting for its start.`))(one.try, of ? ` of ${of}` : '');
  return { try: one.try, words: `${one.confirm ? 'Confirming try' : `Try ${one.try}`}: ${out}`, verdict };
}

/** When the next try starts, from `next_ms` read at `now`: "The next try starts in 45 seconds", "within a minute" while it is not set ahead. */
export function nextWords(nextMs, now) {
  if (!Number.isFinite(nextMs)) return 'The next try starts within a minute.';
  const seconds = Math.max(0, Math.round((nextMs - now) / 1000));
  return seconds === 0 ? 'The next try starts now.' : `The next try starts in ${counted(seconds, 'second')}.`;
}

const END = {
  found: 'The change was found.',
  'not-found': 'No single change explains the failure.',
  otherwise: 'A try failed in another way, so the search could not judge.',
  'not-started': 'A try did not start.',
  broken: 'The search could not go on.',
  stopped: 'The search was stopped.',
};

/**
 * What the search says now, from `changes bisect --status --json` read at
 * `now`: `{kind, lines, tries, result}`. `kind` is `running`, `ended` or
 * `idle`. `result` is `{lines, undo, makeSure, left}` once it ended; `left`
 * is the note that the machine runs the last try.
 */
export function searchOf(status, now) {
  const tries = list(status?.tries).map((one) => tryWords(one, status.of));
  const result = status?.result;
  if (result) {
    const found = result.end === 'found';
    const words = list(result.words).filter((line) => typeof line === 'string');
    const named = found && text(result.what) ? `Found: ${result.what}${text(result.version) ? ` ${result.version}` : ''}${result.confirmed ? ', confirmed' : ', by elimination'}.` : (END[result.end] ?? END.broken);
    return {
      kind: 'ended',
      lines: [],
      tries,
      result: {
        lines: words.length > 0 ? words : [named],
        undo: list(result.undo).filter((line) => typeof line === 'string'),
        makeSure: found && !result.confirmed,
        left: 'This start is a try of the search. The next usual start is the failed one again until the fix is taken.',
      },
    };
  }
  if (status?.running) {
    const last = tries.at(-1);
    return {
      kind: 'running',
      lines: [last ? last.verdict : `Try ${status.try ?? 1} of ${status.of ?? '?'} starts at the next restart.`, nextWords(status.next_ms, now)],
      tries,
      result: null,
    };
  }
  return { kind: 'idle', lines: [], tries, result: null };
}

/** The line that explains why a search cannot start after an update that was taken back. */
export const FELL_BACK_WORDS = ended('The search is not offered after an update was taken back: it cannot start the slot of that update');
