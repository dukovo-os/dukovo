// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a menu does once it is opened (design 4.6): its items as
// `role="menu"`, which the arrow keys, Home and End move through, Esc
// closes with focus back on the button, and Tab leaves. shell/menus.js
// makes the button and reads this file the first time one is used.

import { h } from '../dom.js';

/**
 * The menu item elements for `items`, each one of:
 *
 * - `{heading}`: words that name the menu's owner, not an item;
 * - `{separator: true}`;
 * - `{label, href}`: a link;
 * - `{label, onSelect}`: a command;
 * - `{label, radio: true, checked, onSelect}`: one of a group.
 */
function itemElements(items, close) {
  return items.map((item) => {
    if (item.separator) return h('hr', { role: 'separator' });
    if (item.heading) return h('div', { class: 'menu-head', role: 'presentation' }, item.heading);
    const role = item.radio ? 'menuitemradio' : 'menuitem';
    const attributes = {
      role, tabindex: '-1', class: 'menu-item', 'aria-checked': item.radio ? String(Boolean(item.checked)) : null,
    };
    const body = [item.radio && h('span', { class: 'menu-mark', 'aria-hidden': 'true' }, item.checked ? '●' : ''), item.label];
    const element = item.href
      ? h('a', { ...attributes, href: item.href }, ...body)
      : h('button', { ...attributes, type: 'button' }, ...body);
    // Focus goes back to the button first, so that a dialog that the item
    // opens, or the next page, finds the button as what had focus.
    element.addEventListener('click', () => {
      close();
      item.onSelect?.();
    });
    return element;
  });
}

/**
 * Gives the `button` and `popup` that shell/menus.js made their behaviour:
 * `items()` gives the items each time the menu opens, so that a choice
 * shows what is chosen now. Gives `{open, close}`.
 */
export function attach({ button, popup, element, items }) {
  const entries = () => [...popup.querySelectorAll('[role^="menuitem"]')];
  const move = (to) => {
    const all = entries();
    if (all.length > 0) all[(to + all.length) % all.length].focus();
  };

  function close({ focus = true } = {}) {
    if (popup.hidden) return;
    popup.hidden = true;
    button.setAttribute('aria-expanded', 'false');
    document.removeEventListener('pointerdown', outside, true);
    if (focus) button.focus();
  }

  function outside(event) {
    if (!element.contains(event.target)) close({ focus: false });
  }

  function open() {
    popup.replaceChildren(...itemElements(items(), close));
    popup.hidden = false;
    button.setAttribute('aria-expanded', 'true');
    document.addEventListener('pointerdown', outside, true);
    const first = popup.querySelector('[aria-checked="true"]') ?? entries()[0];
    first?.focus();
  }

  button.addEventListener('click', () => (popup.hidden ? open() : close()));
  button.addEventListener('keydown', (event) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (popup.hidden) open();
      if (event.key === 'ArrowUp') move(-1);
    }
  });
  popup.addEventListener('keydown', (event) => {
    const at = entries().indexOf(document.activeElement);
    const keys = {
      ArrowDown: () => move(at + 1),
      ArrowUp: () => move(at - 1),
      Home: () => move(0),
      End: () => move(-1),
      Escape: () => close(),
      Tab: () => close(),
    };
    const act = keys[event.key];
    if (!act) return;
    // Tab keeps its own way on: focus goes to the button first, so that
    // it moves on from there.
    if (event.key !== 'Tab') event.preventDefault();
    event.stopPropagation();
    act();
  });
  popup.addEventListener('focusout', (event) => {
    if (event.relatedTarget && !element.contains(event.relatedTarget)) close({ focus: false });
  });

  return { open, close };
}
