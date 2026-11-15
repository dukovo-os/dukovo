// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Reliability (design 7): /reliability is the page of days, /reliability/starts/N
// one start and /reliability/changes the journal. Each is read when it is
// asked for, so that nothing of the others loads with it.

import { h } from '../../dom.js';

const START = /^starts\/([1-9]\d{0,9})$/;

export async function show(view) {
  // A last slash is no part of the address.
  const rest = view.rest.replace(/\/$/, '');
  if (rest === '') return (await import('./days.js')).show(view);
  if (rest === 'changes') return (await import('./journal.js')).show(view);
  const start = START.exec(rest);
  if (start) return (await import('./start.js')).show(view, Number(start[1]));
  view.head({ title: 'No page here', crumbs: [{ title: 'Reliability', path: '/reliability' }] });
  view.main.append(
    h('p', {}, 'Reliability has no page at ', h('span', { class: 'mono' }, location.pathname), '.'),
    h('div', { class: 'btns' }, h('a', { class: 'btn default', href: '/reliability' }, 'Open Reliability')));
  return undefined;
}
