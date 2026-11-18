// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Overview's dialogs, which hold more than a question: the whole list of
// what needs a look, and how the score is worked out. Each is a modal
// <dialog> named by its heading, made as the shell's own are.

import { h } from '../../dom.js';
import {
  bandRanges, FULL, OTHER_WEIGHT, SERVICE_CAP, weightRows,
} from '../../lib/health.js';
import { openModal } from '../../shell/dialogs.js';
import { linkable } from './card.js';
import { close, dialog } from './change-dialogs.js';
import { checkedWords, SEVERITY_WORDS } from './model.js';

/** The whole list of what needs a look, with each thing's detail and a link where there is a page for it. A row's `data-need` is its id, as in the card's list. */
export function seeAllDialog(rows, time) {
  const items = rows.map((row) => h('li', { class: 'all-row', 'data-need': row.id },
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
