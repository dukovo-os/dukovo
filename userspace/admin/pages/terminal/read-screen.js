// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Read the screen (design 13.10): the last 500 lines of a pane's scrollback
// and its screen as text in a read-only text area, where a screen reader
// moves by line and word and the person can select and copy.

import { h, uniqueId } from '../../dom.js';
import { COPIED, copyText, PRESS_CTRL_C } from '../../lib/copy.js';
import { selectionText } from './term/select.js';

const LINES = 500;

/** The text a pane shows: the screen and up to LINES lines above it, without the empty rows below. */
export function screenText(screen) {
  const first = Math.max(screen.firstLine, screen.scrolled - LINES);
  const last = screen.scrolled + screen.rows - 1;
  return selectionText(screen, { start: { line: first, col: 0 }, end: { line: last, col: screen.cols - 1 } }).trimEnd();
}

/** Opens the dialog for `pane`; focus comes back to where it was when the dialog closes. */
export function readScreen(pane) {
  const id = uniqueId('read');
  const text = screenText(pane.screen);
  const area = h('textarea', {
    class: 'input mono tr-read', id: `${id}-text`, readonly: true, rows: '18', 'aria-label': 'The screen and the lines above it', spellcheck: 'false',
  });
  area.value = text === '' ? 'The screen is empty.' : text;
  const said = h('p', { class: 'hint', role: 'status' });
  const dialog = h('dialog', { class: 'dialog', 'aria-labelledby': id },
    h('form', { method: 'dialog' },
      h('div', { class: 'dialog-head' }, h('h2', { id }, 'Read the screen')),
      h('div', { class: 'dialog-body' },
        h('p', { class: 'hint' }, 'The last 500 lines and the screen, as text. Move by line and word, select, and copy.'), area, said),
      h('div', { class: 'dialog-foot' },
        h('button', {
          class: 'btn',
          type: 'button',
          async onclick() {
            said.textContent = (await copyText(area.value, area)) ? COPIED : PRESS_CTRL_C;
          },
        }, 'Copy all'),
        h('button', { class: 'btn default', type: 'submit' }, 'Close'))));
  dialog.addEventListener('close', () => {
    dialog.remove();
    pane.focus();
  });
  document.body.append(dialog);
  dialog.showModal();
  area.focus();
  area.setSelectionRange(area.value.length, area.value.length);
  area.scrollTop = area.scrollHeight;
}
