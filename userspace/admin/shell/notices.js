// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The toasts of design 9.3, in the notices region: `role="status"`, polite,
// at most three, newest at the bottom. One without a button goes after 8 s;
// hovering or focusing one holds its time. Each has a Close button.

import { h, icon } from '../dom.js';
import { lifetime, makeNotices } from './notices-model.js';

const ICONS = { info: 'info', ok: 'check', warn: 'warn' };

/** Fills `region` with notices. Gives `notice(words, options)`, which gives a function that closes it. */
export function noticesIn(region) {
  const model = makeNotices();
  const timers = new Map();
  const elements = new Map();

  function forget(notice) {
    clearTimeout(timers.get(notice.id));
    timers.delete(notice.id);
    elements.get(notice.id)?.remove();
    elements.delete(notice.id);
  }

  function close(notice) {
    const had = elements.get(notice.id)?.contains(document.activeElement);
    model.close(notice.id);
    forget(notice);
    if (had) document.querySelector('main h1')?.focus();
  }

  function start(notice) {
    clearTimeout(timers.get(notice.id));
    const wait = lifetime(notice);
    if (wait !== null) timers.set(notice.id, setTimeout(() => close(notice), wait));
  }

  function draw(notice) {
    const body = h('span', { class: 'notice-words' }, notice.words);
    const element = h('div', { class: `notice ${notice.kind}` },
      icon(ICONS[notice.kind]),
      body,
      notice.action && h('button', {
        class: 'btn small', type: 'button',
        onclick: () => {
          close(notice);
          notice.action.run();
        },
      }, notice.action.label),
      h('button', { class: 'btn small link', type: 'button', onclick: () => close(notice) }, 'Close'));
    element.addEventListener('mouseenter', () => clearTimeout(timers.get(notice.id)));
    element.addEventListener('mouseleave', () => start(notice));
    element.addEventListener('focusin', () => clearTimeout(timers.get(notice.id)));
    element.addEventListener('focusout', () => start(notice));
    elements.set(notice.id, element);
    region.append(element);
    start(notice);
  }

  return (words, options) => {
    const { notice, fresh, dropped } = model.add(words, options);
    for (const gone of dropped) forget(gone);
    if (fresh) draw(notice);
    else start(notice);
    return () => close(notice);
  };
}
