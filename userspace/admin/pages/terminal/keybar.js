// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The key bar's buttons (design 13.10), made when the bar is first shown:
// on a phone always, on a wide screen behind the Keys button. The sticky
// Ctrl and Alt are keybar-model.js's.

import { h } from '../../dom.js';
import { KEYS, KeyBar } from './keybar-model.js';

export { KeyBar };

/**
 * Fills `element` with the keys. `bar` is the KeyBar; `press(bytes)` sends
 * the bytes of a key to the pane that has focus; `modes()` gives its
 * screen's modes. Gives `sync()`, which shows which sticky key waits.
 */
export function fillKeys(element, bar, { press, modes, refocus }) {
  const sync = () => {
    for (const key of element.children) if (key.dataset.key === 'ctrl' || key.dataset.key === 'alt') key.setAttribute('aria-pressed', String(bar[key.dataset.key]));
  };
  element.replaceChildren(...KEYS.map((key) => h('button', {
    class: 'tr-key', type: 'button', 'aria-label': key.name, 'data-key': key.id, 'aria-pressed': key.sticky ? 'false' : null,
    // The key bar never takes focus from the pane, so the soft keyboard stays.
    onmousedown: (event) => event.preventDefault(),
    onclick: () => {
      const bytes = bar.press(key.id, modes());
      if (bytes !== null) press(bytes);
      sync();
      refocus();
    },
  }, key.label)));
  sync();
  return sync;
}
