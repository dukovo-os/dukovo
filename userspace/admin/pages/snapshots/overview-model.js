// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Snapshots and backups page gives the Overview (design 5.11), as
// plain values: the things to look at, from `backup list --json` and `snap
// list --json`, and where the step "Copy your data off this server" stands.

import { counted } from '../../lib/size-words.js';
import {
  atWords, backupPlaces, isCovered, lateOf, placeHref, stateOf,
} from './model.js';

/** The folders that a machine must have snapshots of, as the check SEC-160 holds. */
export const MUST_COVER = Object.freeze(['/users', '/shared']);

/** `words` as a sentence. */
const sentence = (words) => (/[.!?]$/.test(words) ? words : `${words}.`);

/** Unix seconds of the milliseconds `ms`, or null. */
const secondsOf = (ms) => (ms === null ? null : Math.floor(ms / 1000));

/**
 * The things the Overview lists: a backup that failed, one that is
 * overdue, a check by a restore that failed, and a folder that no snapshot
 * covers. `backups` and `list` are what `GET /api/1/backups` and `GET
 * /api/1/snapshots` gave, or null for one that could not be read; `time` is
 * lib/time.js's (null leaves out the time of the last good backup) and
 * `now` is in milliseconds. `covers` names the alerts and the check that say
 * the same, so that one fault is listed once.
 */
export function needsOf(backups, list, { time = null, now = Date.now() } = {}) {
  const needs = [];
  for (const place of backupPlaces(backups)) {
    const { name } = place;
    const state = stateOf(place);
    const late = lateOf(place);
    const lastGood = time && state.done !== null ? `The last good one was ${atWords(state.done, { time, now })}.` : '';
    const base = { href: placeHref(name), icon: 'snapshots', at: null, covers: [`backup-failed:${name}`] };
    if (state.failed) {
      needs.push({ ...base, id: `backup-failed-${name}`, severity: 'warn', title: `The last backup to ${name} failed`, detail: [sentence(state.failed), lastGood].filter(Boolean).join(' '), at: secondsOf(state.tried) });
    }
    if (late) {
      needs.push({
        ...base,
        id: `backup-overdue-${name}`,
        severity: late.tone,
        title: `Backups to ${name} are ${counted(late.days, 'day')} overdue`,
        detail: [lastGood || (state.done === null ? 'Nothing has been backed up there yet.' : ''), 'Open it to back up now.'].filter(Boolean).join(' '),
        covers: [`backup-overdue:${name}`],
      });
    }
    if (state.verifyFailed) {
      needs.push({ ...base, id: `backup-check-${name}`, severity: 'warn', title: `Checking ${name}'s backups by restoring them failed`, detail: sentence(state.verifyFailed) });
    }
  }
  if (list != null) {
    for (const path of MUST_COVER) {
      if (isCovered(list, path)) continue;
      needs.push({
        id: `snapshots-none-${path}`,
        severity: 'warn',
        title: `No snapshots of ${path}`,
        detail: `Nothing makes a snapshot of ${path} each day, so a file that is deleted or changed there cannot be brought back from this server.`,
        href: '/snapshots',
        icon: 'snapshots',
        at: null,
        covers: ['SEC-160'],
      });
    }
  }
  return needs;
}

/** Whether one backup place has a good backup: one made, and the last try did not fail. */
export const hasGoodBackup = (backups) => backupPlaces(backups).some((place) => stateOf(place).done !== null && !stateOf(place).failed);

/** Where the step "Copy your data off this server" stands, in words. */
export function stepSummary(backups) {
  const places = backupPlaces(backups);
  const good = places.find((place) => stateOf(place).done !== null && !stateOf(place).failed);
  if (good) return good.every === 'hand' ? `${good.name} is backed up by hand` : `${good.name} backs up every ${good.every === 'week' ? 'week' : 'day'}`;
  if (places.length > 0) return `${places[0].name} is added, and has no good backup yet`;
  return 'Nothing is copied off this server yet';
}

/** How many people's accounts `GET /api/1/accounts` gave (`user list --json`, a list, or the object that holds it); null when it is neither. */
export function peopleCount(answer) {
  const list = Array.isArray(answer) ? answer : answer?.accounts;
  return Array.isArray(list) ? list.length : null;
}

/** Whether the machine is a new install, which is when moving to it is offered: no backup place yet, and one person's account. */
export const isNewInstall = (backups, accounts) => backups != null && backupPlaces(backups).length === 0 && peopleCount(accounts) === 1;
