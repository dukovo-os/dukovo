// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The last list of databases an administrator read, for the palette. Reading it
// signs in to PostgreSQL as its superuser and leaves a line in the journal, so
// the palette never reads it; it finds what this page read, for as long as this
// page of the console is open. Kept in memory only, for the person who read it.
// Nothing runs at import.

let last = null;

/** Keeps `read` (the page's `readOf`) as what `who`, a person's name, last read. */
export function remember(who, read) {
  last = { who, read };
}

/** What `who` last read, or null when that was another person or nothing. */
export const remembered = (who) => (last !== null && last.who === who ? last.read : null);
