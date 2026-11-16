// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The health score (design 10.1), as plain values. `dukovo-check` keeps
// what it found; the weights are the console's. The score is 100 less a
// weight for each finding, a warning at the weight and a FAILED at twice
// it, and never below 0. A check the table does not know weighs 3, so a
// check added later changes the score a little and never breaks it.
//
// The kept result is `{version, checked, release, results: [{area, check,
// result, detail, id, fix}], counts}`, where `result` is passed, warning,
// FAILED or skipped.

/** What each check weighs, by ID: `{weight, words}`; `words` says what the check looks for, for the table in "How the score works". */
export const WEIGHTS = Object.freeze({
  'SEC-101': { weight: 10, words: 'A firewall rule lets every port in from everywhere' },
  'SEC-102': { weight: 15, words: 'The firewall is off or failed' },
  'SEC-110': { weight: 6, words: 'Passwords sign in over SSH' },
  'SEC-111': { weight: 4, words: 'SSH lets every account sign in with a password' },
  'SEC-112': { weight: 6, words: 'Blocking of repeated wrong passwords is off' },
  'SEC-120': { weight: 20, words: 'An account has an empty password' },
  'SEC-121': { weight: 10, words: 'No account is an administrator' },
  'SEC-130': { weight: 6, words: 'A newer release is waiting' },
  'SEC-131': { weight: 4, words: 'No check for updates in 7 days' },
  'SEC-140': { weight: 4, words: 'The clock has not synchronized in a day' },
  'SEC-150': { weight: 4, words: 'A service gave up after its restarts (each; at most 12 in all)' },
  'SEC-160': { weight: 8, words: 'No snapshot schedule covers the people\'s and shared folders' },
  'SEC-170': { weight: 6, words: 'Alerts reach nobody' },
});

/** The weight of a check that repeats an early warning or a hardware check, by the start of its ID. */
export const PREFIX_WEIGHTS = Object.freeze([
  { prefix: 'MET-', weight: 4, words: 'An early warning is active (each)' },
  { prefix: 'HW-', weight: 6, words: 'The last hardware check warned or failed (each)' },
]);

/** What a check weighs when nothing above names it. */
export const OTHER_WEIGHT = 3;

/** The most that SEC-150, a service that gave up, takes off in all. */
export const SERVICE_CAP = 12;

/** The score a machine with no finding has. */
export const FULL = 100;

/** The bands, best first: the least score each starts at, with its words. The words say what the colour says. */
export const BANDS = Object.freeze([
  { id: 'ok', least: 90, words: 'Good' },
  { id: 'warn', least: 70, words: 'Could be better' },
  { id: 'bad', least: 0, words: 'Needs you' },
]);

/** What each band covers, in words: `[{id, words, range}]`, with `range` "90 and over", "70 to 89" or "under 70". */
export function bandRanges() {
  return BANDS.map((band, at) => {
    const above = BANDS[at - 1];
    const range = at === 0 ? `${band.least} and over` : band.least === 0 ? `under ${above.least}` : `${band.least} to ${above.least - 1}`;
    return { id: band.id, words: band.words, range };
  });
}

/** What a check with `id` weighs. */
export function weightOf(id) {
  if (typeof id === 'string') {
    if (Object.hasOwn(WEIGHTS, id)) return WEIGHTS[id].weight;
    const found = PREFIX_WEIGHTS.find((entry) => id.startsWith(entry.prefix));
    if (found) return found.weight;
  }
  return OTHER_WEIGHT;
}

/** The rows of the table in "How the score works": `{id, weight, words}`. A check that failed takes off twice its weight. */
export function weightRows() {
  return [
    ...Object.entries(WEIGHTS).map(([id, entry]) => ({ id, weight: entry.weight, words: entry.words })),
    ...PREFIX_WEIGHTS.map((entry) => ({ id: `${entry.prefix}…`, weight: entry.weight, words: entry.words })),
    { id: 'Any other', weight: OTHER_WEIGHT, words: 'A check this table does not know yet' },
  ];
}

/** The band of `score`. */
export function bandOf(score) {
  return BANDS.find((band) => score >= band.least) ?? BANDS[BANDS.length - 1];
}

/** The word of a check's result in lower case: `passed`, `warning`, `failed` or `skipped`. */
export const outcomeOf = (result) => String(result ?? '').toLowerCase();

// A result counts against the score unless it passed or was skipped: a word
// this console does not know is not taken for a pass.
const counts = (outcome) => outcome !== 'passed' && outcome !== 'skipped' && outcome !== '';

/** Whether `a` is a worse finding than `b`: a FAILED before a warning, then by what each takes off, then by ID, so that the order never changes between two reads. */
const worse = (a, b) => (a.severity === b.severity ? 0 : a.severity === 'bad' ? -1 : 1)
  || b.points - a.points
  || String(a.id).localeCompare(String(b.id));

/**
 * What the checks found, from the kept result: `{score, band, findings,
 * checked}`. A finding is `{id, area, check, detail, fix, severity,
 * points}`, `severity` being `warn` for a warning and `bad` for a FAILED,
 * and `points` what it takes off. `checked` is the time the checks ran, in
 * Unix seconds, or null when the result does not say.
 */
export function assess(result) {
  const results = Array.isArray(result?.results) ? result.results : [];
  const findings = results
    .filter((one) => counts(outcomeOf(one.result)))
    .map((one) => {
      const failed = outcomeOf(one.result) === 'failed';
      const id = typeof one.id === 'string' && one.id !== '' ? one.id : null;
      const weight = weightOf(id);
      return {
        id,
        area: one.area ?? '',
        check: one.check ?? '',
        detail: one.detail ?? '',
        fix: one.fix ?? '',
        severity: failed ? 'bad' : 'warn',
        points: failed ? weight * 2 : weight,
      };
    })
    .sort(worse);

  const taken = findings.reduce((sum, one) => sum + one.points, 0);
  const gaveUp = findings.filter((one) => one.id === 'SEC-150').reduce((sum, one) => sum + one.points, 0);
  const score = Math.max(0, FULL - (taken - Math.max(0, gaveUp - SERVICE_CAP)));

  const seconds = Date.parse(result?.checked ?? '') / 1000;
  return { score, band: bandOf(score), findings, checked: Number.isFinite(seconds) ? seconds : null };
}
