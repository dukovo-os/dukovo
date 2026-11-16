// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette finds of updates: the system and the firmware set the last
// check offers something for, with their state to filter by (`state:held`),
// and a row of their own to stage, hold or release them ("Stage the system
// update"). Enter on one opens the page. The apps are the Apps page's.

// The states an offer is in, the ones to look at first.
export const STATES = ['update', 'held', 'waiting', 'withdrawn', 'current'];

const NAMES = { system: 'System', firmware: 'Firmware set' };

/** The rows of an answer of `GET /api/1/updates`: one for the system and one for the firmware set, when the check offers them. */
export function itemsOf(answer) {
  const offers = Array.isArray(answer?.updates?.offers) ? answer.updates.offers : [];
  return offers.filter((offer) => offer.item in NAMES).map((offer) => ({
    key: offer.item,
    title: `${NAMES[offer.item]} ${offer.from ?? ''}`.trim(),
    detail: offer.state === 'current' ? 'Up to date' : `${offer.to} is ${offer.state === 'update' ? 'ready' : offer.state}`,
    state: offer.state,
    href: `/updates#${offer.item}`,
    action: 'updates.apply',
    args: { item: offer.item },
  }));
}

export const providers = [{
  group: 'Updates',
  kind: 'update',
  filters: { state: STATES },
  items: async (cx) => itemsOf(await cx.cache('/api/1/updates', 5000, () => cx.api.get('/api/1/updates').catch(() => null))),
}];

export const phrases = {
  'updates.apply': 'Stage the {item} update',
  'updates.hold': 'Hold the {item} update',
  'updates.unhold': 'Release the hold on the {item} update',
};
