// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What certd and `cert` did to the certificates, from `cert history --json`
// (the oldest first): the lines the Renewals card shows, newest first, with a
// failure and the success that followed it told as one line, and the
// failure of a renewal that is the newest thing a certificate has.

import { daysWords } from './model.js';

/** How long after a failure a success of the same certificate is told with it, in seconds. */
export const JOIN_SECONDS = 7 * 86_400;

/** How many days of history the card shows before Show all. */
export const SHOWN_DAYS = 90;

const DAY_MS = 86_400_000;

/** The ID at the end of a detail, "(CRT-103)", and the words before it. */
export function splitId(detail) {
  const found = /^(.*?)\s*\(([A-Z]{2,4}-\d{3})\)\s*$/s.exec(String(detail ?? ''));
  return found ? { words: found[1], id: found[2] } : { words: String(detail ?? ''), id: null };
}

/** The entries of `history` that read, oldest first. */
const entriesOf = (history) => (Array.isArray(history) ? history : [])
  .filter((entry) => typeof entry?.name === 'string' && Number.isFinite(entry.at_ms) && typeof entry.event === 'string')
  .sort((one, other) => one.at_ms - other.at_ms);

/** "4 seconds", "1 minute 5 seconds"; null when no time is given. */
export function tookWords(ms) {
  if (!Number.isFinite(ms) || ms < 0) return null;
  const seconds = Math.max(1, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds} ${seconds === 1 ? 'second' : 'seconds'}`;
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return `${minutes} ${minutes === 1 ? 'minute' : 'minutes'}${rest > 0 ? ` ${rest} ${rest === 1 ? 'second' : 'seconds'}` : ''}`;
}

/** What `event` did, in the words a line starts with after the name. */
const EVENT_WORDS = Object.freeze({
  made: 'made by this server',
  renewed: 'renewed',
  got: 'got from the authority',
  added: 'added by hand',
  replaced: 'replaced by hand',
  revoked: 'revoked',
  removed: 'removed',
});

/** How long after `from` (milliseconds) `to` was, in words: "the same day", "the next day", "3 days later". */
function laterWords(from, to, zone) {
  const day = (ms) => {
    const parts = new Intl.DateTimeFormat('en-CA', { timeZone: zone, year: 'numeric', month: '2-digit', day: '2-digit' }).format(ms);
    return Math.floor(Date.parse(`${parts}T00:00:00Z`) / DAY_MS);
  };
  const apart = day(to) - day(from);
  if (apart <= 0) return 'the same day';
  return apart === 1 ? 'the next day' : `${daysWords(apart)} later`;
}

/**
 * The lines of the card, newest first: `{key, at, name, words, detail, id,
 * tone, then}`. `then` is the sentence that tells what followed a failure,
 * when a renewal of the same certificate followed within seven days (that
 * renewal has no line of its own). `exists` is the set of names that are
 * certificates now, which says whether a failure was of a renewal or of the
 * first order; `zone` the person's, to tell the days apart.
 */
export function linesOf(history, { exists = new Set(), zone = 'UTC' } = {}) {
  const entries = entriesOf(history);
  const joined = new Map();
  const taken = new Set();
  entries.forEach((entry, at) => {
    if (entry.event !== 'failed') return;
    const next = entries.findIndex((later, index) => index > at && !taken.has(index) && later.name === entry.name
      && (later.event === 'renewed' || later.event === 'got') && later.at_ms - entry.at_ms <= JOIN_SECONDS * 1000);
    const failedAgain = entries.findIndex((later, index) => index > at && later.name === entry.name && later.event === 'failed');
    if (next < 0 || (failedAgain >= 0 && failedAgain < next)) return;
    taken.add(next);
    joined.set(at, entries[next]);
  });
  return entries.map((entry, at) => ({ entry, at })).filter(({ at }) => !taken.has(at)).reverse().map(({ entry, at }) => {
    const { words, id } = splitId(entry.detail);
    const failed = entry.event === 'failed';
    const success = joined.get(at) ?? null;
    let then = null;
    if (success) {
      const by = success.by === 'cert' ? 'Renewed by hand' : 'Tried again on its own and succeeded';
      then = `${by} ${laterWords(entry.at_ms, success.at_ms, zone)}.`;
    }
    const took = tookWords(entry.took_ms);
    return {
      key: `${entry.at_ms}-${entry.name}-${entry.event}`,
      at: Math.floor(entry.at_ms / 1000),
      name: entry.name,
      words: failed ? (exists.has(entry.name) ? 'renewal failed' : 'getting it failed') : (EVENT_WORDS[entry.event] ?? entry.event),
      detail: [words, took].filter(Boolean).join(', '),
      id,
      tone: failed ? 'bad' : null,
      then,
    };
  });
}

/** The lines of `linesOf` from the last `SHOWN_DAYS` days, or all of them; `now` in milliseconds. */
export function shownLines(lines, all, now) {
  return all ? lines : lines.filter((line) => now - line.at * 1000 <= SHOWN_DAYS * DAY_MS);
}

/**
 * The newest failure of each certificate whose newest event is a failure:
 * a map from the name to `{at, words, id}`. A renewal that came after it, or a
 * removal, ends it.
 */
export function failuresOf(history) {
  const last = new Map();
  for (const entry of entriesOf(history)) last.set(entry.name, entry);
  const failures = new Map();
  for (const [name, entry] of last) {
    if (entry.event !== 'failed') continue;
    const { words, id } = splitId(entry.detail);
    failures.set(name, { at: Math.floor(entry.at_ms / 1000), words, id });
  }
  return failures;
}

/** "renewal failed today 6:00 AM, certd tries again at its next look"; `when` is the time's, which gives the clock for today. */
export function failureWords(failure, when) {
  const at = when(failure.at);
  const words = /^\d/.test(at) ? `today ${at}` : at.replace(/^Yesterday/, 'yesterday');
  return `renewal failed ${words}, certd tries again at its next look`;
}
