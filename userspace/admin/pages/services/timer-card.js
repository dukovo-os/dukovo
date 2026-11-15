// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Timer card of the page of one service (design 4.4): a service that a
// timer starts says so, links to the Scheduled tasks and, to an unlocked
// administrator, says when it runs. The schedule is read once when the card
// first shows and again when the console is unlocked or locked, never with
// the page's own reads, which come every few seconds (design 3.4).

import { fill, h } from '../../dom.js';
import { makeReads } from '../overview/admin-reads.js';
import { rowsList } from './cards.js';
import { timerCardRows, timerRows } from './timer-model.js';

/** The card of the timer of the service `name`: `{element, set(service)}`. `set` shows the card when the service has a timer. */
export function makeTimer(view, name) {
  const reads = makeReads(view);
  const body = h('div', { class: 'svc-body' });
  const element = h('section', { class: 'card', id: 'timer', 'aria-labelledby': 'timer-h', hidden: true }, h('h2', { id: 'timer-h' }, 'Timer'), body);
  let on = false;
  let latest = 0;

  /** Reads the timers and draws this service's. Only the last read counts. */
  async function load() {
    latest += 1;
    const mine = latest;
    const result = await reads.read('/api/1/timers', { admin: true });
    if (view.signal.aborted || mine !== latest) return;
    const own = result.state === 'ok' && Array.isArray(result.value) ? result.value.filter((timer) => timer.service === name) : [];
    const [row] = timerRows(own, { time: view.time, nowMs: Date.now() });
    fill(body,
      h('p', {}, 'Spark starts this service at the times its file names.'),
      result.state !== 'ok' && reads.note(result, 'its schedule', load),
      result.state === 'ok' && (row ? rowsList(timerCardRows(row)) : h('p', { class: 'muted' }, 'No schedule is listed for it.')),
      h('div', { class: 'btns' }, h('a', { class: 'btn small', href: '/services/scheduled' }, 'Open Scheduled tasks')));
  }

  view.on('unlock', () => {
    reads.forget();
    if (on) load();
  });

  return {
    element,
    set(service) {
      const wanted = service.timer === true;
      element.hidden = !wanted;
      if (wanted && !on) load();
      on = wanted;
    },
  };
}
