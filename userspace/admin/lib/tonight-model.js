// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What an app's update in tonight's window says, as plain values: the window,
// from the restart time `update settings --json` gives, and what a request that
// waits for it says. The Updates page and the Apps pages share it.

const DAY = 24 * 60;

/** The minutes after midnight of "03:00", or null for text that is not a time of day. */
function minutesOf(time) {
  const found = /^(\d\d):(\d\d)$/.exec(String(time ?? ''));
  const [hours, minutes] = found ? [Number(found[1]), Number(found[2])] : [24, 60];
  return hours < 24 && minutes < 60 ? hours * 60 + minutes : null;
}

/** The window as `[start, end]` in minutes after midnight: the hour that ends at the restart time, 2:00 to 3:00 without one. It may cross midnight. */
export function windowOf(restart) {
  const end = minutesOf(restart) ?? 3 * 60;
  return [(end + DAY - 60) % DAY, end];
}

/** "2:00 AM" for 120 minutes after midnight. */
function clockOf(minutes) {
  const hour = Math.floor(minutes / 60) % 24;
  return `${hour % 12 === 0 ? 12 : hour % 12}:${String(minutes % 60).padStart(2, '0')} ${hour < 12 ? 'AM' : 'PM'}`;
}

/** "2:00 and 3:00 AM", "11:30 PM and 12:30 AM": the window as the words of "between". */
export function betweenWords(restart) {
  const [from, to] = windowOf(restart).map(clockOf);
  return `${from.slice(-2) === to.slice(-2) ? from.slice(0, -3) : from} and ${to}`;
}

/** "Installs tonight, between 2:00 and 3:00 AM", and why that hour when the server has a restart time. */
export function windowWords(restart) {
  return `Installs tonight, between ${betweenWords(restart)}${minutesOf(restart) === null ? '' : ': the hour before this server\'s restart time'}.`;
}

/** What `bundle list --json`'s `tonight` says, `{words, missed}`; null for none. */
export function tonightOf(tonight) {
  if (!tonight || typeof tonight !== 'object') return null;
  const missed = Number.isFinite(tonight.missed_ms);
  return { words: missed ? 'The window passed while the machine was off: it installs tonight.' : 'Installs tonight', missed };
}
