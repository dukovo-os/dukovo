// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "Go back to it…": the question that goes back to the version kept before
// the last update, which settings it starts with, and then the restart. The
// action makes the version before the one that starts next; the restart is
// asked for apart, as every restart is, so that it says who is signed in.

import { h, uniqueId } from '../../dom.js';
import { openFormDialog } from '../../lib/form-dialog.js';
import { keptWords } from './model.js';

/** Opens the question. Gives whether the machine was set to go back. */
export async function openGoBack(ctx) {
  const { view, data } = ctx;
  const kept = keptWords(data.slots);
  if (!kept) {
    view.notice('No version is kept for going back.', { kind: 'warn' });
    return false;
  }
  const running = data.running ?? 'the version that runs now';
  // A fallback already put the settings from before back, so the choice that is made once is today's.
  const before = data.slots.last_update?.settings_restored === false;
  const name = uniqueId('back');
  const choice = (value, words, hint) => {
    const input = h('input', { type: 'radio', name, id: uniqueId('back-choice'), value, 'aria-describedby': `${name}-${value}` });
    input.checked = (value === 'before') === before;
    return { input, node: h('label', { class: 'radio-card', for: input.id }, input, h('span', {}, words, h('span', { id: `${name}-${value}`, class: 'hint' }, ` ${hint}`))) };
  };
  const options = [
    choice('before', `The settings from before the update to ${running}`, 'They are put back at the next start, as a fallback does, and today\'s are kept apart.'),
    choice('keep', 'Keep today\'s settings', 'The settings stay as they are.'),
  ];
  const picked = () => (options[0].input.checked ? 'before' : 'keep');
  const done = await openFormDialog(view, {
    title: `Go back to ${kept.version}?`,
    icon: 'undo',
    lead: `Go back to ${kept.version}. The machine restarts into it; it is kept as the version before, so you can come back to ${running} the same way.`,
    fields: [h('fieldset', { class: 'choices' }, h('legend', {}, 'Settings'), ...options.map((one) => one.node))],
    label: 'Go back and restart',
    command: () => ({ action: 'updates.undo', args: { item: 'system', settings: picked() } }),
    submit: async (button, slot) => (await view.api.run('updates.undo', { item: 'system', settings: picked() }, {
      button, where: { slot }, words: { unlock: 'go back to the version before', done: `${kept.version} starts at the next restart.` },
    })).ok,
  });
  if (!done) return false;
  const { restartNow } = await import('../../lib/restart.js');
  await restartNow(view, { why: `To go back to ${kept.version}.`, kind: 'update', version: kept.version, pending: await ctx.pending() });
  await ctx.reload(null);
  return true;
}
