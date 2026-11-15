// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What one opened backup place says (design 5.7), as plain values: what it
// holds and keeps, how its last check went, whether its key was found, and
// the backups that `backup show` lists there. They are read only when a
// place is opened, so the page reads this file then and not before.

import { counted } from '../../lib/size-words.js';
import { inOrder, placeName, placesWords } from '../../lib/snapshot-model.js';
import { dayWords } from '../../lib/snapshot-words.js';
import {
  listOf, msOf, sentence, stateOf, whereWords,
} from './model.js';

/** What a backup place keeps, as 5.7 says it: "7 daily, 4 weekly, 12 monthly" or "3 chains of up to 7 backups". */
export function keepsWords(place) {
  const keeps = place.keeps ?? {};
  if (place.format === 'restic') {
    const parts = [['daily', keeps.daily], ['weekly', keeps.weekly], ['monthly', keeps.monthly]].filter(([, count]) => count > 0).map(([word, count]) => `${count} ${word}`);
    return parts.length > 0 ? parts.join(', ') : 'Only the newest backup';
  }
  return keeps.chains > 0 && keeps.chain > 0 ? `${counted(keeps.chains, 'chain')} of up to ${counted(keeps.chain, 'backup')}` : '';
}

/** How often a place is checked by a restore. */
const CHECKS = Object.freeze({ week: 'Checked every week.', month: 'Checked every month.', never: 'Not checked unless someone asks.' });

/**
 * What one backup place says of itself, from `backup list`: `{holds, keeps,
 * check, key}`; `check` and `key` are `{words, tone}`.
 */
export function detailOf(place, { time, now }) {
  const state = stateOf(place);
  const how = CHECKS[place.verify] ?? '';
  let check;
  if (state.verifyFailed) check = { words: sentence(`Failed: ${state.verifyFailed}`), tone: 'warn' };
  else if (state.verified !== null) check = { words: `${dayWords(state.verified, time.zone, now)}: restored the newest backups and compared them. Passed.`, tone: 'ok' };
  else check = { words: 'Never checked.', tone: '' };
  const key = msOf(place.key_ms);
  return {
    holds: placesWords(listOf(place, 'places')),
    keeps: keepsWords(place),
    check: { words: [check.words, how].filter(Boolean).join(' '), tone: check.tone },
    key: key === null ? { words: 'No recovery key was found for it.', tone: 'warn' } : { words: `Shown once on ${dayWords(key, time.zone, now)}`, tone: '' },
  };
}

// ---- what backup show says -----------------------------------------------------------------

/**
 * The backups a place holds, from `backup show --json`'s `places`, newest
 * first in each: `[{path, name, backups: [{time, ms, whole}]}]`, in the
 * order the page lists places. A backup whose time does not read is left out.
 */
export function heldBackups(show) {
  const byPath = new Map(listOf(show, 'places').filter((place) => typeof place?.path === 'string').map((place) => [place.path, place]));
  return inOrder([...byPath.keys()]).map((path) => ({
    path,
    name: placeName(path),
    backups: listOf(byPath.get(path), 'backups')
      .filter((backup) => typeof backup?.time === 'string' && Number.isFinite(backup.at_ms))
      .map((backup) => ({ time: backup.time, ms: backup.at_ms, whole: backup.whole === true }))
      .sort((a, b) => b.ms - a.ms),
  })).filter((place) => place.backups.length > 0);
}

/** Where a place's backups stay when it is removed, in words that follow "stay". */
export function staysWords(place) {
  const where = whereWords(place).replace(/ · encrypted$/, '');
  if (place.kind === 'ssh') return `at ${where.replace(/^SSH to /, '')}`;
  if (place.kind === 's3') return `in ${where.replace(/^S3 bucket /, 'the bucket ')}`;
  return where.startsWith('USB disk') ? `on ${where}` : `in ${place.to}`;
}
