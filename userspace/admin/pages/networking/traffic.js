// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Traffic card of the Networking page (design 9.3): the bits a second each
// card received and sent, over a range the person picks, which is kept in the
// address (`?range=7d`) as the Overview's is. The numbers are open to every
// signed-in account. One chart for each card, drawn again each time the
// history is read.

import { h } from '../../dom.js';
import { makeChart } from '../../lib/chart.js';
import { modelOf, RANGES, rangeOf } from '../../lib/chart-model.js';
import { usageDue } from '../overview/refresh.js';
import { trafficChart } from './network-model.js';

/** How often the card asks whether the range in view is due to be read again. */
const CHECK_MS = 30_000;

/** The range control: radio buttons that the arrow keys move between. `pick(id)` is called when one is chosen. */
function rangeControl(current, pick) {
  const buttons = RANGES.map((range) => h('button', {
    type: 'button', role: 'radio', 'aria-checked': String(range.id === current), tabindex: range.id === current ? '0' : '-1',
    class: range.id === current ? 'on' : '', 'data-range': range.id,
  }, range.label));
  const choose = (button, focus) => {
    for (const other of buttons) {
      const on = other === button;
      other.setAttribute('aria-checked', String(on));
      other.tabIndex = on ? 0 : -1;
      other.classList.toggle('on', on);
    }
    if (focus) button.focus();
    pick(button.dataset.range);
  };
  const group = h('span', { class: 'seg', role: 'radiogroup', 'aria-label': 'Range' }, ...buttons);
  for (const button of buttons) button.addEventListener('click', () => choose(button, false));
  group.addEventListener('keydown', (event) => {
    const at = buttons.indexOf(document.activeElement);
    const to = { ArrowRight: at + 1, ArrowDown: at + 1, ArrowLeft: at - 1, ArrowUp: at - 1, Home: 0, End: buttons.length - 1 }[event.key];
    if (at < 0 || to === undefined) return;
    event.preventDefault();
    choose(buttons[(to + buttons.length) % buttons.length], true);
  });
  return group;
}

/** The card. `set(names)` says which cards the machine has; the history is read when the first names come and when the range changes. */
export function makeTraffic(view) {
  let range = rangeOf(view.query.get('range')).id;
  let names = [];
  let latest = 0;
  let lastRead = 0;
  let answer = null;
  const heading = h('h2', { id: 'net-traffic-h' });
  const errors = h('div', { class: 'net-errors' });
  const empty = h('p', { class: 'muted', role: 'status' });
  const grid = h('div', { class: 'net-charts' });
  const element = h('section', { class: 'card net-traffic', 'aria-labelledby': 'net-traffic-h', hidden: true }, heading, errors, empty, grid);
  const charts = new Map();

  const title = () => `Traffic, ${rangeOf(range).words.replace(/^the /, '')}`;
  const retitle = () => {
    heading.replaceChildren(title(), h('span', { class: 'more' }, control));
  };
  const control = rangeControl(range, (id) => {
    range = id;
    retitle();
    const url = new URL(location.href);
    url.searchParams.set('range', id);
    history.replaceState(null, '', `${url.pathname}${url.search}${url.hash}`);
    load();
  });
  retitle();

  /** Puts the numbers of `answer` in the charts. */
  function draw() {
    const none = answer === null || !Array.isArray(answer) || answer.length === 0;
    grid.hidden = none;
    empty.textContent = none ? 'The machine has kept no history of traffic yet.' : '';
    if (none) return;
    const now = Math.floor(Date.now() / 1000);
    for (const [name, drawn] of charts) drawn.update(modelOf(trafficChart(name), answer, { range: rangeOf(range), now }), view.time);
  }

  /** Reads the history of `range`. Only the last read counts. */
  async function load() {
    if (names.length === 0) return;
    latest += 1;
    const mine = latest;
    lastRead = Date.now();
    try {
      const found = await view.api.get(`/api/1/metrics/usage/${range}`).catch((error) => {
        // metrics says it has no points on a machine that has kept none, which is no failure.
        if (error?.id === 'ADM-123') return null;
        throw error;
      });
      if (mine !== latest) return;
      errors.replaceChildren();
      answer = found;
      draw();
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return;
      view.problem(error, { slot: errors });
    }
  }

  view.every(CHECK_MS, () => {
    if (usageDue(lastRead, Date.now(), range)) load();
  });

  return {
    element,
    set(cards) {
      const next = cards.map((card) => card.name);
      if (next.join('\n') === names.join('\n')) return;
      names = next;
      element.hidden = names.length === 0;
      charts.clear();
      grid.replaceChildren(...names.map((name, at) => {
        const drawn = makeChart(`net-${at}`);
        charts.set(name, drawn);
        return h('div', { class: 'net-chart' }, h('h3', {}, name), drawn.element);
      }));
      if (answer !== null) draw();
      load();
    },
  };
}
