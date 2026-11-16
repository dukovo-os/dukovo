// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// How a hold, or an update put off, says when and by whom, in the person's own
// clock and the accounts' names. The Updates page and the Apps pages share it.

/** "on Nov 13 at 3:06 AM", "today at 3:06 AM", from the person's clock `time` (lib/time.js). */
export function atWords(ms, time) {
  const words = time.when(ms / 1000);
  if (/^\d/.test(words)) return `today at ${words}`;
  if (words.startsWith('Yesterday ')) return `yesterday at ${words.slice(10)}`;
  const found = /^(.+?) (\d{1,2}:\d{2} [AP]M)$/.exec(words);
  return found ? `on ${found[1]} at ${found[2]}` : `on ${words}`;
}

/** "Nov 23 at 3:06 AM", "today at 3:06 AM": when a reminder that is put off comes back. */
export const untilWords = (ms, time) => atWords(ms, time).replace(/^on /, '');

export const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

const text = (value) => (typeof value === 'string' && value.trim() !== '' ? value.trim() : null);

/** The account names by user ID, from `GET /api/1/accounts` (a list or the object that holds it). */
export function namesOf(answer) {
  const accounts = Array.isArray(answer) ? answer : answer?.accounts;
  return new Map((Array.isArray(accounts) ? accounts : []).filter((one) => Number.isFinite(one?.uid) && text(one.name)).map((one) => [one.uid, one.name]));
}

/** " by admin": who did it, by name when it is known, else by number; nothing when it is not told. */
export const byWords = (by, names) => (Number.isFinite(by) ? ` by ${names?.get(by) ?? `account ${by}`}` : '');

/**
 * What a hold says of itself, `held on Nov 2 at 3:06 AM by admin: "why"`, each
 * of `at` (ms), `by` (a user ID) and `why` that is known; null when none is.
 * The reason is the person's text and goes in as text.
 */
export function holdStory({ at = null, by = null, why = null }, time, names) {
  const reason = text(why);
  if (!Number.isFinite(at) && !Number.isFinite(by) && !reason) return null;
  return `held${Number.isFinite(at) ? ` ${atWords(at, time)}` : ''}${byWords(by, names)}${reason ? `: "${reason}"` : ''}`;
}
