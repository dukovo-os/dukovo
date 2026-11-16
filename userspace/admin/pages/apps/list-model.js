// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the list of apps says, as plain values so that a test can hold each
// word: the address of the page, the rows from `bundle list --json` and the
// last check for updates, their order, and the words of the line over the
// table. The palette reads the rows from here too.

import { counted } from '../../lib/size-words.js';
import { tonightOf } from '../../lib/tonight-model.js';
import {
  appHref, checkOf, flawWords, isText, listOf, offersOf, SEVERITIES, updateWords, urgent,
} from './apps-model.js';
import { initialOf, tintOf } from './mark.js';

/** "Checked 4 minutes ago.", and why the apps could not be checked when they could not; `time` is the console's (lib/time.js). An undefined `last` is a read that failed. */
export function checkedWords(last, time) {
  if (last === undefined) return 'The last check could not be read.';
  if (!Number.isFinite(last?.checked_ms)) return 'Not checked yet.';
  const when = `Checked ${time.relative(Math.floor(last.checked_ms / 1000))}.`;
  const why = listOf(last.updates?.unchecked).find((part) => part?.item === 'apps')?.why;
  return isText(why) ? `${when} Could not check the apps: ${why}.` : when;
}

/** What a row says when no update is known, by what the last check says of the apps. */
const NO_UPDATE = Object.freeze({
  checked: 'Up to date', none: 'Not checked yet', unchecked: 'Could not check', unknown: 'Not known',
});

/** The update of one row: the offer of the check, else the version bundled last read, else none. */
function updateOf(bundle, offer, check) {
  const to = offer?.to ?? bundle.update ?? null;
  if (bundle.held === true || offer?.state === 'held') return updateWords({ from: bundle.version, to, state: 'held' });
  if (isText(to) && to !== bundle.version) return updateWords({ ...offer, from: bundle.version, to, state: offer?.state ?? 'update' });
  return { state: 'current', to: null, kind: null, pair: null, words: NO_UPDATE[check] };
}

/** The row of one bundle of `bundle list --json`, with its offer from the check. */
function rowOf(bundle, offer, check) {
  const name = isText(bundle.name) ? bundle.name : bundle.id;
  const flaws = flawWords(bundle.flaws);
  const worst = SEVERITIES.includes(bundle.flaws?.worst) ? bundle.flaws.worst : null;
  return {
    id: bundle.id,
    name,
    version: String(bundle.version ?? ''),
    everyone: bundle.for_everyone === true,
    held: bundle.held === true,
    hold: bundle.held === true ? { at: bundle.held_since_ms ?? null, by: bundle.held_by ?? null, why: bundle.held_reason ?? null } : null,
    tonight: tonightOf(bundle.tonight),
    initial: initialOf(name),
    tint: tintOf(bundle.id),
    href: appHref(bundle.id),
    update: updateOf(bundle, offer, check),
    flaws: flaws === null ? null : { words: flaws, worst, urgent: urgent(worst) },
  };
}

/** The order of the list: the apps with known flaws first, the worst first, then those that have an update, then by name. */
const compareRows = (one, other) => {
  const rank = (row) => (row.flaws ? 0 : row.update.state === 'update' || row.update.state === 'waiting' ? 1 : 2);
  const worst = (row) => SEVERITIES.indexOf(row.flaws?.worst);
  return rank(one) - rank(other)
    || worst(other) - worst(one)
    || one.name.toLowerCase().localeCompare(other.name.toLowerCase())
    || one.id.localeCompare(other.id);
};

/**
 * The rows of the Apps list from `list`, the answer of `apps.list`
 * (`bundle list --json`), and `last`, the last check or null: `{everyone,
 * yours}`. The apps installed for everyone are those the page acts on, in
 * the order of the list; a person's own are apart, since no action runs
 * them.
 */
export function rowsOf(list, last) {
  const offers = offersOf(last);
  const check = checkOf(last);
  const rows = listOf(list?.bundles)
    .filter((bundle) => isText(bundle?.id))
    .map((bundle) => rowOf(bundle, offers.get(bundle.id), check))
    .sort(compareRows);
  return { everyone: rows.filter((row) => row.everyone), yours: rows.filter((row) => !row.everyone) };
}

/** The words that say how many apps the list holds, for the table's caption and the row of the Yours table. */
export const captionWords = (count, whose) => `${counted(count, 'app')} installed ${whose}`;

/** The commands of an app that is a person's own, which no action runs. */
export const ownCommands = (id) => ({ update: `bundle update ${id}`, remove: `bundle remove ${id}` });

/** The word the palette filters a row by: the flawed first, then held, then those with an update; null for the rest. */
export function stateWord(row) {
  if (row.flaws) return 'flawed';
  if (row.update.state === 'held') return 'held';
  return row.update.state === 'update' || row.update.state === 'waiting' ? 'update' : null;
}

/** What the palette says of an app after its name: "8.0.3 · update waits", "8.0.3 · held", "8.0.3 · 2 known flaws, worst high". */
export function paletteWords(row) {
  const update = { update: 'update waits', waiting: 'update waits for its wave', held: 'held' }[row.update.state] ?? null;
  return [row.version, update, row.flaws?.words].filter(Boolean).join(' · ');
}

/** What the page says when no app has the ID asked for. */
export const noSuchApp = (id) => `No app called ${id} is installed here.`;

/** What the head says while there is no page to add an app: where apps come from, and the commands. */
export const ADD_LEAD = 'Apps come from a catalog: bundle search WORD, then elevate bundle install ID --for-everyone.';

/** The rows that an update of all the apps takes: those with an update that this machine has. */
export const updatable = (rows) => rows.filter((row) => row.update.state === 'update');
