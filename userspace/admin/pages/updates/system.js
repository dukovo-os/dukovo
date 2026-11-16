// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The System and Firmware cards: what the last check says, by the state the
// offer is in, and the buttons of each state for an administrator: stage, hold
// and release, restart now or at a time, take a staged update back, and go
// back to the version before. A restart is the page's (page.js), and a going
// back is back.js, read when first needed.

import { fill, h } from '../../dom.js';
import { button, makeCard, paragraphs, taskLine } from './cards.js';
import {
  clockWords, firmwareCard, keptWords, MAKER_WORDS, systemCard, withdrawnWords,
} from './model.js';

const STAGE = { unlock: 'stage the update', running: 'Staging the update', done: 'The update is staged. It starts at the next restart.', busy: 'Staging…' };

/** The System card, drawn from `ctx` (see page.js). Gives `{element, draw}`. */
export function makeSystem(ctx) {
  const { view } = ctx;
  const card = makeCard('system', 'System');
  ctx.pendingHost = card.top;

  /** Runs `action` for the system and reads again. */
  async function run(action, args, one, words) {
    const result = await ctx.run(card, action, args, one, words);
    if (result.ok && !result.task) await ctx.reload(card.heading);
    return result;
  }

  async function goBack() {
    const { openGoBack } = await import('./back.js');
    await openGoBack(ctx);
  }

  function draw() {
    const { data, admin } = ctx;
    const say = systemCard({
      check: data.check, slots: data.slots, settings: data.settings, failed: data.failed?.failed, running: data.running, time: view.time,
    });
    const withdrawn = withdrawnWords(data.check);
    const kept = keptWords(data.slots);
    const time = data.settings?.restart;
    const at = h('input', { id: 'restart-time', class: 'input', type: 'time', value: time ?? '03:00' });
    const buttons = {
      stage: () => button('Stage it', (one) => run('updates.apply', { item: 'system' }, one, STAGE).then((result) => result.task && ctx.begin(result.task, 'updates.apply', { item: 'system' })), 'default'),
      hold: () => button('Hold this version', (one) => run('updates.hold', { item: 'system' }, one, { unlock: 'hold the update', done: 'The update is held.' })),
      release: () => button('Release the hold', (one) => run('updates.unhold', { item: 'system' }, one, { unlock: 'release the hold', done: 'The hold is released.' }), 'default'),
      unstage: () => button('Take it back', (one) => run('updates.unstage', {}, one, { unlock: 'take the update back', done: 'The staged update is taken back.' })),
      restart: () => button('Restart now…', () => ctx.restart(), 'default'),
      happened: () => button('What happened', () => ctx.show('happened')),
      goback: () => button('Go back…', goBack),
    };
    const shown = admin ? [...say.buttons, ...(withdrawn?.buttons ?? [])].map((id) => buttons[id]()) : [];
    const picker = admin && say.state === 'staged' && !time
      ? h('div', { class: 'field' }, h('label', { for: at.id }, 'Restart at'), h('div', { class: 'btns' }, at, button('Restart at that time', () => ctx.restartAt(at.value))))
      : null;
    if (admin && say.state === 'staged' && time) shown.splice(1, 0, button(`Restart at ${clockWords(time)}`, () => ctx.restartAt(time)));
    card.head(say.version, say.pill);
    fill(card.body,
      paragraphs([say.title, ...say.lines], true),
      withdrawn && paragraphs(withdrawn.lines),
      taskLine(ctx.task('system')) && h('p', { class: 'muted' }, taskLine(ctx.task('system'))),
      shown.length > 0 && h('div', { class: 'btns' }, ...shown),
      picker,
      h('div', { id: 'go-back', class: 'upd-kept' }, kept
        ? [h('p', {}, kept.text), admin && h('div', { class: 'btns' }, button('Go back to it…', goBack))]
        : (data.slotsState === 'locked'
          ? [h('p', { class: 'muted' }, 'Unlock to see the version kept for going back.'), h('div', { class: 'btns' }, button('Show the slots', () => ctx.readSlots()))]
          : h('p', { class: 'muted' }, admin ? 'No version is kept for going back.' : 'An administrator can go back to the version before.'))));
  }
  return { element: card.element, draw };
}

/** The Firmware card. Gives `{element, draw}`. */
export function makeFirmware(ctx) {
  const card = makeCard('firmware', 'Firmware');

  function draw() {
    const { data, admin } = ctx;
    const say = firmwareCard({ check: data.check, slots: data.slots, running: data.running });
    const stage = (one) => !say.reason && ctx.run(card, 'updates.apply', { item: 'firmware' }, one, { unlock: 'stage the firmware set', running: 'Staging the firmware set', done: 'The firmware set is staged.' })
      .then((result) => (result.task ? ctx.begin(result.task, 'updates.apply', { item: 'firmware' }) : result.ok && ctx.reload(card.heading)));
    const release = (one) => ctx.run(card, 'updates.unhold', { item: 'firmware' }, one, { unlock: 'release the hold', done: 'The hold is released.' }).then((result) => result.ok && ctx.reload(card.heading));
    const stageButton = say.buttons.includes('stage') && admin ? button('Stage it', stage, 'default') : null;
    if (stageButton && say.reason) {
      stageButton.setAttribute('aria-disabled', 'true');
      stageButton.setAttribute('aria-describedby', 'firmware-why');
    }
    card.head(null, say.pill);
    fill(card.body,
      h('p', {}, h('b', {}, say.title)),
      taskLine(ctx.task('firmware')) && h('p', { class: 'muted' }, taskLine(ctx.task('firmware'))),
      (stageButton || (say.buttons.includes('release') && admin)) && h('div', { class: 'btns' }, stageButton, say.buttons.includes('release') && admin && button('Release the hold', release)),
      say.reason && h('p', { id: 'firmware-why', class: 'hint' }, say.reason),
      h('p', { class: 'muted' }, MAKER_WORDS));
  }
  return { element: card.element, draw };
}
