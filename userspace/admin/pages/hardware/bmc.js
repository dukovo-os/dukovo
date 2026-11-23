// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The BMC card: who the baseboard management controller is and its network
// address, what is not well among its power, current and voltage sensors with
// the rest under a list that opens, the last ten entries of its event log, and
// the button that lights the identify light. Only an administrator whose
// console is unlocked reads the BMC, and only a machine with one has the card:
// the page puts it in when `hardware show` as an administrator names a BMC.

import { fill, h } from '../../dom.js';
import { button, makeCard } from './cards.js';
import {
  bmcRows, eventRows, IDENTIFY_WORDS, NO_EVENTS, NO_SENSORS, sensorGroups, showAllWords,
} from './bmc-model.js';

const IDENTIFY = 'hardware.identify';

/** A line of a sensor: its name and its words, the words in a state's colour when it is not well. */
const lineOf = (item) => h('li', {}, h('b', {}, item.name), ' · ', item.tone ? h('span', { class: `st ${item.tone}` }, item.words) : item.words);

/** The card, drawn from `ctx` (see page.js). Gives `{element, errors, draw}`; `draw` reads `ctx.data.inventory.bmc`, `ctx.data.sensors` and `ctx.data.events`. */
export function makeBmc(ctx) {
  const card = makeCard('bmc', 'Baseboard management controller');
  const { view } = ctx;
  const identify = button('Identify', (one) => view.api.run(IDENTIFY, {}, {
    button: one,
    where: { slot: card.errors },
    words: {
      unlock: 'light the identify light', running: 'Lighting the identify light', busy: 'Lighting the light…', done: 'The identify light is on for 15 seconds.',
    },
  }));

  function sensorsPart(answer) {
    const { total, attention, groups } = sensorGroups(answer);
    if (total === 0) return h('p', { class: 'muted' }, NO_SENSORS);
    return [
      attention.length > 0
        ? h('ul', { class: 'hw-checks' }, attention.map((item) => lineOf({ ...item, name: `${item.kind}: ${item.name}` })))
        : h('p', { class: 'small muted' }, `All ${total} power, current and voltage sensors read normally.`),
      h('details', { class: 'hw-modules' },
        h('summary', {}, showAllWords(total)),
        groups.map((group) => [h('h3', {}, group.title), h('ul', {}, group.items.map(lineOf))])),
    ];
  }

  function eventsPart(answer) {
    if (answer === undefined) return h('p', { class: 'muted' }, 'The event log could not be read.');
    const rows = eventRows(answer, view.time);
    if (rows.length === 0) return h('p', { class: 'muted' }, NO_EVENTS);
    return h('table', { class: 'tbl stack' },
      h('caption', {}, 'The last entries of the event log, newest first'),
      h('tbody', {}, rows.map((row) => h('tr', {},
        h('td', { 'data-label': 'When' }, row.when),
        h('td', { 'data-label': 'What' }, row.tone ? h('span', { class: `st ${row.tone}` }, row.words) : row.words)))));
  }

  function draw() {
    const { data } = ctx;
    const rows = bmcRows(data.inventory?.bmc);
    fill(card.body,
      h('dl', { class: 'rows' }, rows.flatMap(([label, words]) => [h('dt', {}, label), h('dd', {}, words)])),
      sensorsPart(data.sensors),
      h('h3', {}, 'Event log'),
      eventsPart(data.events),
      h('p', { class: 'small muted' }, IDENTIFY_WORDS),
      h('div', { class: 'btns' }, identify));
  }

  return { element: card.element, errors: card.errors, draw };
}
