// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "Checks after an update": the services that must run again, the checks that
// take an update back when they fail (required) and those that only send an
// alert (wanted), each with Remove for an administrator, and the dialogs
// that add one, read when first needed.

import { fill, h } from '../../dom.js';
import { button, makeCard } from './cards.js';
import { checksCard } from './settings-model.js';

/** The card. Gives `{element, draw}`. */
export function makeChecks(ctx) {
  const card = makeCard('checks', 'Checks after an update');

  async function remove(action, args, one, words) {
    const result = await ctx.run(card, action, args, one, words);
    if (result.ok) await ctx.reload(card.heading);
  }

  async function add(which) {
    const { openAddCheck, openAddService } = await import('./add-check.js');
    const changed = await (which === 'service' ? openAddService(ctx) : openAddCheck(ctx));
    if (changed) await ctx.reload(card.heading);
  }

  function draw() {
    const { data, admin } = ctx;
    if (!data.checks) {
      card.head(null);
      fill(card.body, h('p', { class: 'muted' }, 'The checks could not be read.'));
      return;
    }
    const say = checksCard(data.checks);
    const row = (line, one) => h('li', { class: 'upd-row' }, h('span', {}, line), admin && one);
    card.head(null);
    fill(card.body,
      h('h3', {}, 'Required'),
      h('p', {}, h('b', {}, 'These services run again'), say.essential.length > 0 ? `: ${say.essential.join(', ')}` : ': none'),
      say.essential.length > 0 && h('ul', { class: 'upd-list' }, ...say.essential.map((name) => row(name, button('Remove', (one) => remove('updates.essential', { service: name, on: false }, one, { unlock: 'remove a service from the checks', done: 'The service is no longer checked.' }))))),
      say.required.length > 0 && h('ul', { class: 'upd-list' }, ...say.required.map((one) => row(one.line, button('Remove', (b) => remove('updates.check-remove', { name: one.name }, b, { unlock: 'remove a check', done: 'The check is removed.' }))))),
      h('p', { class: 'muted' }, 'If one fails, the update is taken back.'),
      h('h3', {}, 'Wanted'),
      say.wanted.length > 0 ? h('ul', { class: 'upd-list' }, ...say.wanted.map((one) => row(one.line, button('Remove', (b) => remove('updates.check-remove', { name: one.name }, b, { unlock: 'remove a check', done: 'The check is removed.' }))))) : h('p', {}, 'None.'),
      h('p', { class: 'muted' }, 'If one fails, you get an alert and the update stays.'),
      say.seen && h('p', { class: 'hint' }, say.seen),
      admin && h('div', { class: 'btns' }, button('Add a check…', () => add('check')), button('Add a service…', () => add('service'))));
  }
  return { element: card.element, draw };
}
