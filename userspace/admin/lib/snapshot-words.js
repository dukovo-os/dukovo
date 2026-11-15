// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The times of snapshots in words (design 3.5, 5.5 and 5.6), and the address
// of a snapshot's folder: the day of a time, the label that a restored file
// carries in its name, a snapshot's time in a crumb or a heading. The list of
// snapshots and the page of a folder both use them, and nothing else is
// here, so that a folder opens without the rest of the model.

import { makeTime } from './time.js';

/** `text` with the narrow no-break space that newer ICU puts before AM and PM made a plain one, so that a name made of it is printable ASCII. */
const plain = (text) => text.replace(/[  ]/g, ' ');

/** The day of `ms` in words: "Nov 10" this year, "Nov 10, 2025" in another. */
export function dayWords(ms, zone, now = Date.now()) {
  const time = makeTime(zone, () => now);
  const year = (at) => new Intl.DateTimeFormat('en-US', { timeZone: time.zone, year: 'numeric' }).format(at);
  const options = year(ms) === year(now) ? { month: 'short', day: 'numeric' } : { month: 'short', day: 'numeric', year: 'numeric' };
  return plain(new Intl.DateTimeFormat('en-US', { timeZone: time.zone, ...options }).format(ms));
}

/**
 * The label that `snap restore --beside` puts in the name of a restored
 * file, from the time `ms` the snapshot was made, on the clocks of `zone`:
 * "Nov 13 3:02 AM", with the year in another year. It is plain ASCII of
 * well under the 40 characters the command takes, and has no `/`.
 */
export function beside(ms, zone, now = Date.now()) {
  const time = makeTime(zone, () => now);
  return `${dayWords(ms, zone, now)} ${time.clock(ms / 1000)}`;
}

/**
 * A snapshot's time as a crumb or a heading has it: "today 3:02 AM",
 * "yesterday 6:41 PM", "Nov 10 2:04 AM", with the year in another year.
 */
export function whenWords(ms, time) {
  const words = time.when(ms / 1000);
  if (/^\d/.test(words)) return `today ${words}`;
  return words.startsWith('Yesterday ') ? `yesterday ${words.slice('Yesterday '.length)}` : words;
}

/** The address that browses `folder` of the snapshot `snapshot` of `place`. */
export function browseHref(place, snapshot, folder) {
  return `/snapshots/browse?${new URLSearchParams({ place, snapshot, folder }).toString()}`;
}
