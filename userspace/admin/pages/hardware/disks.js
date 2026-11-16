// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Disks card: each whole disk with its health, its last short and
// extended tests from its own log, and the buttons that run one. The log is
// read for an administrator whose console is unlocked, by the card itself,
// when the page reads and when the console unlocks. The list is made once for
// the disks it shows and only what changes is drawn again, so that a button
// that was pressed keeps its focus while its test runs.

import { fill, h } from '../../dom.js';
import { percentOf } from '../../lib/tasks-words.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { button, makeCard, setOff } from './cards.js';
import {
  DISKS_WORDS, disksOf, LOCKED_TESTS, NO_HEALTH, READER_TESTS, testArgs, testsOf,
} from './disks-model.js';

/** The card, drawn from `ctx` (see page.js). Gives `{element, errors, draw, read}`; `read(ask)` reads the disks' logs and gives whether it did. */
export function makeDisks(ctx) {
  const card = makeCard('disks', 'Disks');
  const { admin, view } = ctx;
  const logs = new Map();
  let access = admin ? 'locked' : 'reader';
  let shown = null;
  let rows = new Map();
  const list = h('ul', { class: 'hw-checks' });
  const why = h('p', { id: 'disks-why', class: 'hint', hidden: true }, 'A check is running. Its step is under the heading.');
  const note = h('p', { class: 'muted' });
  const unlock = button(LOCKED_TESTS, () => read(true), 'small');

  fill(card.body, list, admin && why, admin && h('p', { class: 'small muted' }, DISKS_WORDS), note, admin && unlock);

  /** A disk's row: its name and health, its tests, the buttons that run one. */
  function rowOf(disk) {
    const line = h('p', { class: 'small' });
    const test = (label, full) => {
      const one = button(label, () => ctx.running() === null && ctx.startCheck(testArgs(disk.name, full), one, card.errors));
      one.setAttribute('aria-describedby', 'disks-why');
      return one;
    };
    const buttons = [test('Short test', false), test('Extended test', true)];
    const element = h('li', { class: 'hw-check' },
      h('p', {}, h('a', { href: `/storage/${encodeURIComponent(disk.name)}` }, h('b', {}, disk.name)),
        disk.model && h('span', { class: 'muted' }, ` · ${disk.model} `), ' ', h('span', { class: `pill ${disk.pill.tone}` }, disk.pill.word)),
      line, admin && h('div', { class: 'btns' }, buttons));
    return { element, line, buttons };
  }

  function draw() {
    const disks = disksOf(ctx.data.storage);
    const key = JSON.stringify(disks);
    if (key !== shown) {
      shown = key;
      rows = new Map((disks ?? []).map((disk) => [disk.name, rowOf(disk)]));
      fill(list, [...rows.values()].map((row) => row.element));
    }
    const off = ctx.running() !== null;
    why.hidden = !off;
    for (const disk of disks ?? []) {
      const row = rows.get(disk.name);
      const task = ctx.running(disk.name);
      for (const one of row.buttons) setOff(one, off);
      if (task) {
        const percent = percentOf(task);
        fill(row.line, percent === null ? 'Testing…' : `Testing: ${percent}%`);
      } else if (!disk.hasHealth) {
        fill(row.line, h('span', { class: 'muted' }, NO_HEALTH));
      } else {
        const tests = logs.has(disk.name) ? testsOf(logs.get(disk.name), view.time, Math.floor(Date.now() / 1000)) : [];
        fill(row.line, tests.flatMap((one, at) => [at > 0 && ' · ', one.tone ? h('span', { class: one.tone }, one.words) : one.words]));
      }
    }
    fill(note, disks === null ? 'The disks could not be read.' : disks.length === 0 && 'No disk was found.', !admin && READER_TESTS);
    unlock.hidden = access !== 'locked';
  }

  async function read(ask) {
    if (!admin || !(isUnlocked() || (ask && await askToUnlock(view.session, 'read the disks\' own tests')))) return false;
    const names = (disksOf(ctx.data.storage) ?? []).filter((disk) => disk.hasHealth).map((disk) => disk.name);
    const found = await Promise.allSettled(names.map((name) => view.api.get(`/api/1/storage/disks/${encodeURIComponent(name)}/health`)));
    if (view.signal.aborted) return false;
    logs.clear();
    names.forEach((name, at) => logs.set(name, found[at].status === 'fulfilled' ? found[at].value : null));
    access = 'ok';
    draw();
    return true;
  }

  return {
    element: card.element, errors: card.errors, draw, read,
  };
}
