// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The sign-in notices (design 10, 9.3): what the machine kept for the next
// sign-in, such as a start that fell back to the earlier version, shown as the
// terminal shows it. Dismiss hides one in this browser only, by its name and
// a fingerprint of its words, so that the same notice with other words is
// shown again. Pages add buttons to a notice through their `noticeButtons`
// part.

import { h, icon } from '../../dom.js';
import { keep, recall } from '../../lib/store.js';
import { linkable, reader } from './card.js';
import { hide, visibleNotices } from './model.js';

const HIDDEN = 'notices.hidden';

/** The buttons the pages give a notice named `name`: `[{label, href}]`, those whose page is shown. */
async function buttonsFor(view, name) {
  const found = await view.parts('overview', (part) => part.noticeButtons?.(name) ?? []);
  return found.flatMap((one) => (Array.isArray(one.value) ? one.value : [])).filter((button) => button?.label && linkable(button.href));
}

/** The place the notices go, which has nothing in it, and so no height, while there are none. */
export function makeNotices(view) {
  const element = h('div', { class: 'ov-notices' });

  function dismiss(notice, banner) {
    keep(HIDDEN, hide(recall(HIDDEN, []), notice.key));
    const next = banner.nextElementSibling ?? banner.previousElementSibling;
    banner.remove();
    (next?.querySelector('button') ?? view.main.closest('main')?.querySelector('h1'))?.focus();
  }

  async function bannerOf(notice) {
    const [first, ...rest] = notice.lines;
    const buttons = (await buttonsFor(view, notice.name)).map((button, at) => h('a', { class: `btn${at === 0 ? ' default' : ''}`, href: button.href }, button.label));
    const banner = h('div', { class: `banner ${notice.kind}`, role: 'status', 'data-notice': notice.name });
    const dismisser = h('button', { class: 'btn link', type: 'button', onclick: () => dismiss(notice, banner) }, 'Dismiss');
    banner.append(
      icon(notice.kind === 'warn' ? 'warn' : 'info'),
      h('div', {},
        h('p', {}, h('b', {}, first), rest.length > 0 && ' ', rest.join(' ')),
        h('div', { class: 'btns' }, ...buttons, dismisser)));
    return banner;
  }

  const load = reader(view, element, {
    // A notice is a courtesy: a read that fails says nothing here, and the shell says the server is away.
    read: () => view.api.get('/api/1/console/notices').catch((error) => {
      if (error?.name === 'AbortError') throw error;
      return { notices: [] };
    }),
    async draw(answer) {
      const shown = visibleNotices(answer?.notices, recall(HIDDEN, []));
      element.replaceChildren(...(await Promise.all(shown.map(bannerOf))));
    },
  });
  return { element, load };
}
