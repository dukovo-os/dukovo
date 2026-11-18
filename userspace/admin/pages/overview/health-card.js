// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Health card (design 10.1): the score as a ring with its band in words,
// "N things to look at" with the first four, and what to do when there is no
// score to show. The list is the pages' needs and the checks that did not
// pass; model.js says what goes in it.

import { fill, h, icon } from '../../dom.js';
import { readChecks } from '../../lib/checks.js';
import { commandBox, linkable, makeCard, reader } from './card.js';
import { seeAllDialog, scoreDialog } from './dialogs.js';
import { healthCard, lookWords, SEVERITY_WORDS } from './model.js';

/** The score's ring, with its number and its band in words for a screen reader. */
function ring(card) {
  const label = card.score === null ? 'No health score' : `Health score ${card.score} out of 100, ${card.band.words}`;
  const element = h('div', { class: `score ${card.tone}`, role: 'img', 'aria-label': label },
    h('span', {}, card.score === null ? '–' : String(card.score), card.score !== null && h('small', {}, 'of 100')));
  element.style.setProperty('--v', String(card.score ?? 0));
  return element;
}

/** One row of the list: an icon, its title and detail, and a link when there is a page. Its `data-need` is the row's id, which names it when another row has the same title. */
function rowOf(row, time) {
  const words = h('span', { class: 'need-words' },
    h('b', {}, h('span', { class: 'sr-only' }, SEVERITY_WORDS[row.severity]), row.title),
    (row.detail || row.at !== null) && h('small', {}, [row.at === null ? null : time.when(row.at), row.detail].filter(Boolean).join(' · ')));
  const mark = icon(row.icon);
  mark.classList.add(row.severity === 'info' ? 'info' : row.severity);
  const body = linkable(row.href)
    ? h('a', { class: 'need-row', href: row.href }, mark, words, h('span', { class: 'go', 'aria-hidden': 'true' }, '›'))
    : h('div', { class: 'need-row' }, mark, words);
  return h('li', { 'data-need': row.id }, body);
}

/** The card, which reads again with `load()`. */
export function makeHealth(view) {
  const card = makeCard('health', 'Health');
  const explain = h('button', { class: 'btn link small', type: 'button' }, 'How the score works');
  card.more.append(explain);
  let current = null;
  explain.addEventListener('click', () => scoreDialog(current?.checks.state === 'ok' ? current.checks.assessed : null, view.time));

  const load = reader(view, card.body, {
    async read() {
      const [checks, found] = await Promise.all([
        readChecks(view),
        view.parts('overview', (part, cx) => part.needs?.(cx) ?? []),
      ]);
      return { checks, needs: found.flatMap((one) => (Array.isArray(one.value) ? one.value : [])) };
    },
    draw({ checks, needs }) {
      current = { checks };
      const model = healthCard(checks, needs);
      const more = model.more > 0
        && h('button', { id: 'ov-see-all', class: 'btn link small see-all', type: 'button', onclick: () => seeAllDialog(model.rows, view.time) }, `See all ${model.more}`);
      fill(card.body,
        h('div', { class: 'health-top' },
          ring(model),
          h('div', { class: 'health-words' },
            h('span', { class: `st ${model.tone}` }, model.line),
            h('span', { class: 'muted look' }, lookWords(model.count)))),
        model.command && h('div', { class: 'health-command' },
          h('p', { class: 'small muted' }, 'Run the checks at a terminal. An administrator\'s sign-in is asked for.'),
          commandBox(model.command)),
        model.shown.length > 0 && h('ul', { class: 'need' }, ...model.shown.map((row) => rowOf(row, view.time))),
        more);
    },
  });
  return { element: card.element, load };
}
