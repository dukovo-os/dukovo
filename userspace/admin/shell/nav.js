// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The side navigation and the phone's tab bar (design 4.4 and 4.7): the
// entries of pages.js that nav-model.js says a session sees, with badges
// from the pages' `badge` parts, read at start and when an event may have
// changed them.

import { fill, h, icon } from '../dom.js';
import {
  badgeName, badgesDue, currentEntry, navEntries, phoneTabs,
} from './nav-model.js';

/**
 * Makes the navigation. `parts` is shell/parts.js's `collect`; `openMore()`
 * opens the phone's sheet. Gives `{side, tabbar, update(path), readBadges(why)}`.
 */
export function createNav({ pages, session, host, parts, openMore }) {
  const side = h('nav', { class: 'side', id: 'side-nav', 'aria-label': 'Pages' });
  const tabbar = h('nav', { class: 'tabbar', 'aria-label': 'Pages' });
  const badges = new Map();
  let path = '/';
  let lastRead = null;

  const count = (badge) => badge && badge.count > 0
    && h('span', { class: `count${badge.kind === 'bad' ? ' bad' : ''}`, 'aria-hidden': 'true' }, String(badge.count));

  function link(entry, { child = null, badge = null } = {}) {
    const target = child ?? entry;
    const at = currentEntry(pages, path);
    const here = (at?.under ?? at?.id) === entry.id;
    const childHere = entry.children.some((one) => one.path === path);
    const current = child ? path === child.path : here && !childHere;
    const words = badgeName(target.title, badge);
    return h('a', {
      class: `nav${child ? ' sub' : ''}${child ? (current ? ' cur' : '') : (here ? ' cur' : '')}`,
      href: target.path,
      'aria-current': current ? 'page' : null,
      'aria-label': words === target.title ? null : words,
    }, !child && icon(entry.icon), target.title, count(badge));
  }

  function badgeOf(entry, child) {
    const own = badges.get(entry.id) ?? null;
    const holder = entry.children?.find((one) => one.badge);
    if (holder) return child === holder ? own : null;
    return child ? null : own;
  }

  function draw() {
    const { main, bottom } = navEntries(pages, session());
    const open = currentEntry(pages, path);
    const entryLinks = (entry) => [
      link(entry, { badge: badgeOf(entry, null) }),
      open?.id === entry.id && entry.children.map((child) => link(entry, { child, badge: badgeOf(entry, child) })),
    ];
    fill(side,
      h('span', { class: 'hostname' }, host),
      ...main.flatMap(entryLinks),
      bottom.length > 0 && h('hr'),
      ...bottom.flatMap(entryLinks));

    const { tabs } = phoneTabs(pages, session());
    fill(tabbar,
      ...tabs.map((entry) => h('a', {
        class: currentEntry(pages, path)?.id === entry.id ? 'cur' : null, href: entry.path,
        'aria-current': currentEntry(pages, path)?.id === entry.id && path === entry.path ? 'page' : null,
      }, icon(entry.icon), entry.title)),
      h('button', { class: 'tab-more', type: 'button', onclick: (event) => openMore(event.currentTarget) }, icon('menu'), 'More'));
  }

  return {
    side,
    tabbar,
    /** Marks the entry that holds `next` and lists its children. */
    update(next) {
      path = next;
      draw();
    },
    /** Reads the badges again when `badgesDue` says so. `why` is `'start'`, `'event'` or `'poll'`. */
    async readBadges(why) {
      const now = Date.now();
      if (!badgesDue({ why, last: lastRead, now, visible: document.visibilityState === 'visible' })) return;
      lastRead = now;
      const found = await parts('badge', (part, cx) => part.count(cx));
      badges.clear();
      for (const { entry, value } of found) if (value) badges.set(entry.id, value);
      draw();
    },
    draw,
  };
}
