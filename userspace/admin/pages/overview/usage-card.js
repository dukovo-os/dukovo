// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Usage (design 10.3): four small charts, CPU, memory, disk and network, over
// a range the person picks, which is kept in the address (`?range=7d`). The
// numbers are open to every signed-in account. A machine that has kept no
// history yet says so, and says whether metricsd, which writes it, runs.

import { h } from '../../dom.js';
import { makeChart } from '../../lib/chart.js';
import {
  CHARTS, modelOf, RANGES, rangeOf, totalMemory,
} from '../../lib/chart-model.js';
import { makeCard, reader } from './card.js';
import { noNumbersWords } from './model.js';
import { usageDue } from './refresh.js';

/** The range control: a group of radio buttons, which the arrow keys move between. `pick(id)` is called when one is chosen. */
function rangeControl(current, pick) {
  const buttons = RANGES.map((range) => h('button', {
    type: 'button', role: 'radio', 'aria-checked': String(range.id === current), tabindex: range.id === current ? '0' : '-1', 'data-range': range.id,
  }, range.label));
  const group = h('span', { class: 'seg', role: 'radiogroup', 'aria-label': 'Range' }, ...buttons);
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
  buttons.forEach((button) => {
    button.classList.toggle('on', button.getAttribute('aria-checked') === 'true');
    button.addEventListener('click', () => choose(button, false));
  });
  group.addEventListener('keydown', (event) => {
    const at = buttons.indexOf(document.activeElement);
    const to = { ArrowRight: at + 1, ArrowDown: at + 1, ArrowLeft: at - 1, ArrowUp: at - 1, Home: 0, End: buttons.length - 1 }[event.key];
    if (at < 0 || to === undefined) return;
    event.preventDefault();
    choose(buttons[(to + buttons.length) % buttons.length], true);
  });
  return group;
}

/**
 * The card. `about()` gives `console.about`'s answer or null, for the memory
 * the machine has. Gives `{element, load, due}`: `due(now)` says whether the
 * range in view is to be read again, which the page asks each minute.
 */
export function makeUsage(view, { about }) {
  let range = rangeOf(view.query.get('range')).id;
  // "Usage, last 24 hours": the range's words without their "the".
  const titleWords = () => `Usage, ${rangeOf(range).words.replace(/^the /, '')}`;
  const card = makeCard('usage', titleWords());
  const charts = CHARTS.map((chart) => ({ chart, drawn: makeChart(chart.id) }));
  const grid = h('div', { class: 'usage-grid' }, ...charts.map((one) => one.drawn.element));
  const empty = h('p', { class: 'muted', role: 'status' });
  const foot = h('p', { class: 'muted small usage-foot' },
    'History is kept for two weeks at one-minute steps and a year as hourly averages, with nothing to install.');
  card.body.replaceChildren(grid, empty, foot);

  const rename = () => card.heading.firstChild.replaceWith(document.createTextNode(titleWords()));

  let lastRead = 0;
  const read = reader(view, card.body, {
    async read() {
      const now = Math.floor(Date.now() / 1000);
      const [answer, facts] = await Promise.all([
        view.api.get(`/api/1/metrics/usage/${range}`).catch((error) => {
          // metrics says it has no points on a machine that has kept none, which is no failure.
          if (error?.id === 'ADM-123') return null;
          throw error;
        }),
        about().catch(() => null),
      ]);
      const services = answer === null || (Array.isArray(answer) && answer.length === 0)
        ? await view.api.get('/api/1/services').catch(() => null) : null;
      return { answer, facts, services, now, wanted: range };
    },
    draw({ answer, facts, services, now, wanted }) {
      // The person chose another range while this was read: that read draws it.
      if (wanted !== range) return;
      // reader() may have replaced the body with a failure: put the charts back.
      if (!card.body.contains(grid)) card.body.replaceChildren(grid, empty, foot);
      const none = answer === null || !Array.isArray(answer) || answer.length === 0;
      grid.hidden = none;
      foot.hidden = none;
      empty.textContent = none ? noNumbersWords(services) : '';
      if (none) return;
      const total = totalMemory(answer, facts);
      for (const { chart, drawn } of charts) drawn.update(modelOf(chart, answer, { range: rangeOf(range), now, total }), view.time);
    },
  });

  const load = () => {
    lastRead = Date.now();
    return read();
  };

  const control = rangeControl(range, (id) => {
    range = id;
    rename();
    const url = new URL(location.href);
    url.searchParams.set('range', id);
    history.replaceState(null, '', `${url.pathname}${url.search}${url.hash}`);
    load();
  });
  card.more.append(control);
  return { element: card.element, load, due: (now = Date.now()) => usageDue(lastRead, now, range) };
}
