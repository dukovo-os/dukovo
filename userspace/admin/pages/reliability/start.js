// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// One start (design 7.3) at /reliability/starts/N: the steps of the start
// beside those of the last good start, or of any other start that has its
// times, and the services that took longest. A start that has no times, or
// that the machine does not know, says so.

import { fill, h } from '../../dom.js';
import { makeReads } from '../overview/admin-reads.js';
import { failureWords, makeCard } from '../overview/card.js';
import {
  chainWords, hasNoTimes, NO_TIMES, resultOf, slowestOf, startRows, stepRows, tryNumbers, versionWords,
} from './starts-model.js';

/** The select's value for the last good start, which the answer has already, and for no other start. */
const LAST_GOOD = 'good';
const NOTHING = 'none';

/** The page for the start `number`. */
export async function show(view, number) {
  view.head({ title: `Start ${number}`, crumbs: [{ title: 'Reliability', path: '/reliability' }] });
  const reads = makeReads(view);
  const facts = makeCard('start', `Start ${number}`);
  const steps = makeCard('steps', 'Where the time went');
  const slow = makeCard('slow', 'What took longest');
  view.main.append(h('div', { class: 'rel' }, facts.element, steps.element, slow.element));
  let latest = 0;

  /** The card of the facts of the start, from the list of starts, which may not have been read. */
  function drawFacts(list) {
    const start = list.state === 'ok' && Array.isArray(list.value) ? list.value.find((one) => one.start === number) : null;
    if (!start) {
      fill(facts.body, h('p', { class: 'muted' }, 'The machine does not say more of this start.'));
      return;
    }
    const result = resultOf(start, tryNumbers(list.value));
    const row = startRows([start], view.time)[0];
    fill(facts.body, h('dl', { class: 'rows' },
      h('dt', {}, 'Started'), h('dd', {}, Number.isFinite(start.at_ms) ? view.time.full(start.at_ms / 1000) : 'Time not known'),
      h('dt', {}, 'Version'), h('dd', {}, versionWords(start) || 'Not known'),
      h('dt', {}, 'Ready in'), h('dd', {}, row.ready),
      h('dt', {}, 'Result'), h('dd', {}, result.words || 'Not known')));
  }

  /** The steps table with `against`, the start compared with, and a select that chooses another. */
  function drawSteps(answer, list, against, picked, { say = null, focus = false } = {}) {
    const rows = stepRows(answer, against);
    const others = (list.state === 'ok' && Array.isArray(list.value) ? list.value : []).filter((one) => one.start !== number);
    const select = h('select', { id: 'rel-against', class: 'input' },
      h('option', { value: LAST_GOOD, selected: picked === LAST_GOOD }, answer.compared_with ? `The last good start (start ${answer.compared_with.start})` : 'The last good start'),
      h('option', { value: NOTHING, selected: picked === NOTHING }, 'No other start'),
      ...others.map((one) => h('option', { value: String(one.start), selected: picked === String(one.start) },
        `Start ${one.start}${Number.isFinite(one.at_ms) ? `, ${view.time.when(one.at_ms / 1000)}` : ''}`)));
    select.addEventListener('change', () => compareWith(answer, list, select.value));
    const columns = ['Step', 'In this start', ...(against ? [`In start ${against.start}`, 'Change'] : [])];
    fill(steps.body,
      h('div', { class: 'field rel-against' }, h('label', { for: 'rel-against' }, 'Compare with'), select),
      say && h('p', { class: 'muted', role: 'status' }, say),
      h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack rel-tbl' },
        h('caption', { class: 'sr-only' }, `The steps of start ${number}`),
        h('thead', {}, h('tr', {}, ...columns.map((words, at) => h('th', { scope: 'col', class: at === 0 ? '' : 'num' }, words)))),
        h('tbody', {}, ...rows.map((row) => h('tr', { class: row.worse ? 'worse' : '' },
          h('th', { scope: 'row' }, row.name),
          h('td', { class: 'num', 'data-label': 'In this start' }, row.now),
          against && h('td', { class: 'num', 'data-label': `In start ${against.start}` }, row.then),
          against && h('td', { class: 'num', 'data-label': 'Change' }, row.change, row.worse && h('span', { class: 'sr-only' }, ', slower'))))))));
    if (focus) select.focus();
  }

  /** Reads another start's steps and draws the table beside them. */
  async function compareWith(answer, list, value) {
    if (value === LAST_GOOD) {
      drawSteps(answer, list, answer.compared_with, LAST_GOOD, { focus: true });
      return;
    }
    if (value === NOTHING) {
      drawSteps(answer, list, null, NOTHING, { focus: true });
      return;
    }
    const result = await reads.read(`/api/1/starts/${value}`);
    if (view.signal.aborted) return;
    if (result.state === 'ok') {
      drawSteps(answer, list, { start: Number(value), steps: result.value.steps ?? [] }, value, { focus: true });
    } else {
      const words = result.state === 'error' && hasNoTimes(result.error) ? `No times are kept for start ${value}.` : result.state === 'error' ? failureWords(result.error) : 'This start cannot be compared with.';
      drawSteps(answer, list, answer.compared_with, LAST_GOOD, { say: words, focus: true });
    }
  }

  function drawTimes(answer, list) {
    drawSteps(answer, list, answer.compared_with, answer.compared_with ? LAST_GOOD : NOTHING, {
      say: answer.compared_with ? null : 'No good start with times is kept to compare with.',
    });
    const slowest = slowestOf(answer);
    const chain = chainWords(answer);
    fill(slow.body,
      slowest.length === 0 ? h('p', { class: 'muted' }, 'No service of this start has a time.') : h('ul', { class: 'rel-slowest' }, ...slowest.map((one) => h('li', {}, h('b', {}, one.name), ` took ${one.took} to be ready`))),
      chain && h('p', { class: 'small muted' }, `What the start waited for, in order: ${chain}.`));
  }

  async function load() {
    latest += 1;
    const mine = latest;
    const [list, answer] = await Promise.all([reads.read('/api/1/starts'), reads.read(`/api/1/starts/${number}`)]);
    if (view.signal.aborted || mine !== latest) return;
    const known = list.state === 'ok' && Array.isArray(list.value) && list.value.some((one) => one.start === number);
    const unknown = list.state === 'ok' && Array.isArray(list.value) && !known;
    drawFacts(list);
    if (unknown) {
      const words = `The machine has no record of start ${number}.`;
      fill(steps.body, h('p', { class: 'muted' }, words), h('div', { class: 'btns' }, h('a', { class: 'btn small', href: '/reliability' }, 'Open Reliability')));
      slow.element.hidden = true;
      return;
    }
    slow.element.hidden = false;
    if (answer.state === 'ok') {
      drawTimes(answer.value, list);
    } else if (answer.state === 'error' && hasNoTimes(answer.error)) {
      fill(steps.body, h('p', { class: 'muted' }, NO_TIMES));
      slow.element.hidden = true;
    } else {
      fill(steps.body, reads.note(answer, 'the start\'s times', load));
      slow.element.hidden = true;
    }
  }

  view.on('unlock', () => {
    reads.forget();
    load();
  });
  await load();
}
