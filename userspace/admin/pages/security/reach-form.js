// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The forms that change who may reach the console (design 8.7 and 9.2): making
// it public, which asks for care and offers the second step first, and making
// it answer this network only, which says so when the browser comes from
// outside and would be cut off. Each is only tried when it can take this
// browser away. Read when a form is first opened.

import { fill, h, icon } from '../../dom.js';
import { windowsFor } from '../../lib/trial-model.js';
import { openForm } from './dialog.js';
import { switchField } from './fields.js';
import { networksOf, reachCutsOff } from './model.js';
import { cutsWords, publicWords } from './rules-form-model.js';
import { rulesOf } from './rules-model.js';

/**
 * Opens the form that changes the reach to `reach`, `any` or `private`, from
 * the settings `answer`. A try begins the trial of `banner`; `reread()` reads
 * the settings again. Resolves when the dialog closes.
 */
export async function openReachForm(view, {
  reach, answer, banner, reread,
}) {
  const found = reach === 'private' ? await view.api.get('/api/1/network').catch(() => null) : null;
  const rules = rulesOf(answer);
  const cuts = reach === 'private' && reachCutsOff(view.session.from, networksOf(found));
  const { choices, chosen } = windowsFor(cuts, location.hostname);
  const select = h('select', { class: 'select', id: 'sec-reach-window' }, ...choices.map((one) => h('option', { value: String(one.seconds), selected: one.seconds === chosen }, one.label)));
  const seconds = () => Number(select.value);
  const ends = () => view.time.clock(Math.floor(Date.now() / 1000) + seconds());
  const second = reach === 'any' && rules.second_step === 'if-set'
    ? switchField({ label: 'Ask administrators for a second step first', hint: 'Sets the rule to administrators, and whoever has added one, before the console is made public.', on: true })
    : null;
  const warning = h('div', { class: 'banner warn', role: 'status', hidden: !cuts });
  const drawWarning = () => fill(warning, cuts && [icon('warn'), h('div', {}, h('p', {}, cutsWords(view.session.from, ends())))]);
  select.addEventListener('change', drawWarning);
  drawWarning();

  const run = async (button, slot, trial) => {
    if (second?.input.checked) {
      const set = await view.api.run('security.sign-in-set', { second_step: 'administrators' }, {
        button, where: { slot }, words: { unlock: 'change the sign-in rules', running: 'Asking administrators for a second step', done: 'Administrators are asked for a second step.' },
      });
      if (!set.ok) return false;
    }
    const args = trial ? { reach, try: seconds() } : { reach };
    const result = await view.api.run('console.reach', args, {
      button, where: { slot }, words: { unlock: reach === 'any' ? 'make the console public' : 'limit the console to this network', running: 'Changing who may reach the console', done: trial ? 'Who may reach the console is being tried.' : 'The console now answers this network only.' },
    });
    if (result.ok && trial) await banner.begin({ seconds: seconds() });
    reread();
    return result.ok;
  };
  // A change that takes this browser away is only tried; making the console public is tried so that a change not kept goes back.
  const buttons = [{ label: 'Try it', kind: 'default', submit: (button, slot) => run(button, slot, true) }];
  if (reach === 'private' && !cuts) buttons.unshift({ label: 'Apply', kind: '', submit: (button, slot) => run(button, slot, false) });
  const dialog = openForm(view, {
    title: reach === 'any' ? 'Make the console public' : 'Only this network',
    icon: 'lock',
    lead: reach === 'any'
      ? publicWords(rules)
      : 'The console will answer this machine, the networks of its cards and the private address ranges, WireGuard included, and no one else.',
    fields: [second?.element, warning, h('div', { class: 'field' }, h('label', { for: select.id }, 'Undo it after'), select, h('span', { class: 'hint' }, 'The change is undone then unless it is kept.'))],
    command: () => ({ action: 'console.reach', args: { reach, try: seconds() } }),
    buttons,
  });
  await dialog.closed;
}
