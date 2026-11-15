// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Overview's dialogs, which hold more than a question: the whole list of
// what needs a look, how the score is worked out, one change with its diffs,
// and the question before a change is put back. Each is a modal <dialog>
// named by its heading, made as the shell's own are.

import { h, uniqueId } from '../../dom.js';
import {
  bandRanges, FULL, OTHER_WEIGHT, SERVICE_CAP, weightRows,
} from '../../lib/health.js';
import { diffLines } from '../../lib/panel-model.js';
import { head, openModal } from '../../shell/dialogs.js';
import { linkable } from './card.js';
import { byWords, checkedWords, SEVERITY_WORDS } from './model.js';

/** A dialog named by `title` that closes with `buttons` in its foot, holding `body`. */
function dialog(title, iconName, body, buttons, { medium = false } = {}) {
  const id = uniqueId('ov-dialog');
  return h('dialog', { class: `dialog${medium ? ' medium' : ''}`, 'aria-labelledby': id },
    h('form', { method: 'dialog' },
      head(id, iconName, title),
      h('div', { class: 'dialog-body' }, ...body),
      h('div', { class: 'dialog-foot' }, ...buttons)));
}

const close = (words = 'Close') => h('button', { class: 'btn default', type: 'submit', value: 'close' }, words);

/** The whole list of what needs a look, with each thing's detail and a link where there is a page for it. */
export function seeAllDialog(rows, time) {
  const items = rows.map((row) => h('li', { class: 'all-row' },
    h('b', {}, SEVERITY_WORDS[row.severity], row.title),
    (row.detail || row.at !== null) && h('span', { class: 'muted small' }, [row.at === null ? null : time.when(row.at), row.detail].filter(Boolean).join(' · ')),
    linkable(row.href) && h('a', { href: row.href }, 'Open')));
  return openModal(dialog(`${rows.length} things to look at`, 'info', [h('ul', { class: 'all-list' }, ...items)], [close()]));
}

/** How the score is worked out: the rules in a few sentences, the table of weights and when the checks last ran. */
export function scoreDialog(assessed, time) {
  const checked = assessed ? checkedWords(assessed.checked, time) : null;
  const table = h('div', { class: 'tbl-box', role: 'region', tabindex: '0', 'aria-label': 'What each check takes off' },
    h('table', { class: 'tbl' },
      h('caption', { class: 'sr-only' }, 'What each check takes off the score'),
      h('thead', {}, h('tr', {}, h('th', { scope: 'col' }, 'Check'), h('th', { scope: 'col' }, 'What it looks for'), h('th', { scope: 'col', class: 'num' }, 'Takes off'))),
      h('tbody', {}, ...weightRows().map((row) => h('tr', {},
        h('th', { scope: 'row', class: 'mono' }, row.id), h('td', {}, row.words), h('td', { class: 'num' }, String(row.weight)))))));
  const bands = bandRanges().map((band) => `${band.words} is ${band.range}`);
  return openModal(dialog('How the score works', 'info', [
    h('p', {}, `The score starts at ${FULL}. Each thing the checks find takes off its weight, and a check that failed takes off twice its weight. It never goes below 0.`),
    h('p', {}, `${bands.join('; ')}. The words say what the colour says.`),
    table,
    h('p', { class: 'small muted' }, `A check this table does not know takes off ${OTHER_WEIGHT}, and a service that gave up takes off at most ${SERVICE_CAP} in all.`),
    checked && h('p', { class: 'small' }, checked),
  ], [close()]));
}

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
