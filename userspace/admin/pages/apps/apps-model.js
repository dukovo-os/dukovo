// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What an app's update says: the offers of the last check, the words of an
// update and of the flaws it fixes, and what a task that stopped to ask leaves
// behind. The Apps pages and the Updates page say an app's update in these
// words: `updateWords`, `fixesWords`, `flawWords`, `offersOf`, `checkOf`,
// `askedMore` and `sheetOf`; `appHref` is the address of an app's page.

import { counted } from '../../lib/size-words.js';
import { kindOfUpdate, versionPair } from '../../lib/version-words.js';

/** The severities a flaw has, the worst last. */
export const SEVERITIES = Object.freeze(['low', 'medium', 'high', 'critical']);

export const isText = (value) => typeof value === 'string' && value !== '';
export const listOf = (value) => (Array.isArray(value) ? value : []);
export const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

/** The worst of the severities in `words`, or null when none is one of the four. */
export const worstOf = (words) => words
  .filter((word) => SEVERITIES.includes(word))
  .sort((one, other) => SEVERITIES.indexOf(other) - SEVERITIES.indexOf(one))[0] ?? null;

/** Whether `worst` is a severity that the page shows in its warning colour. */
export const urgent = (worst) => worst === 'high' || worst === 'critical';

/**
 * The address of the page of the app `id`. A dot is written out, as for a
 * service: the server answers a path whose last part has a dot as a file.
 */
export const appHref = (id) => `/apps/${encodeURIComponent(id).replaceAll('.', '%2E')}`;

/**
 * The offers of the apps in `last`, what `updates.last` answers (`{checked_ms,
 * updates: {offers, unchecked}}`, or null while no check has run), as a map
 * from each app's ID to its offer `{item, from, to, state, catalog}`. An offer
 * that is `current` or `withdrawn` is none.
 */
export function offersOf(last) {
  const offers = listOf(last?.updates?.offers).filter((offer) => isText(offer?.item) && offer.item !== 'system' && offer.item !== 'firmware'
    && offer.state !== 'current' && offer.state !== 'withdrawn');
  return new Map(offers.map((offer) => [offer.item, offer]));
}

/**
 * What the last check says of the apps: `checked`, `none` (no check has run),
 * `unchecked` (the apps could not be read) or `unknown` (the last check could
 * not be read at all: `last` is undefined).
 */
export function checkOf(last) {
  if (last === undefined) return 'unknown';
  if (!last?.updates) return 'none';
  return listOf(last.updates.unchecked).some((part) => part?.item === 'apps') ? 'unchecked' : 'checked';
}

/**
 * The words of the offer `{from, to, state, wave_day}`. Gives `{state, to,
 * kind, pair, words}`: `state` is `update`, `waiting` (its wave has not
 * reached this machine) or `held`; `kind` is `fixes only` or `new version`
 * (lib/version-words.js); `pair` is "8.0.3 to 8.0.4"; `words` is what a cell
 * of a table says: "8.0.4 · fixes only", "Held at 8.0.3".
 */
export function updateWords(offer) {
  const kind = kindOfUpdate(offer.from, offer.to);
  const pair = versionPair(offer.from, offer.to);
  if (offer.state === 'held') return { state: 'held', to: offer.to, kind, pair, words: `Held at ${offer.from}` };
  if (offer.state === 'waiting') {
    const words = Number.isInteger(offer.wave_day) ? `Reaches this machine on day ${offer.wave_day} of its waves` : 'Waits for its wave';
    return { state: 'waiting', to: offer.to, kind, pair, words };
  }
  return { state: 'update', to: offer.to, kind, pair, words: `${offer.to} · ${kind}` };
}

/**
 * What an update fixes, from the flaws it fixes (each `{severity}`): "Fixes 1
 * security advisory, high", "Fixes 2 security advisories, worst high"; null
 * when it fixes none.
 */
export function fixesWords(flaws) {
  if (flaws.length === 0) return null;
  const worst = worstOf(flaws.map((flaw) => flaw.severity));
  const many = flaws.length > 1;
  const severity = worst === null ? '' : `, ${many ? 'worst ' : ''}${worst}`;
  return `Fixes ${flaws.length} security ${many ? 'advisories' : 'advisory'}${severity}`;
}

/**
 * What a list says of the flaws of the version installed, `{state, count,
 * worst}`: "2 known flaws, worst high", or null when it knows none.
 */
export function flawWords(flaws) {
  if (flaws?.state !== 'known' || !(flaws.count > 0)) return null;
  return `${counted(flaws.count, 'known flaw')}${SEVERITIES.includes(flaws.worst) ? `, worst ${flaws.worst}` : ''}`;
}

/** The line that `bundle update` asks its question with, when an update asks for more than the version it replaces. */
const QUESTION = /Install it\?/;

/** Whether the task `task` (read whole, with its log) ended because the update asks for more than the version it replaces, and waits for a yes. */
export const askedMore = (task) => task?.state === 'failed' && listOf(task.log).some((line) => QUESTION.test(line));

/** The sheet that came before the question, the lines that say what the update asks for. */
export function sheetOf(task) {
  const lines = listOf(task?.log);
  const asked = lines.findIndex((line) => QUESTION.test(line));
  return (asked === -1 ? lines : lines.slice(0, asked)).join('\n').trim();
}
