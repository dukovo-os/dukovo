// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The dialog of the networks that are never blocked (design 8.5 and 22.7): the
// list, each with Remove, and a field that adds one. A network that is not
// private, or is wider than a /16 or a /48, is asked about first. Read when the
// dialog is first opened.

import { fill, h } from '../../dom.js';
import { ask } from '../../shell/dialogs.js';
import { openForm } from './dialog.js';
import { textField } from './fields.js';
import { neverCheck } from './rules-form-model.js';

/** Opens the dialog; `reader` is the blocked card's read, `{read(), answer()}`. Resolves when it closes. */
export async function openNeverForm(view, reader) {
  const list = h('ul', { class: 'sec-never' });
  const field = textField({ label: 'Add a network', hint: 'An address or a network, such as 192.168.1.0/24.', mono: true });
  const draw = () => {
    const never = reader.answer()?.never ?? [];
    fill(list, never.length === 0 ? h('li', { class: 'muted' }, 'No network is on the list.') : never.map((network) => h('li', {},
      h('span', { class: 'mono' }, network),
      h('button', { class: 'btn small', type: 'button', 'aria-label': `Remove ${network}`, onclick: async (event) => {
        const result = await view.api.run('security.never-block-remove', { network }, {
          button: event.currentTarget, where: { slot: dialog.errors }, words: { unlock: `remove ${network} from the never-block list`, running: 'Removing', done: `${network} is removed from the list.` },
        });
        if (result.ok) {
          await reader.read();
          draw();
        }
      } }, 'Remove'))));
  };
  const add = {
    label: 'Add to the list',
    kind: 'default',
    async submit(button, slot) {
      const found = neverCheck(field.input.value);
      if (found.problem) {
        field.fault(found.problem);
        return false;
      }
      if (found.ask && await ask({
        title: 'Never block this network?',
        words: found.ask,
        iconName: 'warn',
        choices: [{ label: 'Go back', value: 'back', kind: 'default', safe: true }, { label: 'Never block it', value: 'add' }],
      }) !== 'add') return false;
      const result = await view.api.run('security.never-block', { network: found.network }, {
        button, where: { slot }, words: { unlock: `never block ${found.network}`, running: 'Adding', done: `${found.network} is never blocked.` },
      });
      if (!result.ok) return false;
      await reader.read();
      field.input.value = '';
      draw();
      return false;
    },
  };
  const dialog = openForm(view, {
    title: 'Never blocked',
    icon: 'security',
    lead: 'An address in one of these networks is never blocked, however often it gets a password wrong.',
    fields: [list, field.element],
    command: () => {
      const found = neverCheck(field.input.value);
      return found.problem ? null : { action: 'security.never-block', args: { network: found.network } };
    },
    buttons: [add],
    cancel: 'Close',
  });
  draw();
  await dialog.closed;
}
