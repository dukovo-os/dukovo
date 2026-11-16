// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Memory test card: why to test, the test that runs while the system
// does (the memory area of a quick check, which an administrator starts
// here), the test of all of it from the start menu, which needs the machine
// to restart, and when the memory was last tested. The card is made once and
// only its last run and its button's state are drawn again, so that a button
// that was pressed keeps its focus.

import { fill, h } from '../../dom.js';
import { button, makeCard, setOff } from './cards.js';
import {
  MEMORY_RESTART_WHY, MEMORY_TEST_ARGS, MEMORY_WORDS, memoryRunWords, READER_TEST,
} from './model.js';

/** The card, drawn from `ctx` (see page.js). Gives `{element, errors, draw}`. */
export function makeMemory(ctx) {
  const card = makeCard('memory', 'Memory test');
  const { admin, view } = ctx;
  const last = h('p', {});
  const why = h('p', { id: 'memory-why', class: 'hint', hidden: true }, 'A check is running. Its step is under the heading.');
  const test = button('Test part of the memory', (one) => ctx.running() === null && ctx.startCheck(MEMORY_TEST_ARGS, one, card.errors));
  test.setAttribute('aria-describedby', 'memory-why');

  fill(card.body,
    h('p', {}, MEMORY_WORDS.why),
    h('div', { class: 'hw-action' },
      h('p', {}, h('b', {}, 'While the system runs. '), MEMORY_WORDS.running),
      admin && h('div', { class: 'btns' }, test),
      admin && view.api.inline('hardware.check', MEMORY_TEST_ARGS),
      admin && why),
    h('div', { class: 'hw-action' },
      h('p', {}, h('b', {}, 'All of it, from the start menu. '), MEMORY_WORDS.menu),
      // The choice to test at the next restart comes in this row, beside Restart now.
      admin && h('div', { class: 'btns hw-menu' }, button('Restart now…', () => ctx.power('restart', MEMORY_RESTART_WHY)))),
    !admin && h('p', { class: 'muted' }, READER_TEST),
    last,
    h('p', { class: 'muted' }, MEMORY_WORDS.nothing));

  function draw() {
    const off = ctx.running() !== null;
    setOff(test, off);
    why.hidden = !off;
    fill(last, h('b', {}, 'Last run: '), ctx.data.last === undefined ? 'could not be read' : memoryRunWords(ctx.data.last, view.time));
  }

  return { element: card.element, errors: card.errors, draw };
}
