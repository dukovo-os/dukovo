// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The range control of a chart's card: a group of radio buttons that the
// arrow keys move between, for the ranges of lib/chart-model.js.

import { h } from '../dom.js';
import { RANGES } from './chart-model.js';

/** The control with `current` chosen; `pick(id)` is called with the word of the range each time one is chosen. */
export function rangeControl(current, pick) {
  const buttons = RANGES.map((range) => h('button', {
    type: 'button', role: 'radio', 'aria-checked': String(range.id === current), tabindex: range.id === current ? '0' : '-1',
    class: range.id === current ? 'on' : '', 'data-range': range.id,
  }, range.label));
  const choose = (button, focus) => {
    for (const other of buttons) {
      const on = other === button;
      other.setAttribute('aria-checked', String(on));
      other.tabIndex = on ? 0 : -1;
      other.classList.toggle('on', on);
    }
    if (focus) button.focus();
    pick(button.dataset.range);
  };
  const group = h('span', { class: 'seg', role: 'radiogroup', 'aria-label': 'Range' }, ...buttons);
  for (const button of buttons) button.addEventListener('click', () => choose(button, false));
  group.addEventListener('keydown', (event) => {
    const at = buttons.indexOf(document.activeElement);
    const to = { ArrowRight: at + 1, ArrowDown: at + 1, ArrowLeft: at - 1, ArrowUp: at - 1, Home: 0, End: buttons.length - 1 }[event.key];
    if (at < 0 || to === undefined) return;
    event.preventDefault();
    choose(buttons[(to + buttons.length) % buttons.length], true);
  });
  return group;
}
