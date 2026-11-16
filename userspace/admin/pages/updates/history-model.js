// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The History card as plain values: the changes of the system and the
// firmware set by start, then what bundled did to the apps, newest first.
// Each is cut to the first 20 until the person asks for all.

import { versionPair } from '../../lib/version-words.js';

/** How many rows of each list show before "Show all". */
export const SHOWN = 20;

/** How an outcome of a start is said, with the tone of its pill. */
export const OUTCOME = Object.freeze({ good: ['ok', 'Good'], unfinished: ['warn', 'Not finished'], failed: ['bad', 'Failed'] });

const WHAT = Object.freeze({
  installed: 'Installed', updated: 'Updated', 'rolled-back': 'Rolled back', removed: 'Removed', held: 'Held', released: 'Released', waiting: 'Waiting', failed: 'Failed', flawed: 'A flaw was found',
});

const list = (value) => (Array.isArray(value) ? value : []);

/** "System 1.0.0-alpha.1 to 1.0.0-alpha.2" or "Firmware set 2026.11 to 2026.12": one change of a start. */
function changeWords(change) {
  const name = change.item === 'firmware' ? 'Firmware set' : 'System';
  return `${name} ${versionPair(change.from, change.to) ?? 'changed'}`;
}

/**
 * The changes of `update history --json`, one row for each start: `{start,
 * ms, words, outcome, error}`, the newest first. A start that changed the
 * system and the firmware set is one row with both said.
 */
export function startRows(doc) {
  const starts = new Map();
  for (const change of list(doc?.changes)) {
    const row = starts.get(change.start) ?? {
      start: change.start, ms: Date.parse(change.time), parts: [], outcome: change.outcome, error: change.error ?? null,
    };
    row.parts.push(changeWords(change));
    starts.set(change.start, row);
  }
  return [...starts.values()].sort((a, b) => b.start - a.start).map(({ parts, ...row }) => ({ ...row, words: parts.join('; ') }));
}

/**
 * What bundled did to the apps, newest first: `{ms, app, what, pair, who,
 * note}`. `who` is "by itself" when bundled did it, else the account's name
 * from `accounts` (its `uid`), else the number.
 */
export function appRows(doc, accounts) {
  const names = new Map(list(accounts).map((one) => [one.uid, one.name]));
  return list(doc?.apps).map((one) => ({
    ms: Date.parse(one.time),
    app: one.id,
    what: WHAT[one.what] ?? one.what,
    pair: versionPair(one.from, one.to),
    who: one.by_user === null || one.by_user === undefined ? 'by itself' : (names.get(one.by_user) ?? `account ${one.by_user}`),
    note: typeof one.note === 'string' && one.note !== '' ? one.note : null,
  })).sort((a, b) => b.ms - a.ms);
}
