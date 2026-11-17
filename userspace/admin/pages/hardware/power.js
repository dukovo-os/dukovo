// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Restart or shut down card: the restart or shut down that waits, with
// Cancel (a line the page makes when one is set, lib/pending-restart.js),
// Restart and Shut down, which ask first, and a restart at a time of day.
// A reader is told who may.

import { fill, h, uniqueId } from '../../dom.js';
import { button, makeCard, setOff } from './cards.js';
import { POWER_WORDS, READER_POWER } from './model.js';

/** The card, drawn from `ctx` (see page.js). Gives `{element, errors, host, draw}`; `host` is where the pending line goes. */
export function makePower(ctx) {
  const host = h('div', { class: 'hw-pending' });
  const card = makeCard('power', 'Restart or shut down', host);
  const inputId = uniqueId('hw-at');
  const time = h('input', { class: 'input', type: 'time', id: inputId, name: 'at', step: '60' });
  const inline = h('div', {});

  /** The command under the time, for the time as it is typed now; none until one is. */
  const drawInline = () => fill(inline, time.value !== '' && ctx.view.api.inline('power.restart', { at: time.value }));
  time.addEventListener('input', drawInline);
  time.addEventListener('change', drawInline);

  /** Sets the restart at the time typed; one press at a time, and the button keeps its focus. */
  let setting = false;
  async function restartAt(one) {
    if (setting) return;
    setting = true;
    setOff(one, true);
    try {
      await ctx.power('restartAt', time.value);
    } finally {
      setting = false;
      setOff(one, false);
    }
  }

  function draw() {
    if (!ctx.admin) {
      fill(card.body, h('p', {}, POWER_WORDS), h('p', { class: 'muted' }, READER_POWER));
      return;
    }
    fill(card.body,
      h('p', {}, POWER_WORDS),
      h('div', { class: 'btns' },
        button('Restart…', () => ctx.power('restart', '')),
        button('Shut down…', () => ctx.power('shutDown'), 'danger')),
      h('div', { class: 'hw-at' },
        h('div', { class: 'field' }, h('label', { for: inputId }, 'Restart at'), time),
        button('Restart at that time', restartAt)),
      inline);
  }

  return {
    element: card.element, errors: card.errors, host, draw,
  };
}
