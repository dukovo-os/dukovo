// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A menu button (design 4.6) for the header. The button and its closed list
// are made at start; shell/menu.js, which opens and moves through the list,
// is read when the button is first pressed.

import { h } from '../dom.js';

let made = 0;

/**
 * Makes a menu. `label` is the menu's name; `content` is what the button
 * shows; `items()` gives the items (shell/menu.js lists the kinds) each time
 * it opens. Gives `{element, button}`.
 */
export function menu({ label, content, items }) {
  made += 1;
  const id = `menu-${made}`;
  const button = h('button', {
    class: 'topbtn', type: 'button', 'aria-haspopup': 'menu', 'aria-expanded': 'false', 'aria-controls': id,
  }, ...content);
  const popup = h('div', { id, class: 'menu', role: 'menu', 'aria-label': label, hidden: true });
  const element = h('span', { class: 'menu-box' }, button, popup);

  const first = async (event) => {
    if (event.type === 'keydown' && event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    event.preventDefault();
    button.removeEventListener('click', first);
    button.removeEventListener('keydown', first);
    const { attach } = await import('./menu.js');
    const { open } = attach({ button, popup, element, items });
    open();
    if (event.key === 'ArrowUp') [...popup.querySelectorAll('[role^="menuitem"]')].at(-1)?.focus();
  };
  button.addEventListener('click', first);
  button.addEventListener('keydown', first);
  return { element, button };
}
