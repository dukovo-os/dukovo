// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The pages the shell shows itself (design 3.4): "No page here", the words
// for a reader where a page is for administrators, and C2a's second-step
// page. The router reads this file when one of them is called for.

import { h } from '../dom.js';
import { titleFor } from './route-model.js';

/** What a reader gets where a page is for administrators and has no view of its own. */
export function adminsOnly(shell, entry) {
  return {
    async show(view) {
      view.head({ title: entry.title });
      view.main.append(h('div', { class: 'banner warn' }, h('i', { class: 'ic ic-lock', 'aria-hidden': 'true' }),
        h('div', {}, h('p', {}, h('b', {}, 'This page is for administrators.'),
          ` ${view.session.name}'s account can look but not change. Ask an administrator, or ask to be made one.`),
        shell.hasPage('accounts') && h('div', { class: 'btns' }, h('a', { class: 'btn small', href: '/accounts' }, 'Who are the administrators?')))));
    },
  };
}

/** "No page here", with a way to the Overview and to the palette. */
export function noPage(shell, path) {
  return {
    async show(view) {
      view.head({ title: 'No page here' });
      view.main.append(
        h('p', {}, 'The console has no page at ', h('span', { class: 'mono' }, path), '.'),
        h('div', { class: 'btns' },
          h('a', { class: 'btn default', href: '/' }, 'Go to the Overview'),
          h('button', { class: 'btn', type: 'button', onclick: () => shell.openPalette() }, 'Search or run a command')));
    },
  };
}

/** C2a's second-step page, in the shell. */
export function secondStepPage(shell) {
  return {
    async show(view) {
      document.title = titleFor('Second step', shell.host);
      const { secondStepView } = await import('../account-steps.js');
      view.main.append(...secondStepView({ session: view.session, signInAgain: () => shell.signInAgain() }));
    },
  };
}
