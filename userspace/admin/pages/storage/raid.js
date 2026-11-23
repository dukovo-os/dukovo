// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The RAID cards of the Storage page, read side only: the controllers, the
// bays drawn as the front of the server, the RAID volumes with their task
// bars and the physical disks with the health the controller reports. Nothing
// here changes the machine. The page loads this file with the first answer
// that lists a controller, so a machine with none never reads it; the words
// are raid-model.js's and this file only draws them. A card is drawn again
// only when what it says changes.

import { fill, h, uniqueId } from '../../dom.js';
import { bayGroups, controllerRows, controllersOf, diskTables, volumeCards } from './raid-model.js';

/** The legend under the bays: each state, drawn as its bay is. */
const LEGEND = Object.freeze([['use', 'In use'], ['rebuild', 'Rebuilding'], ['fail', 'Failed'], ['spare', 'Hot spare'], ['ready', 'Ready'], ['empty', 'Empty']]);

/** The heading of a controller's part of a card, when the machine has more than one. */
const partTitle = (many, title) => many && h('h3', { class: 'sto-raid-sub' }, title);

/** A row of the controller's card: the term, and its words, with the state in a mark when it has a tone. */
function rowOf(row) {
  return [
    h('dt', {}, row.label),
    h('dd', {},
      row.pill ? h('span', { class: `st ${row.tone}` }, row.value) : row.value,
      row.note && h('small', { class: 'sto-note' }, row.note)),
  ];
}

/** The Controllers card's body. */
function controllerPanels(list) {
  return list.map((controller) => h('div', { class: 'sto-ctl', 'data-controller': String(controller.n) },
    h('div', { class: 'sto-ctl-head' }, h('h3', {}, controller.title), h('span', { class: `st ${controller.state.tone}` }, controller.state.word)),
    h('dl', { class: 'rows' }, controller.rows.flatMap(rowOf))));
}

/** One bay: its name and state in words, and a drawing of its lights that a screen reader skips. */
function bayOf(bay) {
  const leds = { use: ['ok', ''], rebuild: ['ok', 'warn'], fail: ['', 'bad'], spare: ['ok', ''], ready: ['ok', ''], direct: ['ok', ''], off: ['', ''], empty: ['', ''] }[bay.kind] ?? ['', ''];
  return h('li', { class: `bay ${bay.kind}`, 'data-bay': bay.name },
    h('span', { class: 'ledrow', 'aria-hidden': 'true' }, h('span', { class: `led ${leds[0]}`.trim() }), h('span', { class: `led ${leds[1]}`.trim() })),
    h('b', {}, bay.name),
    h('span', { class: 'word' }, bay.word),
    bay.sub && h('span', { class: 'sub' }, bay.sub),
    bay.size && h('span', { class: 'sub' }, bay.size),
    h('span', { class: 'grip', 'aria-hidden': 'true' }));
}

/** The row of bays of one backplane; its width is the number of bays, set as a property so that the page's content policy allows it. */
function baysRow(group) {
  const row = h('ul', { class: 'sto-bays', 'aria-label': `Drive bays${group.title ? `, ${group.title.toLowerCase()}` : ''}, front of the server` }, group.bays.map(bayOf));
  row.style.setProperty('--bays', String(group.bays.length));
  return row;
}

/** The Bays card's body: the front of the server, a row of bays for each backplane, and what the colours are. */
function baysOf(list) {
  const many = list.length > 1;
  return [
    ...list.map((controller) => h('div', { class: 'sto-front', 'data-controller': String(controller.n) },
      partTitle(many, controller.title),
      controller.groups.map((group) => h('div', { class: 'sto-chassis' },
        group.title && h('h4', { class: 'sto-raid-sub' }, group.title),
        baysRow(group))))),
    h('ul', { class: 'sto-legend', 'aria-label': 'What the bays show' }, LEGEND.map(([kind, words]) => h('li', {}, h('span', { class: `swatch ${kind}`, 'aria-hidden': 'true' }), words))),
  ];
}

/** The task bar of a volume: its words, the bar and the time left. */
function taskOf(task) {
  const meter = h('i');
  meter.style.setProperty('--v', String(task.percent ?? 0));
  const id = uniqueId('sto-task');
  return h('div', { class: 'sto-task' },
    h('p', { class: 'small', id }, task.line),
    h('div', {
      class: `meter${task.percent === null ? ' wait' : ''}`, role: 'progressbar', 'aria-labelledby': id, 'aria-valuemin': '0', 'aria-valuemax': '100', ...(task.percent === null ? {} : { 'aria-valuenow': String(task.percent) }),
    }, meter));
}

/** The stack line: "volume data → sda → RAID 1 → Bays 0 and 1". */
function stackOf(stack) {
  return h('p', { class: 'stack sto-stack' },
    stack.flatMap((part, at) => [at > 0 && h('span', { class: 'to', 'aria-hidden': 'true' }, '→'), at > 0 && h('span', { class: 'sr-only' }, ', then '), h('span', {}, part)]));
}

/** One volume. */
function volumeOf(volume) {
  return h('div', { class: 'sto-volume panel', 'data-volume': volume.name },
    h('div', { class: 'sto-ctl-head' }, h('h3', {}, volume.name), h('span', { class: 'small muted' }, volume.head), h('span', { class: `st ${volume.state.tone}` }, volume.state.word)),
    volume.task && taskOf(volume.task),
    volume.note && h('p', { class: 'small muted' }, volume.note),
    h('dl', { class: 'rows' },
      h('dt', {}, 'Bays'), h('dd', {}, volume.bays),
      h('dt', {}, 'Stripe and cache'), h('dd', {}, volume.cache),
      h('dt', {}, 'Sits under'), h('dd', {}, stackOf(volume.stack))));
}

/** The RAID volumes card's body. */
function volumesOf(list) {
  const many = list.length > 1;
  return list.map((controller) => h('div', { class: 'sto-vols', 'data-controller': String(controller.n) },
    partTitle(many, controller.title),
    controller.rows.length === 0 ? h('p', { class: 'muted' }, 'This controller has no volumes.') : controller.rows.map(volumeOf)));
}

const COLUMNS = ['Bay', 'Disk', 'Kind', 'State', 'Health'];

/** The table of physical disks. */
function tablesOf(list) {
  const many = list.length > 1;
  return list.map((controller) => {
    const id = uniqueId('sto-disks-cap');
    return h('div', { class: 'sto-pd', 'data-controller': String(controller.n) },
      partTitle(many, controller.title),
      controller.rows.length === 0 ? h('p', { class: 'muted' }, 'This controller has no disks.') : h('div', { class: 'tbl-box', role: 'region', tabindex: '0', 'aria-labelledby': id },
        h('table', { class: 'tbl stack sto-pd-table' },
          h('caption', { id, class: 'sr-only' }, `Physical disks of ${controller.title}, by bay, with the health the controller reports`),
          h('thead', {}, h('tr', {}, COLUMNS.map((name) => h('th', { scope: 'col' }, name)))),
          h('tbody', {}, controller.rows.map((row) => h('tr', { 'data-disk': row.bay },
            h('th', { scope: 'row', 'data-label': COLUMNS[0] }, row.bay),
            h('td', { 'data-label': COLUMNS[1] }, row.disk, row.details && h('small', {}, row.details)),
            h('td', { 'data-label': COLUMNS[2] }, row.kind),
            h('td', { 'data-label': COLUMNS[3] }, h('span', { class: `st ${row.state.tone}` }, row.state.word), row.state.note && h('small', {}, row.state.note)),
            h('td', { 'data-label': COLUMNS[4] }, h('span', { class: `st ${row.health.tone}` }, row.health.word), row.health.line && h('small', {}, row.health.line))))))));
  });
}

/** A card with a heading, as the page's other cards are. */
function card(id, title, intro = null) {
  const heading = h('h2', { id: `${id}-h` }, title);
  const body = h('div', { class: 'sto-raid-body' });
  const element = h('section', { class: 'card sto-raid-card', id, 'aria-labelledby': heading.id }, heading, intro && h('p', { class: 'small muted' }, intro), body);
  return { element, heading, body };
}

/**
 * The RAID cards. `set(raid, {storage, time})` draws the answer of `storage.raid`
 * (`storage` is `storage show`'s, for the volume that sits on a RAID volume's disk,
 * `time` lib/time.js's); with no controller nothing is drawn and the element is
 * empty. Gives `{element, set}`.
 */
export function makeRaid() {
  const parts = {
    controllers: card('controllers', 'Controllers'),
    bays: card('bays', 'Bays'),
    volumes: card('volumes', 'RAID volumes'),
    disks: card('physical', 'Physical disks', 'Health read through the controller'),
  };
  const element = h('div', { class: 'sto-raid' });
  const drawn = new Map();
  let drawnAny = false;

  /** Fills a card's body when what it says has changed. */
  function paint(name, value, draw) {
    const key = JSON.stringify(value);
    if (drawn.get(name) === key) return;
    drawn.set(name, key);
    fill(parts[name].body, draw(value));
  }

  return {
    element,
    set(raid, { storage = null, time }) {
      if (controllersOf(raid).length === 0) {
        if (drawnAny) fill(element);
        drawnAny = false;
        drawn.clear();
        return;
      }
      if (!drawnAny) {
        fill(element, h('div', { class: 'sto-raid-top' }, parts.controllers.element), parts.bays.element, parts.volumes.element, parts.disks.element);
        drawnAny = true;
      }
      paint('controllers', controllerRows(raid), controllerPanels);
      paint('bays', controllersOf(raid).map(bayGroups), baysOf);
      paint('volumes', volumeCards(raid, { storage, duration: time.duration }), volumesOf);
      paint('disks', diskTables(raid, time), tablesOf);
    },
  };
}
