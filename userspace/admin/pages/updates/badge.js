// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The count beside "Updates" in the navigation: the parts that have an update
// ready and not put off, from the last check the machine kept. The navigation
// shows it once the page has an entry with `nav`.

import { putOff, readLater } from './put-off.js';

/** The badge, or null when nothing waits or no check was kept. */
export async function count(cx) {
  const [answer, later] = await Promise.all([cx.cache('/api/1/updates', 5000, () => cx.api.get('/api/1/updates').catch(() => null)), readLater(cx)]);
  const offers = Array.isArray(answer?.updates?.offers) ? answer.updates.offers : [];
  const now = Date.now();
  const ready = offers.filter((offer) => offer.state === 'update' && !putOff(offer, later, now)).length;
  return ready === 0 ? null : { count: ready, label: 'ready', kind: 'info' };
}
