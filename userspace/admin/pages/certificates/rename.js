// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The dialog that changes the machine's name and domain. It says what the
// new name changes for the address this browser reaches the console by, and
// when the name is changed it hands the page what the new address is: from
// then on the page asks for nothing, since the old name is no longer
// answered. The page reads this file when Change is pressed.

import { clearError, errorSlot, fill, h, showError } from '../../dom.js';
import { openFormDialog } from '../../lib/form-dialog.js';
import {
  fingerprintOf, followAddress, fullNameOf, nameProblem, renameLines,
} from './rename-model.js';

/** Opens the dialog and resolves, when it closes, with whether the name was changed. */
export function openRename(ctx) {
  const { view } = ctx;
  const { machine } = ctx.data;
  const host = globalThis.location?.hostname ?? '';
  const port = ctx.data.about?.port ?? globalThis.location?.port;
  const address = ctx.data.about?.addresses?.[0] ?? null;

  const first = h('input', {
    id: 'rename-first', class: 'input mono', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', 'aria-describedby': 'rename-first-error',
  });
  first.value = machine.first;
  const domain = h('input', {
    id: 'rename-domain', class: 'input mono', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', 'aria-describedby': 'rename-domain-hint',
  });
  domain.value = machine.domain ?? '';
  const firstError = errorSlot('rename-first-error');
  const lines = h('div', { class: 'certs-lines-box', role: 'status' });

  const full = () => fullNameOf(first.value, domain.value);
  const problem = () => nameProblem(first.value, domain.value);
  const changed = () => problem() === '' && full().toLowerCase() !== machine.name.toLowerCase();

  /** Says what the name typed changes, once it is a name and a new one. */
  function say() {
    if (problem() !== '') {
      fill(lines, h('p', { class: 'muted' }, 'Type the new name to see what changes.'));
      return;
    }
    fill(lines, changed()
      ? renameLines(machine, full(), { host, port, address }).map((words) => h('p', {}, words))
      : h('p', { class: 'muted' }, 'This is the name it has.'));
  }
  say();
  for (const input of [first, domain]) {
    input.addEventListener('input', () => {
      clearError(firstError, first);
      say();
    });
  }

  return openFormDialog(view, {
    title: 'Change the machine\'s name',
    icon: 'server',
    lead: [lines],
    fields: [
      h('div', { class: 'field' }, h('label', { for: first.id }, 'Name'), first, firstError),
      h('div', { class: 'field' }, h('label', { for: domain.id }, 'Domain (optional)'), domain, h('span', { id: 'rename-domain-hint', class: 'hint' }, 'The rest of the machine\'s name, such as example.org.')),
    ],
    label: 'Change the name',
    command: () => (changed() ? { action: 'machine.rename', args: { name: full() } } : null),
    async submit(confirm, slot) {
      const why = problem();
      if (why !== '') {
        showError(firstError, { title: why, detail: '', fix: '' }, first);
        return false;
      }
      if (!changed()) {
        showError(firstError, { title: 'This is the name it has.', detail: '', fix: '' }, first);
        return false;
      }
      const name = full();
      const result = await view.api.run('machine.rename', { name }, {
        button: confirm,
        where: { slot, fields: { name: first } },
        words: {
          unlock: 'rename this machine', running: 'Renaming the machine', busy: 'Renaming…', done: `The machine is called ${name} now.`,
        },
      });
      if (!result.ok) return false;
      // The answer is the last the page asks for: the old name is refused from here on.
      ctx.renamed({
        name,
        fingerprint: fingerprintOf(result.result?.output),
        follow: followAddress(host, machine, name, port),
        host,
      });
      return true;
    },
  });
}
