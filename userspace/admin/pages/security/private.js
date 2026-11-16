// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Private access (design 8.8 and 9.3): "Reach this console from anywhere,
// privately", with WireGuard. Not set up, it says what it does and sets it up;
// set up, it lists the devices with when each last connected, adds a device
// (its file, with its private key, is shown once as a QR and as text) and
// removes one. Removing the device this browser comes through is refused by
// admind (ADM-137), and the card shows its words. What only an administrator
// may read is shown when the console is unlocked, and read again every 10
// seconds while the card is shown and after each change.

import { h } from '../../dom.js';
import { adminReader, makeCard, unlocked } from './card.js';
import {
  deviceRows, interfaceOf, interfaceWords, WHAT,
} from './private-model.js';

/** How often the devices are read again while the card is shown. */
const READ_MS = 10_000;

/** The card. Gives `{element, load}`. */
export function makePrivate(view) {
  const card = makeCard('private', 'Reach this console from anywhere, privately');
  let reader = null;

  /** The dialogs are read when the first is asked for. */
  const dialogs = () => import('./private-dialogs.js');

  async function openSetUp() {
    if (!(await unlocked(view, 'set up private access'))) return;
    await (await dialogs()).openSetUp(view, reader);
  }

  async function openAdd(taken) {
    if (!(await unlocked(view, 'add a device'))) return;
    await (await dialogs()).openAdd(view, reader, taken);
  }

  async function remove(name, button) {
    await (await dialogs()).removeDevice(view, reader, { name, button, errors: card.errors });
  }

  function draw(answer) {
    const iface = interfaceOf(answer);
    if (iface === null) {
      return [h('p', {}, WHAT), h('div', { class: 'btns' }, h('button', { class: 'btn', type: 'button', 'data-action': 'private-set-up', onclick: openSetUp }, 'Set up private access…'))];
    }
    const rows = deviceRows(iface, view.time);
    const taken = rows.map((row) => row.name);
    return [
      h('p', { class: 'small muted mono' }, interfaceWords(iface)),
      rows.length === 0
        ? h('p', { class: 'muted' }, 'No device yet. Add one to reach this console privately.')
        : h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack' },
          h('caption', { class: 'sr-only' }, 'Devices that reach this server privately'),
          h('thead', {}, h('tr', {}, ...['Device', 'Last connected', 'Traffic'].map((title) => h('th', { scope: 'col' }, title)), h('th', { scope: 'col' }, h('span', { class: 'sr-only' }, 'Action')))),
          h('tbody', {}, ...rows.map((row) => h('tr', {},
            h('td', { 'data-label': 'Device' }, h('b', {}, row.name), h('small', { class: 'mono' }, row.addresses)),
            h('td', { 'data-label': 'Last connected' }, row.connected),
            h('td', { 'data-label': 'Traffic' }, row.transfer),
            h('td', { class: 'end' }, h('button', { class: 'btn small', type: 'button', 'aria-label': `Remove ${row.name}`, onclick: (event) => remove(row.name, event.currentTarget) }, 'Remove'), view.api.inline('private.remove', { name: row.name }))))))),
      h('div', { class: 'btns' }, h('button', { class: 'btn', type: 'button', 'data-action': 'private-add', onclick: () => openAdd(taken) }, 'Add a device…')),
    ];
  }

  reader = adminReader(view, card, {
    action: 'private.show',
    every: READ_MS,
    words: {
      unlock: 'see private access',
      reader: 'Administrators see private access.',
      locked: 'Unlock to see private access.',
      failed: 'Private access could not be read.',
      missing: 'This server sets up private access with a later update of Dukovo.',
    },
    draw,
  });
  return { element: card.element, load: reader.load };
}
