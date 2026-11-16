// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The count beside "Updates" in the navigation: the parts that have an update
// ready, from the last check the machine kept. The navigation shows it once
// the page has an entry with `nav`.

/** The badge, or null when nothing waits or no check was kept. */
export async function count(cx) {
  const answer = await cx.cache('/api/1/updates', 5000, () => cx.api.get('/api/1/updates').catch(() => null));
  const offers = Array.isArray(answer?.updates?.offers) ? answer.updates.offers : [];
  const ready = offers.filter((offer) => offer.state === 'update').length;
  return ready === 0 ? null : { count: ready, label: 'ready', kind: 'info' };
}
