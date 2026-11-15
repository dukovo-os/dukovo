// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Snapshots and backups page says of snapshots (design 3.5, 5.5 and
// 5.6), as plain values: the snapshots of every place as one list by time,
// grouped by day in the person's zone, and the words of the rule that keeps
// them. `snap list --json` gives a time as RFC 3339 text (design 11.5). The
// times in words are snapshot-words.js's, a folder's rows snapshot-folder.js's.
// Nothing here touches the page.

import { counted } from './size-words.js';
import { dayWords } from './snapshot-words.js';
import { makeTime } from './time.js';

/** "a", "a and b", "a, b and c". */
function listWords(items) {
  if (items.length <= 1) return items.join('');
  return `${items.slice(0, -1).join(', ')} and ${items[items.length - 1]}`;
}

// ---- places --------------------------------------------------------------

// The places that people know, in the order the page lists them, with what
// each is called and how a sentence says it.
const KNOWN = Object.freeze([
  { path: '/users', name: 'People\'s files', short: '/users' },
  { path: '/shared', name: 'Shared folders and websites', short: '/shared' },
  { path: '/system/settings', name: 'Settings', short: 'settings' },
  { path: '/applications', name: 'Apps and their data', short: 'apps' },
]);

/** The place `path` as a person names it: "People's files", or the path itself for a place that has no such name. */
export const placeName = (path) => KNOWN.find((known) => known.path === path)?.name ?? path;

/** The place `path` inside a sentence: "/users", "settings". */
export const placeShort = (path) => KNOWN.find((known) => known.path === path)?.short ?? path;

/** "/users, /shared and settings": the places of `paths`, as a sentence names them. */
export const placesWords = (paths) => listWords(paths.map(placeShort));

/** `paths` in the order the page lists places: the known ones first, as `KNOWN` has them, then the others by path. */
export function inOrder(paths) {
  const rank = (path) => {
    const at = KNOWN.findIndex((known) => known.path === path);
    return at === -1 ? KNOWN.length : at;
  };
  return [...new Set(paths)].sort((a, b) => rank(a) - rank(b) || (a < b ? -1 : a > b ? 1 : 0));
}

// ---- the rule that keeps snapshots ----------------------------------------

/**
 * The rule of `settings`, `{daily, weekly}` as snap.toml gives them, in
 * words that follow "Snapshots": "kept daily for 7 days and weekly for 4
 * weeks". Empty text when the server does not say.
 */
export function keptWords(settings) {
  const { daily, weekly } = settings ?? {};
  const parts = [];
  if (daily > 0) parts.push(`daily for ${counted(daily, 'day')}`);
  if (weekly > 0) parts.push(`weekly for ${counted(weekly, 'week')}`);
  if (parts.length > 0) return `kept ${parts.join(' and ')}`;
  return Number.isFinite(daily) && Number.isFinite(weekly) ? 'only the newest kept' : '';
}

// ---- the list --------------------------------------------------------------

/** The words of `kept`, which snap gives as a list of newest, daily and weekly; none when it is not a list. */
const keptList = (kept) => (Array.isArray(kept) ? kept.filter((word) => typeof word === 'string' && word !== '') : []);

/**
 * The snapshots of every place as one list, newest first, from `places`,
 * each `{place, snapshots: [{name, made, named, kept, prunable,
 * restored}]}` as `snap list PLACE --json` gives them, and `update`
 * (`[{name, made}]`, what an update kept of the settings). A snapshot that
 * several places made in one round has one row: they share a name. A row
 * is `{id, name, ms, places, named, kept, prunable, restored, update}`;
 * `kept` is the list of words, and a snapshot whose time does not read is
 * left out.
 */
export function mergeSnapshots(places, update = []) {
  const byName = new Map();
  for (const { place, snapshots } of places) {
    for (const snapshot of Array.isArray(snapshots) ? snapshots : []) {
      const ms = Date.parse(snapshot?.made);
      if (!Number.isFinite(ms) || typeof snapshot.name !== 'string') continue;
      const row = byName.get(snapshot.name) ?? {
        id: snapshot.name, name: snapshot.name, ms, places: [], named: snapshot.named === true, kept: keptList(snapshot.kept),
        prunable: snapshot.prunable === true, restored: snapshot.restored === true, update: false,
      };
      row.ms = Math.min(row.ms, ms);
      row.places.push(place);
      byName.set(snapshot.name, row);
    }
  }
  const rows = [...byName.values()];
  const made = (Array.isArray(update) ? update : []).map((one) => Date.parse(one?.made)).filter(Number.isFinite);
  if (made.length > 0) {
    const ms = Math.min(...made);
    rows.push({ id: `update-${ms}`, name: 'update', ms, places: [], named: false, kept: [], prunable: false, restored: false, update: true });
  }
  return rows.sort((a, b) => b.ms - a.ms || (a.id < b.id ? -1 : 1));
}

/** Why a snapshot is kept, in one word: its name when it was named, else the longest rule that holds it, or that the next prune removes it. */
export function whyKept(row) {
  if (row.named) return row.name;
  if (row.prunable) return 'removed at the next prune';
  for (const word of ['daily', 'weekly', 'newest']) if (row.kept.includes(word)) return word;
  return '';
}

/**
 * `rows` grouped by the day they were made in `zone` at `now`: `[{id,
 * title, rows}]` for Today, Yesterday and Earlier, newest first in each,
 * each only when it has rows. A time after now (the clock was set back)
 * is today's.
 */
export function byDay(rows, zone, now = Date.now()) {
  const time = makeTime(zone, () => now);
  const today = time.midnight() * 1000;
  const yesterday = time.midnight(today / 1000 - 1) * 1000;
  const groups = [
    { id: 'today', title: 'Today', rows: [] },
    { id: 'yesterday', title: 'Yesterday', rows: [] },
    { id: 'earlier', title: 'Earlier', rows: [] },
  ];
  for (const row of [...rows].sort((a, b) => b.ms - a.ms)) {
    groups[row.ms >= today ? 0 : row.ms >= yesterday ? 1 : 2].rows.push(row);
  }
  return groups.filter((group) => group.rows.length > 0);
}

/** The words that stand for the rows of Earlier until they are shown: "14 daily and 4 weekly", or "18 snapshots" when the server says why none is kept. */
export function earlierWords(rows) {
  const counts = { daily: 0, weekly: 0, named: 0 };
  for (const row of rows) {
    const word = row.named ? 'named' : row.kept.includes('daily') ? 'daily' : row.kept.includes('weekly') ? 'weekly' : null;
    if (word) counts[word] += 1;
  }
  const said = Object.values(counts).reduce((sum, count) => sum + count, 0);
  if (said === 0 || said < rows.length) return counted(rows.length, 'snapshot');
  return listWords(Object.entries(counts).filter(([, count]) => count > 0).map(([word, count]) => `${count} ${word}`));
}

/** "Oldest Sep 18": the day of the oldest of `rows`, or empty text when there are none. */
export function oldestWords(rows, zone, now = Date.now()) {
  if (rows.length === 0) return '';
  return `Oldest ${dayWords(Math.min(...rows.map((row) => row.ms)), zone, now)}`;
}

/**
 * What a restored snapshot says of where it came from: "Restored from nas:
 * its backup of Nov 10", from its name, `restored-` and the backup's time.
 * `from` names the backup place when there is one only, and `null` says so
 * in other words ("Restored from a backup of Nov 10").
 */
export function restoredWords(row, zone, from = null, now = Date.now()) {
  const found = /^restored-(\d{4})-(\d{2})-(\d{2})/.exec(row.name);
  const day = found ? dayWords(Date.UTC(Number(found[1]), Number(found[2]) - 1, Number(found[3]), 12), 'UTC', now) : null;
  if (from && day) return `Restored from ${from}: its backup of ${day}`;
  return day ? `Restored from a backup of ${day}` : 'Restored from a backup';
}

