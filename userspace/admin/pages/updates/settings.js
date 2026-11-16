// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "How this server updates": when it looks, whether the system and the apps
// update by themselves, what a metered connection waits for, and the channel.
// An administrator changes them with Save, which makes `update set` and
// `sysupdate channel` arguments of what moved, and may take the change back
// at once; everyone else reads the answers in words.

import { fill, h, uniqueId } from '../../dom.js';
import { button, makeCard } from './cards.js';
import {
  argsOf, channelNote, formOf, noteOf, QUESTIONS,
} from './settings-model.js';

/** The card. Gives `{element, draw}`. */
export function makeSettings(ctx) {
  const card = makeCard('settings', 'How this server updates');
  const status = h('p', { class: 'hint', role: 'status' });

  /** Runs what `args` (from argsOf) says, one action each; false when one was refused. */
  async function apply(args, one, words) {
    if (args.set && !(await ctx.run(card, 'updates.set', args.set, one, { unlock: 'change how this server updates', done: words })).ok) return false;
    if (args.channel && !(await ctx.run(card, 'updates.channel', { channel: args.channel }, one, { unlock: 'change the channel', done: words })).ok) return false;
    return true;
  }

  function draw() {
    const { data, admin } = ctx;
    if (!data.settings) {
      card.head(null);
      fill(card.body, h('p', { class: 'muted' }, 'How this server updates could not be read.'));
      return;
    }
    const before = formOf(data.settings);
    const inputs = {};
    const notes = {};
    const timeInput = h('input', { id: uniqueId('restart'), class: 'input', type: 'time' });
    timeInput.value = before.restart;
    const values = () => ({
      ...Object.fromEntries(QUESTIONS.map((one) => [one.key, inputs[one.key].find((input) => input.checked)?.getAttribute('value') ?? before[one.key]])),
      restart: timeInput.value,
    });
    const refresh = () => {
      const now = values();
      timeInput.disabled = now.system !== 'auto';
      for (const { key } of QUESTIONS) {
        notes[key].textContent = (key === 'channel' ? channelNote(before.channel, now.channel, data.running) : noteOf(key, now, { disagree: data.settings?.apps_disagree === true })) ?? '';
      }
    };
    if (!admin) {
      card.head(null);
      fill(card.body, h('dl', { class: 'rows' }, ...QUESTIONS.flatMap((one) => [h('dt', {}, one.legend), h('dd', {}, one.choices.find(([value]) => value === before[one.key])?.[1] ?? before[one.key])])));
      return;
    }
    const groups = QUESTIONS.map((one) => {
      const note = h('span', { id: uniqueId(`${one.key}-note`), class: 'hint' });
      notes[one.key] = note;
      inputs[one.key] = one.choices.map(([value]) => {
        const input = h('input', { type: 'radio', name: `upd-${one.key}`, id: uniqueId(`upd-${one.key}`), value, 'aria-describedby': note.id });
        input.checked = value === before[one.key];
        input.addEventListener('change', refresh);
        return input;
      });
      return h('fieldset', { class: 'choices' }, h('legend', {}, one.legend),
        h('div', { class: 'cards-row' }, ...one.choices.map(([, words], at) => h('label', { class: 'radio-card', for: inputs[one.key][at].id }, inputs[one.key][at], h('span', {}, words)))),
        one.key === 'system' && h('div', { class: 'field' }, h('label', { for: timeInput.id }, 'Restart at'), timeInput),
        note);
    });
    const save = button('Save', async (one) => {
      const now = values();
      const args = argsOf(before, now);
      fill(card.errors);
      if (args.problem) {
        status.textContent = args.problem;
        return;
      }
      if (!args.set && !args.channel) {
        status.textContent = 'Nothing to change.';
        return;
      }
      const ok = await apply(args, one, 'Saved: how this server updates.');
      await ctx.reload(card.heading);
      if (!ok) return;
      const undo = button('Undo', async (back) => {
        if (await apply(argsOf(now, before), back, 'Put back as it was.')) {
          await ctx.reload(card.heading);
          status.textContent = 'Put back as it was.';
        }
      });
      fill(status, 'Saved. ', undo);
    }, 'default');
    card.head(null);
    fill(card.body, h('div', { class: 'choice-groups' }, ...groups), h('div', { class: 'btns' }, save), status);
    refresh();
  }
  return { element: card.element, draw };
}
