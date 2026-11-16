// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the navigation lists, as plain values (design 4.4): the entries a
// session sees in order, a badge's words, the phone's tabs, and when badges
// are read.

import { matchPage } from './route-model.js';

/** Whether `session` may see `entry`. */
function may(entry, session) {
  if (entry.needs === 'admin' && !session.admin) return false;
  if (entry.nav?.flag && !session[entry.nav.flag]) return false;
  return true;
}

/** The entries `session` sees, `{main, bottom}`, each by `nav.order`; `bottom` sits apart under the rule. */
export function navEntries(pages, session) {
  const visible = pages.filter((entry) => entry.nav && may(entry, session));
  const byOrder = (a, b) => a.nav.order - b.nav.order;
  return {
    main: visible.filter((entry) => !entry.nav.bottom).sort(byOrder),
    bottom: visible.filter((entry) => entry.nav.bottom).sort(byOrder),
  };
}

export function currentEntry(pages, path) {
  return matchPage(pages, path)?.entry ?? null;
}

/** A link's accessible name with its badge: "Services, 1 failed"; the title alone when the badge has no count. */
export function badgeName(title, badge) {
  if (!badge || !badge.count) return title;
  return `${title}, ${badge.count} ${badge.label ?? ''}`.trimEnd();
}

/** The phone's tabs by their place (1 to 4; the navigation's entries win a place two claim, then those it does not list), and the rest, which More opens. */
export function phoneTabs(pages, session) {
  const { main, bottom } = navEntries(pages, session);
  const apart = pages.filter((entry) => !entry.nav && may(entry, session));
  const places = new Map();
  for (const entry of [...main, ...bottom, ...apart]) {
    if (entry.phone && !places.has(entry.phone)) places.set(entry.phone, entry);
  }
  const tabs = [...places.entries()].sort(([a], [b]) => a - b).slice(0, 4).map(([, entry]) => entry);
  return { tabs, more: [...main, ...bottom].filter((entry) => !tabs.includes(entry)) };
}

// Badges are read at start, and on the events that can change them at most
// this often, and every five minutes while the tab is seen.
export const BADGE_EVENT_GAP_MS = 30_000;
export const BADGE_POLL_MS = 300_000;

/**
 * Whether the badges are to be read now. `why` is `'start'`, `'event'` or
 * `'poll'`; `last` is when they were last read (null: never), `now` the
 * time, and `visible` whether the tab is seen.
 */
export function badgesDue({ why, last, now, visible }) {
  if (why === 'start' || last === null) return true;
  if (why === 'event') return now - last >= BADGE_EVENT_GAP_MS;
  return visible && now - last >= BADGE_POLL_MS;
}
