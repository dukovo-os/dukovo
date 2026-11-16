// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Preview the files first: each file that adding the site writes, with its
// path, whether it is new or changes what is there, and its text with line
// numbers. Nothing in it can be changed: it is what the command would write.
// The page reads this file when the button is pressed.

import { h, uniqueId } from '../../dom.js';
import { previewFiles } from '../../lib/website-model.js';
import { head, openModal } from '../../shell/dialogs.js';

/** Opens the panel for the files of `plan`; resolves when it closes. */
export function openPreview(plan) {
  const id = uniqueId('web-preview');
  const files = previewFiles(plan);
  const close = h('button', { class: 'btn default', type: 'submit', autofocus: true }, 'Close');
  const dialog = h('dialog', { class: 'dialog medium web-preview', 'aria-labelledby': `${id}-h` },
    h('form', { method: 'dialog' },
      head(`${id}-h`, 'code', 'The files that adding the site writes'),
      h('div', { class: 'dialog-body' },
        files.length === 0
          ? h('p', { class: 'muted' }, 'The plan writes no file.')
          : files.map((file) => h('section', { class: 'web-file', 'data-path': file.path },
            h('p', { class: 'web-file-head' }, h('b', { class: 'mono' }, file.path), ' ', h('span', { class: `pill ${file.state === 'new' ? 'ok' : 'warn'}` }, file.state === 'new' ? 'new' : 'changes')),
            h('pre', { class: 'cmdtext web-lines', tabindex: '0', 'aria-label': `Text of ${file.path}` },
              ...file.lines.map((line) => h('span', { class: 'web-line' }, h('span', { class: 'web-no', 'aria-hidden': 'true' }, `${line.number}`), line.text, '\n'))))),
        h('p', { class: 'small muted' }, 'Only a preview: nothing is written until Add the website is pressed.')),
      h('div', { class: 'dialog-foot' }, close)));
  return openModal(dialog);
}
