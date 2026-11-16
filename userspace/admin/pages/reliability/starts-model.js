// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Reliability page says of starts (design 7.3), as plain values so
// that a test reaches every branch: the Starts table and the updates among
// them, the failed start and the offer to find its change, one start's steps
// beside another's. The days and months they are counted in are model.js; the
// files that draw these are starts.js, start.js and days.js.
//
// The starts and the failed start are what `changes --list`, `changes
// --failed` and `startup --compare` print as JSON, which admind passes on; a
// field any of them leaves out is null.

import { duration } from '../../lib/time.js';

/** `words` with its first letter in capitals. */
const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

/** `count` and its noun, with an s (or `many`) when it is not one. */
const counted = (count, one, many = `${one}s`) => `${count} ${count === 1 ? one : many}`;

// ---- starts ------------------------------------------------------------------

/** How long a step or a start took, from `ms`: "850 ms", "41.2 s", "2 min". A time that is not known is a dash. */
export function msWords(ms) {
  if (!Number.isFinite(ms)) return '–';
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  return duration(ms / 1000);
}

/**
 * The number of each start that was a try of changes bisect, counting a run
 * of tries from its first: a `Map` of start number to its try. `starts` is
 * newest first, as `reliability.starts` gives them.
 */
export function tryNumbers(starts) {
  const tries = new Map();
  let run = 0;
  for (const start of [...starts].reverse()) {
    run = start.result === 'bisect-try' ? run + 1 : 0;
    if (run > 0) tries.set(start.start, run);
  }
  return tries;
}

const RESULTS = Object.freeze({
  good: { words: 'Good', dot: 'info' },
  failed: { words: 'Failed', dot: 'bad' },
  'fell-back': { words: 'Fell back', dot: 'bad' },
});

/**
 * How a start ended, `{words, dot}`: Good, Failed with the check that
 * failed it, Fell back, or the try it was of changes bisect. `tries` is
 * `tryNumbers` of the list it is in. A start with no result has not finished,
 * and one this page does not know is shown as it is written.
 */
export function resultOf(start, tries) {
  if (start.result === null || start.result === undefined) return { words: 'Not finished', dot: 'info' };
  if (start.result === 'bisect-try') return { words: `Try ${tries.get(start.start) ?? 1} of changes bisect`, dot: 'info' };
  if (start.result === 'failed') return { words: start.check ? `Failed: ${start.check}` : 'Failed', dot: 'bad' };
  if (Object.hasOwn(RESULTS, start.result)) return RESULTS[start.result];
  return { words: typeof start.result === 'string' && start.result !== '' ? capital(start.result) : '', dot: 'info' };
}

/** The address of one start's page. */
export const startHref = (number) => `/reliability/starts/${number}`;

/** The release of a start with its slot, as the board writes it: "1.0.0-alpha.1 (slot A)". The records give the slot as `a` or `b`. */
export function versionWords(start) {
  const slot = typeof start.slot === 'string' && start.slot !== '' ? `slot ${start.slot.toUpperCase()}` : null;
  if (!start.release) return slot ? `(${slot})` : '';
  return slot ? `${start.release} (${slot})` : start.release;
}

/**
 * The rows of the Starts table, newest first, each `{number, href, started,
 * version, ready, result}`. A start that never became ready says "never"
 * when it failed or fell back, and a dash when its time is not known.
 */
export function startRows(starts, time) {
  const tries = tryNumbers(starts);
  return starts.map((start) => {
    const result = resultOf(start, tries);
    return {
      number: start.start,
      href: startHref(start.start),
      started: Number.isFinite(start.at_ms) ? time.when(start.at_ms / 1000) : 'Time not known',
      version: versionWords(start),
      ready: Number.isFinite(start.ready_ms) ? msWords(start.ready_ms) : result.dot === 'bad' ? 'never' : '–',
      result,
    };
  });
}

/**
 * The starts whose release is not the one of the start before them: `[{start,
 * from, to}]`, `starts` being newest first. A fall-back is the update going
 * back and a try of changes bisect is it being tried, so neither is one, and
 * a try is not the start that comes before the next. The oldest start has
 * none before it.
 */
export function updatesOf(starts) {
  const found = [];
  starts.forEach((start, at) => {
    if (!start.release || start.result === 'fell-back' || start.result === 'bisect-try') return;
    const before = starts.slice(at + 1).find((one) => one.result !== 'bisect-try');
    if (!before?.release || before.release === start.release) return;
    found.push({ start: start.start, from: before.release, to: start.release });
  });
  return found;
}

// ---- a failed start ----------------------------------------------------------

/**
 * One change since the last good start, as a line: "Radix 0.8.1", "Firmware
 * set 2026.11.4" or "org.nginx.nginx 1.30.6", with when it came when it
 * says. `what` is as the records name it: Dukovo, Radix, "firmware set",
 * "CPU microcode", "boot options" or an app's ID, which is shown as it is. A
 * thing the start no longer had has no version.
 */
export function changeWords(change, time) {
  const what = String(change.what ?? 'Something');
  const name = /^[a-z][a-z ]*$/.test(what) ? capital(what) : what;
  const words = change.version ? `${name} ${change.version}` : `${name}, removed`;
  return Number.isFinite(change.at_ms) ? `${words}, ${time.when(change.at_ms / 1000)}` : words;
}

/** The restarts a search of `count` changes takes at most, as changes(1) counts them: one for a lone change, one for each halving. */
export const restartsOf = (count) => (count <= 1 ? count : Math.ceil(Math.log2(count)));

/** What the card of a failed start tells a person to do at a terminal, besides the search. */
export const FAILED_COMMANDS = Object.freeze([
  { title: 'To list the changes again', command: 'changes --failed' },
  { title: 'To see what the checks suggest', command: 'troubleshoot' },
]);

/**
 * The card of a failed start from `reliability.failed`'s answer: null when
 * no start kept has failed. Otherwise `{href, sentence, lead, changes,
 * running, offer}`: what failed it, the changes since the last good start,
 * and either the search that runs now or the offer to run one (two changes
 * at least, or there is nothing to halve).
 */
export function failedOf(answer, time) {
  const start = answer?.failed;
  if (!start) return null;
  const changes = Array.isArray(answer.since_good) ? answer.since_good : [];
  const at = Number.isFinite(start.at_ms) ? ` at ${time.when(start.at_ms / 1000)}` : '';
  const check = start.check ? ` (${start.check})` : '';
  const bisect = answer.bisect ?? {};
  const running = bisect.running
    ? {
      words: Number.isFinite(bisect.try) && Number.isFinite(bisect.of) ? `Changes bisect is running: try ${bisect.try} of ${bisect.of}.` : 'Changes bisect is running.',
      stop: 'elevate changes bisect --stop',
    }
    : null;
  const restarts = restartsOf(changes.length);
  return {
    href: startHref(start.start),
    sentence: `The start${at} failed its health check${check}.`,
    lead: changes.length === 0 ? 'No change is listed since the last good start.' : 'These changes came since the last good start:',
    changes: changes.map((change) => changeWords(change, time)),
    running,
    offer: !running && changes.length >= 2
      ? {
        words: `Find which change broke it. The machine restarts up to ${counted(restarts, 'time')}, and nothing is changed for good.`,
        command: 'elevate changes bisect',
        later: 'An administrator can also run it from What happened on Updates.',
      }
      : null,
  };
}

// ---- one start ---------------------------------------------------------------

const STEP_WORDS = Object.freeze({ 'boot loader': 'Boot loader', 'sign-in': 'Sign-in' });

/** A change of `now` from `then`, "+2.8 s" or "-0.4 s", and "No change" when it is under a tenth of a second. */
export function deltaWords(now, then) {
  if (!Number.isFinite(now) || !Number.isFinite(then)) return '';
  const apart = now - then;
  if (Math.abs(apart) < 100) return 'No change';
  return `${apart > 0 ? '+' : '-'}${msWords(Math.abs(apart))}`;
}

/**
 * The steps of one start beside those of another: `[{name, now, then,
 * change, worse}]`. `answer` is `startup --json`'s, `against` is `{start,
 * steps}` of the start to compare with, or null. `worse` is true when the
 * step took a tenth of a second or more longer.
 */
export function stepRows(answer, against) {
  const before = new Map((against?.steps ?? []).map((step) => [step.name, step.ms]));
  return (answer.steps ?? []).map((step) => {
    const then = before.has(step.name) ? before.get(step.name) : null;
    return {
      name: STEP_WORDS[step.name] ?? capital(String(step.name)),
      now: msWords(step.ms),
      then: against ? msWords(then) : null,
      change: against ? deltaWords(step.ms, then) : null,
      worse: Boolean(against) && Number.isFinite(step.ms) && Number.isFinite(then) && step.ms - then >= 100,
    };
  });
}

/** The services that took longest to become ready in a start: `[{name, took}]`, as `startup --json` lists them. */
export const slowestOf = (answer) => (answer.slowest ?? []).map((one) => ({ name: one.name, took: msWords(one.took_ms) }));

/** The chain of services that set when the start settled, as a line: "logd, then netd, then web". Null when there is none. */
export function chainWords(answer) {
  const names = (answer.chain ?? []).map((one) => one.name);
  return names.length === 0 ? null : names.join(', then ');
}

/** What a person is told of a start that has no times. */
export const NO_TIMES = 'No times are kept for this start.';

/** Whether `error` is the answer of `startup` ending other than with 0, which is how it says it has no times for the start. */
export const hasNoTimes = (error) => error?.id === 'ADM-123';
