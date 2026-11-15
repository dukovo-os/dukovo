// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The head of a page (design 3.3): its h1, crumbs, buttons at the right and
// the Show commands switch. A page asks for it with `view.head()`; it never
// touches the header, the navigation or `document.title` itself.

import { fill, h } from '../dom.js';
import { titleFor } from './route-model.js';

/**
 * Draws the head into `region` for `options`: `{title, crumbs: [{title,
 * path}], buttons, commands, lead, pill: {words, kind}}`. `prefs` is
 * shell/prefs.js's; `host` the machine's name. Gives `{h1, update}`, where
 * `update(prefs)` brings the switch in line with the preferences.
 */
export function drawHead({ region, options, prefs, host }) {
  const { title, crumbs = [], buttons = [], commands = false, lead = null, pill = null } = options;
  document.title = titleFor(title, host);
  const h1 = h('h1', { tabindex: '-1' }, title);

  const switchButton = commands
    ? h('button', { class: 'showcmd', type: 'button', role: 'switch', 'aria-checked': 'false' },
      h('span', { class: 'sw', 'aria-hidden': 'true' }), 'Show commands')
    : null;
  const sync = (value) => {
    if (!switchButton) return;
    const on = value?.commands === 'shown';
    switchButton.setAttribute('aria-checked', String(on));
    switchButton.querySelector('.sw').classList.toggle('on', on);
  };
  if (switchButton) {
    sync(prefs.value);
    switchButton.addEventListener('click', () => {
      const next = prefs.value?.commands === 'shown' ? 'hidden' : 'shown';
      prefs.set({ commands: next }).catch(() => {});
    });
  }

  fill(region,
    h('div', { class: 'title' },
      crumbs.length > 0 && h('nav', { class: 'crumb', 'aria-label': 'Breadcrumb' },
        ...crumbs.flatMap((crumb) => [h('a', { href: crumb.path }, crumb.title), ' › ']), title),
      h1),
    pill && h('span', { class: `pill ${pill.kind ?? ''}`.trim() }, pill.words),
    h('span', { class: 'spacer' }),
    ...buttons,
    switchButton,
    lead && h('p', { class: 'lead' }, lead));
  return { h1, update: sync };
}
