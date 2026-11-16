// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The This machine card: the model, the processor, the memory, the firmware
// and how long the machine has run, from `hardware show`. The memory's slots
// and modules are what only SMBIOS's tables say, which an administrator reads
// once the console is unlocked, and a reader is told who sees them.

import { fill, h } from '../../dom.js';
import { button, makeCard } from './cards.js';
import {
  firmwareWords, memoryWords, modelWords, moduleWords, processorWords, tookBackWords, upWords,
} from './model.js';

/** The card, drawn from `ctx` (see page.js). Gives `{element, draw}`. */
export function makeMachine(ctx) {
  const card = makeCard('machine', 'This machine');

  /** The modules, in a list that opens from the memory's line. */
  const modulesOf = (modules) => modules.length > 0 && h('details', { class: 'hw-modules' },
    h('summary', {}, `Show the modules (${modules.length})`),
    h('ul', {}, modules.map((one) => h('li', {}, moduleWords(one)))));

  function memoryRow(machine) {
    const { data } = ctx;
    const memory = data.inventory?.memory ?? machine.memory;
    const words = memoryWords(memory, data.access);
    if (words === null) return null;
    const ask = data.access === 'locked' && button('Show the modules', async () => {
      if (await ctx.readInventory()) card.body.querySelector('summary')?.focus();
    }, 'small');
    return [words, ask && [' ', ask], data.access === 'ok' && Array.isArray(memory?.modules) && modulesOf(memory.modules)];
  }

  function upRow() {
    const { data, view } = ctx;
    const up = upWords(data.about?.up);
    if (up === null) return null;
    const tail = tookBackWords(data.about, view.time, Math.floor(Date.now() / 1000));
    return [tail ? `${up}, ${tail}` : up, tail && [' ', h('a', { href: '/updates#happened' }, 'What happened')]];
  }

  function draw() {
    const { data } = ctx;
    if (data.machine === null) {
      fill(card.body, h('p', {}, 'What this machine is could not be read.'));
      return;
    }
    const firmware = firmwareWords(data.machine.firmware);
    const rows = [
      ['Model', modelWords(data.machine)],
      ['Processor', processorWords(data.machine.processor)],
      ['Memory', memoryRow(data.machine)],
      ['Firmware', firmware && [firmware, ' · ', h('a', { href: '/updates#firmware' }, 'Firmware sets on the Updates page')]],
      ['Up for', upRow()],
    ].filter(([, value]) => value);
    fill(card.body, rows.length > 0
      ? h('dl', { class: 'rows' }, rows.flatMap(([label, value]) => [h('dt', {}, label), h('dd', {}, value)]))
      : h('p', {}, 'The firmware and the kernel say nothing about this machine.'));
  }

  return { element: card.element, errors: card.errors, draw };
}
