// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Go back to this, on the version an app keeps: what going back does to the
// app and its data, said before it goes. Read when the button is pressed.

import { rollbackWords } from './app-model.js';
import { openQuestion } from './dialog.js';
import { rollbackCall, runWords } from './run-model.js';

/**
 * Opens the dialog for the app of `info` (`bundle info --json`).
 * `started(id, action, args)` is told of the task that going back begins.
 * Resolves with whether going back was begun.
 */
export function openBack(view, { info, started }) {
  const call = rollbackCall(info.id);
  return openQuestion(view, {
    title: `Go back to ${info.previous}?`,
    icon: 'warn',
    lead: [rollbackWords(info)],
    label: 'Go back',
    command: call,
    async submit(confirm, errors) {
      const result = await view.api.run(call.action, call.args, {
        button: confirm, words: runWords('rollback', info.name, info.previous), where: { slot: errors },
      });
      if (typeof result.task === 'number') started(result.task, call.action, call.args);
      return result.ok;
    },
  });
}
