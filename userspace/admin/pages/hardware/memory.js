// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Memory test card: why to test, the test that runs while the system
// does (the memory area of a quick check, which an administrator starts
// here), the test of all of it at the next restart, the fuller test from the
// start menu, which needs someone at the machine, and when the memory was
// last tested, by the check and at a start. The card is made once and only
// its words and its buttons' states are drawn again, so that a button that
// was pressed keeps its focus; the buttons of the test at the next restart
// are swapped only when it is asked for or cancelled.

import { fill, h } from '../../dom.js';
import { button, makeCard, setOff } from './cards.js';
import { nextWords } from './memtest-model.js';
import {
  MEMORY_RESTART_WHY, MEMORY_TEST_ARGS, MEMORY_WORDS, memoryRunWords, READER_TEST,
} from './model.js';

/** The card, drawn from `ctx` (see page.js). Gives `{element, errors, draw}`. */
export function makeMemory(ctx) {
  const card = makeCard('memory', 'Memory test');
  const { admin, view } = ctx;
  const last = h('p', {});
  const start = h('p', { hidden: true });
  const report = h('div', { class: 'btns', hidden: true }, h('a', { class: 'btn', href: '/report?about=hardware' }, 'Report a problem'));
  const why = h('p', { id: 'memory-why', class: 'hint', hidden: true }, 'A check is running. Its step is under the heading.');
  const test = button('Test part of the memory', (one) => ctx.running() === null && ctx.startCheck(MEMORY_TEST_ARGS, one, card.errors));
  test.setAttribute('aria-describedby', 'memory-why');

  const next = h('p', {});
  const askedLine = h('p', { hidden: true });
  const askButton = button('Test at the next restart…', async (one) => {
    fill(card.errors);
    if (await ctx.memtest('askMemtest', one, card.errors)) dontTest.focus();
  });
  const dontTest = button('Don\'t test', async (one) => {
    fill(card.errors);
    if (await ctx.memtest('cancelMemtest', one, card.errors)) askButton.focus();
  });
  const restartToTest = button('Restart now…', () => ctx.restartToTest());
  const asking = h('div', { class: 'btns' });
  // The menu's own restart is for someone who goes to the machine; once the test is asked, the one beside Don't test is the one.
  const menuRestart = h('div', { class: 'btns hw-menu' }, button('Restart now…', () => ctx.power('restart', MEMORY_RESTART_WHY)));
  let asked = null;

  fill(card.body,
    h('p', {}, MEMORY_WORDS.why),
    h('div', { class: 'hw-action' },
      h('p', {}, h('b', {}, 'While the system runs. '), MEMORY_WORDS.running),
      admin && h('div', { class: 'btns' }, test),
      admin && view.api.inline('hardware.check', MEMORY_TEST_ARGS),
      admin && why),
    h('div', { class: 'hw-action' }, next, askedLine, admin && asking),
    h('div', { class: 'hw-action' },
      h('p', {}, h('b', {}, 'For the fullest test, from the start menu. '), MEMORY_WORDS.menu),
      admin && menuRestart),
    !admin && h('p', { class: 'muted' }, READER_TEST),
    last,
    start,
    report,
    h('p', { class: 'muted' }, MEMORY_WORDS.nothing));

  function draw() {
    const { data } = ctx;
    const off = ctx.running() !== null;
    setOff(test, off);
    why.hidden = !off;
    fill(last, h('b', {}, 'Last run: '), data.last === undefined ? 'could not be read' : memoryRunWords(data.last, view.time));

    fill(next, h('b', {}, 'All of it, at the next restart. '), nextWords(data.machine?.memory?.bytes));
    // The lines of a test that is asked for or was run come from a file of their own, which most machines need no more than the first time.
    import('./memtest-lines.js').then(({ drawLines }) => drawLines(ctx, { asked: askedLine, start, report }), (error) => view.problem(error, { slot: card.errors }));
    const isAsked = Boolean(data.memtest?.asked);
    menuRestart.hidden = isAsked;
    if (isAsked !== asked) {
      asked = isAsked;
      fill(asking, isAsked ? [dontTest, restartToTest] : askButton);
    }
  }

  return { element: card.element, errors: card.errors, draw };
}
