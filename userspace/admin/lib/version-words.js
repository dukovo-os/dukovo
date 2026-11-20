// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// How an update's versions are said: the pair, "8.0.3 to 8.0.4", and the kind
// of update they make, from the versions' own numbers. A bundle's version may
// end in a build number, "15.2.0_2": the same version packed again.

/** The version before a trailing build number, and the build: `['15.2.0', 2]` for "15.2.0_2", `['15.2.0', 1]` for "15.2.0". */
function splitBuild(version) {
  const text = String(version ?? '').trim();
  const found = /^(.*)_([2-9]|[1-9]\d{1,3})$/.exec(text);
  return found ? [found[1], Number(found[2])] : [text, 1];
}

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
 * Whether the update from `from` to `to` is a `'rebuild'`, `'fixes only'` or a
 * `'new version'`. A build number is taken off each version first. A rebuild
 * is the same version with a higher build: 15.2.0 to 15.2.0_2. Of what is
 * left, fixes only is when the last of three or more numbers moved up and
 * every number before it stayed: 8.0.3 to 8.0.4. A version with fewer numbers
 * has no place for a fix (2026.11 to 2026.12 is a new release), a tag such as
 * `-alpha.1` makes no promise of what changes, and a version that is not made
 * of numbers cannot be told, so each of those is a new version.
 */
export function kindOfUpdate(from, to) {
  const [wasVersion, wasBuild] = splitBuild(from);
  const [nextVersion, nextBuild] = splitBuild(to);
  if (wasVersion !== '' && wasVersion === nextVersion && nextBuild > wasBuild) return 'rebuild';
  const was = numbersOf(wasVersion);
  const next = numbersOf(nextVersion);
  if (!was || !next || was.length < 3 || was.length !== next.length) return 'new version';
  const last = was.length - 1;
  const held = was.slice(0, last).every((number, at) => number === next[at]);
  return held && next[last] > was[last] ? 'fixes only' : 'new version';
}
