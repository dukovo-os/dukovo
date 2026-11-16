// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the score card says (design 8.3) in plain values: how many checks
// pass, when they ran, what each finding takes off, and the checks that
// passed or were skipped. They are made from the kept result of dukovo-check,
// `{checked, results: [{area, check, result, detail, id, fix}], counts}`, and
// lib/health.js's `assess` of it. Nothing here touches the page.

import { outcomeOf } from '../../lib/health.js';
import { counted } from '../../lib/size-words.js';

/** How many checks passed, warned or failed, and were skipped, from the result's `counts` when it has them, else from its results. */
function tally(result) {
  const given = result?.counts;
  const numbers = ['passed', 'warning', 'failed', 'skipped'].map((name) => given?.[name]);
  if (numbers.every((count) => Number.isInteger(count) && count >= 0)) {
    const [passed, warning, failed, skipped] = numbers;
    return { passed, warning, failed, skipped };
  }
  const outcomes = (Array.isArray(result?.results) ? result.results : []).map((one) => outcomeOf(one.result));
  const of = (word) => outcomes.filter((outcome) => outcome === word).length;
  return {
    passed: of('passed'), warning: of('warning'), failed: of('failed'), skipped: of('skipped'),
  };
}

/** The line under the score: "26 of 29 checks pass, 2 skipped"; a skipped check is not counted as one that ran. */
export function scoreWords(result) {
  const { passed, warning, failed, skipped } = tally(result);
  const ran = passed + warning + failed;
  const rest = skipped > 0 ? `, ${skipped} skipped` : '';
  if (ran === 0) return `No check ran${rest}`;
  return `${passed === ran ? `All ${ran} checks pass` : `${passed} of ${ran} checks pass`}${rest}`;
}

/** The accessible name of the score's ring. */
export const ringWords = (score, band) => `Health score ${score} of 100: ${band.words}`;

/** What a finding takes off, in words ("takes 6 points") and as the board writes it ("+6"). */
export const pointsWords = (points) => `takes ${counted(points, 'point')}`;
export const plusWords = (points) => `+${points}`;

/** When the checks ran, as a sentence's end: "today at 5:17 AM", "yesterday at 7:40 PM", "on Nov 10 at 2:04 AM"; `time` is lib/time.js's. */
export function checkedWords(seconds, time) {
  const words = time.when(seconds);
  if (/^\d/.test(words)) return `today at ${words}`;
  if (words.startsWith('Yesterday ')) return `yesterday at ${words.slice('Yesterday '.length)}`;
  return `on ${words.replace(/ (\d+:\d\d [AP]M)$/, ' at $1')}`;
}

/** The checks that passed or were skipped, in the order dukovo-check ran them: `{key, area, check, skipped, detail}`. */
export function passedRows(result) {
  return (Array.isArray(result?.results) ? result.results : [])
    .map((one, at) => ({ one, at, outcome: outcomeOf(one.result) }))
    .filter(({ outcome }) => outcome === 'passed' || outcome === 'skipped')
    .map(({ one, at, outcome }) => ({
      key: `${at}`, area: one.area ?? '', check: one.check ?? '', skipped: outcome === 'skipped', detail: one.detail ?? '',
    }));
}

/** The words of the button that opens them: "Show the 26 that pass", "Show the 26 that pass and 2 skipped". */
export function passedButton(rows) {
  const passed = rows.filter((row) => !row.skipped).length;
  const skipped = rows.length - passed;
  return skipped > 0 ? `Show the ${passed} that pass and ${skipped} skipped` : `Show the ${passed} that pass`;
}

/** Whether the kept result says that no check ran, which is the state of a machine that was never checked. */
export const nothingRan = (result) => !Array.isArray(result?.results) || result.results.length === 0;
