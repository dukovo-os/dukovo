// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Use card of the page of one service (design 4.4): what its resource
// domain has used and been held to, which the page reads with the service
// every few seconds, and charts of its processor and memory over a range
// the person picks, which are read when the card opens and when the range
// changes (the address keeps it as `?range=`). The chart is read when the
// first history comes, so that a page of a service that never has one
// does not load it.

import { fill, h } from '../../dom.js';
import { rangeOf } from '../../lib/chart-model.js';
import { rangeControl } from '../../lib/range-control.js';
import { failureWords } from '../overview/card.js';
import { rowsList, stable } from './cards.js';
import {
  chartsOf, modelsOf, noHistory, NO_HISTORY, USE_FOOT, useRows,
} from './use-model.js';

/** The card of the use of the service `name`: `{element, set(service)}`. `set` draws what the service's document says now and, the first time, reads the history. */
export function makeUse(view, name) {
  let range = rangeOf(view.query.get('range')).id;
  let service = null;
  let latest = 0;
  // The charts, made when the first history comes: `[{drawn}]` in the order of `chartsOf`.
  let charts = null;

  const numbers = h('div', { class: 'svc-use-numbers' });
  const setNumbers = stable(numbers);
  const message = h('p', { class: 'muted', role: 'status', hidden: true });
  const grid = h('div', { class: 'svc-charts', hidden: true });
  const control = rangeControl(range, (id) => {
    range = id;
    const url = new URL(location.href);
    url.searchParams.set('range', id);
    history.replaceState(null, '', `${url.pathname}${url.search}${url.hash}`);
    load();
  });
  const element = h('section', { class: 'card svc-use', id: 'use', 'aria-labelledby': 'use-h' },
    h('h2', { id: 'use-h' }, 'Use'), numbers, h('div', { class: 'btns' }, control), message, grid);

  /** Says why the charts are not drawn: the words, and a button to read again when the read failed. */
  function say(words, retry = null) {
    grid.hidden = true;
    message.hidden = false;
    fill(message, words, retry && [' ', h('button', { class: 'btn small', type: 'button', onclick: load }, 'Try again')]);
  }

  /** Reads the history over the range and draws the charts. Only the last read counts. */
  async function load() {
    latest += 1;
    const mine = latest;
    let answer;
    try {
      answer = await view.api.get(`/api/1/services/${encodeURIComponent(name)}/usage/${range}`);
    } catch (error) {
      if (view.signal.aborted || mine !== latest || error?.name === 'AbortError') return;
      // metrics ends with 1 when it has nothing to show, which is no failure.
      if (error?.id === 'ADM-123') {
        say(NO_HISTORY);
        return;
      }
      say(failureWords(error), true);
      return;
    }
    if (view.signal.aborted || mine !== latest) return;
    const models = modelsOf(service, answer, range, Math.floor(Date.now() / 1000));
    if (noHistory(models)) {
      say(NO_HISTORY);
      return;
    }
    if (charts === null) {
      const { makeChart } = await import('../../lib/chart.js');
      if (view.signal.aborted || mine !== latest) return;
      charts = chartsOf(service).map(({ chart }) => makeChart(chart.id));
      grid.replaceChildren(...charts.map((one) => one.element));
    }
    message.hidden = true;
    grid.hidden = false;
    models.forEach((model, at) => charts[at].update(model, view.time));
  }

  return {
    element,
    set(now) {
      const first = service === null;
      service = now;
      const rows = useRows(now.usage);
      setNumbers(rows.length > 0 ? rows : null, (made) => h('div', {}, rowsList(made), h('p', { class: 'muted small' }, USE_FOOT)));
      if (first) load();
    },
  };
}
