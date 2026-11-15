// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the pages of Reliability say of what they read, as plain values: the
// most that a read gives, the rows shown a page at a time, and what the
// sources an administrator reads are called. The files that draw them are
// days.js, journal.js and starts.js.

/** The most rows of the journal that one read gives, which is the route's own limit. */
export const CHANGES_READ = 200;

/** The first `count` of `rows`, and how many are left. */
export const pageOf = (rows, count) => ({ shown: rows.slice(0, count), left: Math.max(0, rows.length - count) });

/** What a count says when the read that made it gave the most it may: "Only the newest 1,000 errors are counted, so older days may have more." */
export const cappedWords = (count, things) => `Only the newest ${count.toLocaleString('en-US')} ${things} are counted, so earlier days may have more.`;

/** What the cards that only an unlocked administrator reads are called together. */
export const ADMIN_READS = 'errors, crashes and alerts';
