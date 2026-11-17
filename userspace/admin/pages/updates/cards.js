// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The frame every card of the Updates page has, and the few pieces they all
// draw: the heading with its state in a pill that a screen reader skips (the
// words say it), the place a refusal goes, and a task's step.

import { fill, h } from '../../dom.js';
import { menu } from '../../shell/menus.js';
import { LATER_CHOICES } from './model.js';

/** A card: `{element, heading, errors, top, body, head(title, pill, extra)}`; `top` is where a line that waits goes, `head` draws the heading, whose title is the card's name and then what it is. */
export function makeCard(id, name) {
  const heading = h('h2', { id: `${id}-h`, tabindex: '-1' }, name);
  const errors = h('div', { class: 'upd-errors' });
  const top = h('div', { class: 'upd-top' });
  const body = h('div', { class: 'upd-body' });
  const element = h('section', { class: 'card', id, 'aria-labelledby': heading.id }, heading, errors, top, body);
  return {
    element,
    heading,
    errors,
    top,
    body,
    head(title, pill = null, extra = null) {
      fill(heading, title ? `${name} · ${title}` : name, pill && h('span', { class: `pill ${pill.kind}`.trim(), 'aria-hidden': 'true' }, pill.word), extra && h('span', { class: 'more' }, extra));
    },
  };
}

/** A paragraph for each of `words`; the first may be made bold. */
export const paragraphs = (words, bold = false) => words.map((one, at) => h('p', {}, bold && at === 0 ? h('b', {}, one) : one));

/** A button: `onclick` runs when it is pressed; `kind` is `default` for the one a card is for. */
export function button(label, onclick, kind = '') {
  const one = h('button', { class: `btn${kind ? ` ${kind}` : ''}`, type: 'button' }, label);
  one.addEventListener('click', () => onclick(one));
  return one;
}

/** "Later ▾": the menu that puts an update's reminder off; `pick(for, button)` is told of the choice. `name` says which update it is for. */
export function laterMenu(name, pick) {
  const made = menu({
    label: `Put off the reminder of ${name}`,
    content: ['Later', h('span', { 'aria-hidden': 'true' }, ' ▾')],
    items: () => LATER_CHOICES.map((one) => ({ label: one.label, onSelect: () => pick(one.for, made.button) })),
  });
  made.button.setAttribute('class', 'btn');
  made.button.setAttribute('aria-label', `Later for ${name}`);
  return made.element;
}

/** The line of an update that is put off, "Put off until Nov 23 at 3:06 AM by admin · Remind me now"; the button is for an administrator. */
export const putLine = (words, remind) => h('div', { class: 'upd-put' }, words, remind && [' · ', button('Remind me now', remind)]);

/**
 * What Later says under an update's words and beside its buttons, for `item`
 * (`system`, `firmware` or an app's ID) called `name`: `{line, menu}`. `later`
 * is `laterOf`'s (model.js); `card` is where a refusal goes. Only an
 * administrator is given the menu and Remind me now.
 */
export function laterParts(ctx, card, item, name, later) {
  const set = async (kind, one) => {
    const words = { unlock: 'put the reminder off', done: kind === 'off' ? 'The reminder is back.' : 'The reminder is put off.' };
    if ((await ctx.run(card, 'updates.later', { item, for: kind }, one, words)).ok) await ctx.reload(card.heading);
  };
  if (!later) return { line: null, menu: null };
  if (later.state === 'put') return { line: putLine(later.words, ctx.admin && ((one) => set('off', one))), menu: null };
  if (later.state === 'ask') return { line: null, menu: ctx.admin ? laterMenu(name, set) : null };
  return { line: h('p', { class: 'muted' }, later.words), menu: null };
}

/** The step a task of this page is at: "Step 2 of 4: fetching, 41%", or null while it has none. */
export function taskLine(task) {
  if (!task) return null;
  if (task.state === 'waiting') return 'Waiting for its turn.';
  const percent = Number.isInteger(task.percent) ? `, ${task.percent}%` : '';
  return task.steps > 0 ? `Step ${task.step} of ${task.steps}: ${task.words || 'working'}${percent}` : `${task.words || 'Working'}${percent}`;
}
