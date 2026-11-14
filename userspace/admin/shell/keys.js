// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The shell's keys (design 4.5). There are no single-character shortcuts,
// so nothing needs a switch to turn them off (WCAG 2.1.4). A terminal pane
// keeps Ctrl+K and Ctrl+/ for the program in it; its own way to the palette
// is Alt+Shift+K.

/**
 * What a key press asks for: `'palette'`, `'shortcuts'` or null. `event`
 * has `key`, `code`, `ctrlKey`, `metaKey`, `altKey` and `shiftKey`;
 * `inTerminal` says whether focus is in a terminal pane.
 */
export function keyAction(event, inTerminal) {
  const command = event.ctrlKey || event.metaKey;
  if (inTerminal) {
    return event.altKey && event.shiftKey && !command && event.code === 'KeyK' ? 'palette' : null;
  }
  if (command && !event.altKey && !event.shiftKey && event.key.toLowerCase() === 'k') return 'palette';
  if (command && !event.altKey && !event.shiftKey && event.key === '/') return 'shortcuts';
  return null;
}

/**
 * Follows the keys until the function it gives is called. A person who
 * signs out and in again gets a new shell in the same page; the old one's
 * listener must go, or one Ctrl+K opens the palette and shuts it again.
 */
export function followKeys({ openPalette, openShortcuts }) {
  const follow = (event) => {
    const action = keyAction(event, Boolean(event.target.closest?.('[data-terminal]')));
    if (!action) return;
    event.preventDefault();
    if (action === 'palette') openPalette();
    else openShortcuts();
  };
  document.addEventListener('keydown', follow);
  return () => document.removeEventListener('keydown', follow);
}
