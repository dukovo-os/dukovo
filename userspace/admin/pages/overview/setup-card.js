// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Finish setting up (design 10): "5 of 6 done" with each step the pages
// register and a button to set it up. It is the machine's, so it is for
// administrators, and it is not there at all while no page has a step.

import { h, icon } from '../../dom.js';
import { makeCard, reader } from './card.js';
import { progressOf, readSteps } from './steps-read.js';

/** The card, hidden until it reads that there are steps. */
export function makeSetup(view) {
  const card = makeCard('setup', 'Finish setting up');
  card.element.hidden = true;
  const load = reader(view, card.body, {
    read: () => (view.session.admin ? readSteps(view) : []),
    draw(steps) {
      card.element.hidden = steps.length === 0;
      if (steps.length === 0) return;
      const progress = progressOf(steps);
      card.more.textContent = progress.words;
      card.more.classList.add('muted');
      const meter = h('div', { class: 'meter', role: 'img', 'aria-label': progress.words }, h('i'));
      meter.style.setProperty('--v', String(progress.percent));
      card.body.replaceChildren(meter, h('ul', { class: 'todo' }, ...steps.map((step) => h('li', {},
        step.done ? h('span', { class: 'tick done' }, icon('check'), h('span', { class: 'sr-only' }, 'Done: ')) : h('span', { class: 'tick', 'aria-hidden': 'true' }),
        h('span', {}, step.title),
        !step.done && h('a', { class: 'btn small', href: `/setup?step=${encodeURIComponent(step.id)}`, 'aria-label': `Set up ${step.title}` }, 'Set up')))));
    },
  });
  return { element: card.element, load };
}
