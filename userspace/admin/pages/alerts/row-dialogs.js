// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The two questions a channel's row asks: what it is told of (the three kinds
// of alert as checkboxes, saved by `alerts.channel-about`) and whether to
// remove it. Read when a row's button is pressed.

import { openFormDialog } from '../../lib/form-dialog.js';
import { refuse } from './cards.js';
import { kindsFields } from './told-fields.js';
import { removeCall } from './model.js';
import { aboutArg } from './told-model.js';

/** Opens the dialog for `row` (a row of model.js's channelRows); gives whether the channel was changed. */
export function openTold(view, row) {
  const kinds = kindsFields(row.channel);
  const args = () => ({ channel: row.arg, about: aboutArg(kinds.ids()) });
  return openFormDialog(view, {
    title: 'What it is told of',
    icon: 'bell',
    lead: [`Choose the kinds of alert that ${row.name} is told of.`],
    fields: [kinds.element],
    label: 'Save',
    command: () => (kinds.problem() ? null : { action: 'alerts.channel-about', args: args() }),
    async submit(button, slot) {
      const problem = kinds.problem();
      if (problem) {
        refuse(slot, problem);
        return false;
      }
      const result = await view.api.run('alerts.channel-about', args(), {
        button, where: { slot }, words: { unlock: `change what ${row.name} is told`, done: `${row.name}: what it is told is saved.` },
      });
      return result.ok === true;
    },
  });
}

/** Opens the question that removes the channel of `row`; gives whether it was removed. */
export function openRemove(view, row) {
  const call = removeCall(row.channel);
  return openFormDialog(view, {
    title: `Remove ${row.name}?`,
    icon: 'warn',
    lead: [`Alerts stop going to ${row.name}. ${call.words}`],
    label: 'Remove',
    danger: true,
    command: { action: call.action, args: call.args },
    async submit(button, slot) {
      const result = await view.api.run(call.action, call.args, {
        button, where: { slot }, words: { unlock: `remove ${row.name}`, done: `${row.name} was removed.` },
      });
      return result.ok === true;
    },
  });
}
