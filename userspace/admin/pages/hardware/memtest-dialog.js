// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The buttons of the test at the next restart: the dialog of "Test at the
// next restart…", with what the test does and its three ways out, and
// "Don't test". Asking is `hardware.memtest`, which restarts nothing;
// Restart now and test then goes on to the restart's own question. The page
// reads this file when a button is first pressed.

import { ask } from '../../shell/dialogs.js';
import { hereWords } from './memtest-model.js';

export const ASK_TITLE = 'Test the memory at the next restart?';

/** The dialog's words: what the start does, then what each button does. `bytes` is the memory's. */
export const askWords = (host, bytes) => [
  `The next start tests the free memory first${hereWords(bytes)}, then starts the system as usual. The server is offline meanwhile, and nobody needs to be at it. The test stops after an hour at most.`,
  `Restart now and test restarts ${host} in a minute, after one more question. At the next restart changes nothing now.`,
];

export const ASK_CHOICES = Object.freeze([
  { label: 'Restart now and test', value: 'now' },
  {
    label: 'At the next restart', value: 'next', kind: 'default', safe: true,
  },
  { label: 'Cancel', value: 'cancel' },
]);

/**
 * Asks, and asks for the test. `one` is the button that was pressed and
 * `slot` is where a refusal goes (HW-105 says why the next start is taken).
 * Gives whether the test was asked for.
 */
export async function askMemtest(ctx, one, slot) {
  const { view } = ctx;
  const choice = await ask({
    title: ASK_TITLE, words: askWords(view.host, ctx.data.machine?.memory?.bytes), choices: ASK_CHOICES, iconName: 'chip',
  });
  if (choice === null || choice === 'cancel') return false;
  const result = await view.api.run('hardware.memtest', {}, {
    button: one,
    where: { slot },
    words: {
      unlock: 'test the memory at the next restart', running: 'Asking for the test', busy: 'Asking…', done: 'The memory is tested at the next restart.',
    },
  });
  if (!result.ok) return false;
  await ctx.readMemtest();
  if (choice === 'now') await ctx.restartToTest();
  return true;
}

/** Cancels the test that was asked for. `one` is the button that was pressed and `slot` is where a refusal goes. Gives whether it ran. */
export async function cancelMemtest(ctx, one, slot) {
  const result = await ctx.view.api.run('hardware.memtest-cancel', {}, {
    button: one,
    where: { slot },
    words: { unlock: 'cancel the memory test', done: 'The memory test is cancelled.', unchanged: 'No memory test was asked for.' },
  });
  if (!result.ok) return false;
  await ctx.readMemtest();
  return true;
}
