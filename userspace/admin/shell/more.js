// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The phone's More sheet (design 4.7): every page of the navigation, then
// the items of the account and Help menus. A modal <dialog> that fills the
// screen; a link in it closes it. The shell reads this file when the sheet
// is first opened.

import { focusBack, h, icon } from '../dom.js';
import { navEntries } from './nav-model.js';

/**
 * Opens the sheet. `pages` and `session()` give what `navEntries` needs;
 * `items` is a list of lists of menu items (shell/menu.js's), which the
 * sheet shows one list after another; `opener` gets focus back when the
 * sheet closes.
 */
export function openMore({ opener, pages, session, items }) {
  const { main, bottom } = navEntries(pages, session());
  const dialog = h('dialog', { class: 'dialog sheet', 'aria-labelledby': 'more-title' });
  const close = () => dialog.close();
  const rows = (list) => list.map((item) => {
    if (item.separator) return h('hr');
    if (item.heading) return h('div', { class: 'menu-head' }, item.heading);
    const body = [item.radio && h('span', { class: 'menu-mark', 'aria-hidden': 'true' }, item.checked ? '●' : ''), item.label];
    if (item.href) return h('a', { class: 'nav', href: item.href }, ...body);
    return h('button', {
      class: 'nav', type: 'button', 'aria-pressed': item.radio ? String(Boolean(item.checked)) : null,
      onclick: () => { close(); item.onSelect(); },
    }, ...body);
  });
  dialog.append(
    h('div', { class: 'dialog-head' }, icon('menu'), h('h2', { id: 'more-title' }, 'More'),
      h('button', { class: 'btn small', type: 'button', onclick: close }, 'Close')),
    h('nav', { class: 'sheet-nav', 'aria-label': 'All pages' },
      ...[...main, ...bottom].map((entry) => h('a', { class: 'nav', href: entry.path }, icon(entry.icon), entry.title))),
    h('div', { class: 'sheet-account' }, ...items.flatMap((list, at) => [at > 0 && h('hr'), ...rows(list)])));
  dialog.addEventListener('click', (event) => { if (event.target.closest('a')) close(); });
  dialog.addEventListener('close', () => {
    dialog.remove();
    focusBack(opener);
  });
  document.body.append(dialog);
  dialog.showModal();
}
