// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The palette's result view (design 11.4): what a read action answered, as a
// table of the first 50 rows of a list, or as the facts of one thing, with
// the JSON in a fold. result-model.js decides what there is to show. Every
// word of the answer is set as text.

import { h, uniqueId } from '../dom.js';
import { describeAnswer, tableWords } from './result-model.js';

/** The table of a list. It scrolls as a region that takes focus, so that a keyboard reaches a wide one. */
function tableOf(shown) {
  const id = uniqueId('pal-table');
  return [
    h('p', { id, class: 'small muted' }, tableWords(shown)),
    h('div', { class: 'tbl-box', role: 'region', 'aria-labelledby': id, tabindex: '0' },
      h('table', { class: 'tbl' },
        h('thead', {}, h('tr', {}, ...shown.columns.map((column) => h('th', { scope: 'col' }, column.label)))),
        h('tbody', {}, ...shown.rows.map((row) => h('tr', {}, ...row.map((text, at) => h('td', { 'data-label': shown.columns[at].label }, text))))))),
  ];
}

/** The view for `answer`, as an element to put in the palette. */
export function resultView(answer) {
  const shown = describeAnswer(answer);
  let body;
  if (shown.kind === 'table') {
    body = tableOf(shown);
  } else if (shown.kind === 'text') {
    body = h('pre', { class: 'cmdtext', tabindex: '0' }, shown.text);
  } else if (shown.facts.length > 0) {
    body = h('dl', { class: 'rows' }, ...shown.facts.flatMap(([label, value]) => [h('dt', {}, label), h('dd', {}, value)]));
  } else {
    body = h('p', { class: 'muted' }, 'The answer has nothing to list. It is in the JSON below.');
  }
  return h('div', { class: 'pal-result' },
    body,
    shown.kind !== 'text' && h('details', { class: 'pal-json' },
      h('summary', {}, 'The answer as JSON'),
      h('pre', { class: 'cmdtext', tabindex: '0', 'aria-label': 'The answer as JSON' }, shown.json)));
}
