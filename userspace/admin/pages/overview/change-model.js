// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a row of the journal says (design 10.2 and 7.3): who made the change,
// how, the words of its title and its dot. The Overview's What changed card
// and the journal page on Reliability both draw rows from these.

import { askedBy } from '../../lib/how.js';
import { filesWords } from './part-model.js';

/** How far back What changed looks when no good start is known, in seconds. */
export const WEEK = 7 * 24 * 3600;

/** The words after "What changed", and the time the list starts at (`since`, Unix seconds). */
export function changedHeading(about, time, now) {
  const start = about?.last_good_start;
  if (Number.isFinite(start)) return { more: `since the last good start, ${time.when(start)}`, since: start };
  return { more: 'in the last 7 days', since: now - WEEK };
}

/** Who made a change, as the row says: "alice in this console", "ci with the API token ci", "alice, asked for by the AI client desk", or a hand edit's author. */
export function byWords(change) {
  const who = change.who === 'outside the console' ? 'Someone' : change.who;
  const token = /^token (.+)$/.exec(change.how ?? '');
  if (change.how === 'by hand') return `${who} edited it by hand`;
  if (change.how === 'undone') return 'Spark undid it';
  if (token) return `By ${change.who} with the API token ${token[1]}`;
  return `By ${change.who}${askedBy(change.how, change.who) || ' in this console'}`;
}

/**
 * One row of What changed: `{id, time, title, sub, dot, hand, unseen}`.
 * `dot` is the colour of the dot on the rail, `ok` for a hand edit that was
 * kept, and `bad` for a command that did not work.
 */
export function changeRow(change, time) {
  const hand = change.how === 'by hand';
  const files = Array.isArray(change.files) ? change.files.length : 0;
  const failed = Number.isInteger(change.status) && change.status !== 0;
  const unseen = hand && !change.seen;
  const sub = [
    byWords(change),
    hand && change.seen ? `kept by ${change.seen.by}` : null,
    !hand && files > 0 ? `${files} ${files === 1 ? 'file' : 'files'}` : null,
    failed ? 'it did not work' : null,
    change.note || null,
  ].filter(Boolean).join(' · ');
  return {
    id: change.id,
    time: time.when(change.at),
    title: hand
      ? `${filesWords(change)} changed outside this console`
      : change.how === 'undone' ? `${filesWords(change)} put back: it was being tried` : change.title ?? change.action ?? 'A change',
    sub,
    dot: failed ? 'bad' : unseen ? 'warn' : hand ? 'ok' : 'info',
    hand,
    unseen,
  };
}
