// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Updates page gives the Overview: the things to look at from the
// last check (an update waits, a release was taken back and is held, a part
// could not be checked, no check for a week), the buttons of the failed start's
// notice, and the row that says when the check was. Every read is the
// person's own: `GET /api/1/updates` is `update last --json`, kept by the daily
// check. An update that is put off to later is not one that waits until then.

import { forget, recall } from '../../lib/store.js';
import { putOff, readLater } from './put-off.js';

/** The days a check may be old before the Overview says so. */
const STALE_DAYS = 7;

/** The key of what this browser kept: the failed start whose "What happened" was opened. */
const SEEN = 'updates.happened.seen';

const read = (cx) => cx.cache('/api/1/updates', 5000, () => cx.api.get('/api/1/updates').catch(() => null));
const offersOf = (answer) => (Array.isArray(answer?.updates?.offers) ? answer.updates.offers : []);

/** The things to look at, for the Overview, from the last check `answer`, the time `now` in milliseconds and the reminders `later` that are put off (null when not read). */
export function needsOf(answer, now, later = null) {
  const offers = offersOf(answer);
  const found = [];
  const system = offers.find((offer) => offer.item === 'system');
  const waits = (offer) => offer?.state === 'update' && !putOff(offer, later, now);
  const advisories = (Array.isArray(answer?.updates?.security?.system) ? answer.updates.security.system : []).filter((flaw) => flaw.fix === 'offered');
  if (waits(system)) {
    found.push({
      id: 'update-system',
      severity: advisories.length > 0 ? 'warn' : 'info',
      title: `${system.to} is ready`,
      detail: advisories.length > 0 ? `It fixes ${advisories.map((flaw) => `${flaw.id}, ${flaw.severity}`).join('; ')}.` : 'Stage it, then restart to finish.',
      href: '/updates#system',
      icon: 'updates',
      at: Number.isFinite(answer.checked_ms) ? Math.floor(answer.checked_ms / 1000) : null,
    });
  }
  if (system?.state === 'held' && system.held_because === 'fell-back') {
    found.push({
      id: 'update-held',
      severity: 'warn',
      title: `${system.to} was taken back and is held`,
      detail: 'It is not staged again until you release it.',
      href: '/updates#happened',
      icon: 'updates',
      at: Number.isFinite(Date.parse(system.held_at)) ? Math.floor(Date.parse(system.held_at) / 1000) : null,
    });
  }
  const apps = offers.filter((offer) => offer.item !== 'system' && offer.item !== 'firmware' && waits(offer)).length;
  if (apps > 0) {
    found.push({
      id: 'update-apps', severity: 'info', title: `${apps} ${apps === 1 ? 'app update is' : 'app updates are'} ready`, detail: 'Update them from Updates.', href: '/updates#apps', icon: 'updates', at: null,
    });
  }
  const unchecked = Array.isArray(answer?.updates?.unchecked) ? answer.updates.unchecked[0] : null;
  if (unchecked) {
    found.push({
      id: 'update-unchecked',
      severity: 'warn',
      title: 'Updates could not be checked',
      detail: `${unchecked.item}: ${unchecked.why || 'no reason was given'}`,
      href: '/updates',
      icon: 'updates',
      at: null,
    });
  }
  const days = Number.isFinite(answer?.checked_ms) ? Math.floor((now - answer.checked_ms) / 86_400_000) : 0;
  if (days >= STALE_DAYS) {
    found.push({
      id: 'update-stale', severity: 'warn', title: `Updates were last checked ${days} days ago`, detail: 'The daily check has not run. Check now from Updates.', href: '/updates', icon: 'updates', at: Math.floor(answer.checked_ms / 1000),
    });
  }
  return found;
}

/** The things to look at, for the Overview. A check that was never kept has none. */
export async function needs(cx) {
  const [answer, later] = await Promise.all([read(cx), readLater(cx)]);
  // A new failed start is a new thing to see: what was kept for the one before it is let go.
  if (recall(SEEN) !== null && answer?.updates) {
    const failed = await cx.cache('/api/1/updates/failed', 5000, () => cx.api.get('/api/1/updates/failed').catch(() => null));
    if (failed?.failed && recall(SEEN) !== failed.failed.start) forget(SEEN);
  }
  return needsOf(answer, Date.now(), later);
}

/** The row of the server card: when the update check was, and what waits. */
export async function rows(cx) {
  const answer = await read(cx);
  if (!Number.isFinite(answer?.checked_ms)) return [];
  const ready = offersOf(answer).filter((offer) => offer.state === 'update').length;
  return [{
    label: 'Updates',
    value: `Checked ${cx.time.relative(answer.checked_ms / 1000)}${ready > 0 ? `, ${ready} ready` : ''}`,
    order: 25,
  }];
}

/** The buttons the failed start's notice gets: what happened, and the report. */
export function noticeButtons(name) {
  if (name !== 'failed-start') return [];
  return [
    { label: recall(SEEN) === null ? 'What happened' : 'See it again', href: '/updates#happened' },
    { label: 'Report it', href: '/report?about=failure' },
  ];
}
