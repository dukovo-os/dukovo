// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// How an update's versions are said: the pair, "8.0.3 to 8.0.4", and the kind
// of update they make, from the versions' own numbers.

/** The numbers of `version` before any tag, `[8, 0, 3]` for "8.0.3", or null when it is not made of numbers. */
function numbersOf(version) {
  const found = /^v?(\d+(?:\.\d+)*)$/.exec(String(version ?? '').trim());
  return found ? found[1].split('.').map(Number) : null;
}

/**
 * "8.0.3 to 8.0.4": the version an app or the system has and the one that is
 * offered. One that is not known, and two that are the same, are said once;
 * null when neither is known.
 */
export function versionPair(from, to) {
  const was = from ? String(from) : null;
  const next = to ? String(to) : null;
  if (was && next && was !== next) return `${was} to ${next}`;
  return next ?? was;
}

/**
 * Whether the update from `from` to `to` is `'fixes only'` or a `'new
 * version'`. Fixes only is when the last of three or more numbers moved up and
 * every number before it stayed: 8.0.3 to 8.0.4. A version with fewer numbers
 * has no place for a fix (2026.11 to 2026.12 is a new release), a tag such as
 * `-alpha.1` makes no promise of what changes, and a version that is not made
 * of numbers cannot be told, so each of those is a new version.
 */
export function kindOfUpdate(from, to) {
  const was = numbersOf(from);
  const next = numbersOf(to);
  if (!was || !next || was.length < 3 || was.length !== next.length) return 'new version';
  const last = was.length - 1;
  const held = was.slice(0, last).every((number, at) => number === next[at]);
  return held && next[last] > was[last] ? 'fixes only' : 'new version';
}
