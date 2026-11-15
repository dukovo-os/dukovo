// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The dialogs of one change of the journal: its diffs, and the question
// before it is put back, with the frame the Overview's other dialogs use.
// The Overview and the journal on Reliability both open them, so they are
// apart from the dialogs only the Overview has.

import { h, uniqueId } from '../../dom.js';
import { diffLines } from '../../lib/panel-model.js';
import { head, openModal } from '../../shell/dialogs.js';
import { byWords } from './change-model.js';

/** A dialog named by `title` that closes with `buttons` in its foot, holding `body`. */
export function dialog(title, iconName, body, buttons, { medium = false } = {}) {
  const id = uniqueId('ov-dialog');
  return h('dialog', { class: `dialog${medium ? ' medium' : ''}`, 'aria-labelledby': id },
    h('form', { method: 'dialog' },
      head(id, iconName, title),
      h('div', { class: 'dialog-body' }, ...body),
      h('div', { class: 'dialog-foot' }, ...buttons)));
}

/** The button that closes a dialog that only shows something. */
export const close = (words = 'Close') => h('button', { class: 'btn default', type: 'submit', value: 'close' }, words);

/** One file of a change: its path, and its diff or why there is none. */
function fileOf(file) {
  const lines = typeof file.diff === 'string' ? diffLines(file.diff) : null;
  return h('div', { class: 'file' },
    h('b', { class: 'mono' }, file.path),
    lines
      ? h('pre', { class: 'cmdtext diff', tabindex: '0', 'aria-label': `Change of ${file.path}` },
        ...lines.map((line) => h('span', { class: `line ${line.kind}` }, line.words && h('span', { class: 'sr-only' }, line.words), line.text, '\n')))
      : h('p', { class: 'small muted' }, `No diff is shown: ${file.no_diff ?? 'the console has no text of this file'}.`),
    file.diff_truncated && h('p', { class: 'small muted' }, 'The diff is cut short.'));
}

/** One change, `GET /api/1/changes/{id}`'s answer: who made it and when, and the diff of each file. */
export function changeDialog(change, time, title) {
  const words = [byWords(change), time.full(change.at)].join(' · ');
  const files = Array.isArray(change.files) ? change.files : [];
  return openModal(dialog(title, 'info', [
    h('p', { class: 'muted' }, words),
    change.note && h('p', {}, change.note),
    files.length > 0 ? h('div', { class: 'files' }, ...files.map(fileOf)) : h('p', { class: 'muted' }, 'This change touched no file.'),
  ], [close()]));
}

/**
 * The question before a change is put back: what it puts back, and the
 * command that does it, which the console runs as an administrator. Gives
 * true when the person chooses to put it back.
 */
export async function putBackDialog({ title, command, files }) {
  const choice = await openModal(dialog(`Put back: ${title}`, 'warn', [
    h('p', {}, 'The earlier text of ', files.length === 1 ? 'this file' : `these ${files.length} files`, ' is written back. What was changed since is lost.'),
    h('ul', { class: 'all-list' }, ...files.map((path) => h('li', {}, h('span', { class: 'mono' }, path)))),
    h('p', { class: 'small muted' }, 'The console runs this command as an administrator:'),
    h('pre', { class: 'cmdtext', tabindex: '0' }, command),
  ], [h('button', { class: 'btn danger', type: 'submit', value: 'put' }, 'Put it back'), h('button', { class: 'btn', type: 'submit', value: 'cancel' }, 'Cancel')], { medium: true }));
  return choice === 'put';
}
