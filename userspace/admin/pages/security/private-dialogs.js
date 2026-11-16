// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The dialogs of private access (design 8.8 and 9.3): Set up, which makes wg0
// and opens WireGuard's port; Add a device, whose answer, the device's settings
// with its private key, is shown once; and Remove, which admind refuses for the
// device this browser comes through (ADM-137). Read when the first is opened.
// `reader` is the card's read, `{read(), answer()}`.

import { h } from '../../dom.js';
import { showOnce } from '../../lib/shown-once.js';
import { ask } from '../../shell/dialogs.js';
import { openForm } from './dialog.js';
import { textField } from './fields.js';
import { serveAndOpen, setUpFields } from './private-form.js';
import { deviceFile, fileName, nameProblem, REACH_WORDS } from './private-form-model.js';
import { WHAT } from './private-model.js';

/** Set up private access: the address devices reach this server at, the port and the network. */
export async function openSetUp(view, reader) {
  const fields = setUpFields(view.host ?? location.hostname);
  const dialog = openForm(view, {
    title: 'Set up private access',
    icon: 'lock',
    lead: [WHAT, 'Setting it up opens WireGuard\'s UDP port in the firewall.'],
    fields: fields.elements,
    command: () => {
      const { args } = fields.check();
      return args && { action: 'private.serve', args };
    },
    buttons: [{
      label: 'Set up',
      kind: 'default',
      async submit(button, slot) {
        const { args, problems } = fields.check();
        if (args === null) {
          fields.fault(problems);
          return false;
        }
        const made = await serveAndOpen(view, args, { button, where: { slot } });
        if (made) reader.read();
        return made;
      },
    }],
  });
  await dialog.closed;
}

/** Add a device by its name; the answer holds its private key and is shown once. `taken` are the names of the devices there are. */
export async function openAdd(view, reader, taken) {
  const name = textField({ label: 'Name of the device', hint: '1 to 32 letters, digits, - and _, such as phone.' });
  let made = null;
  const dialog = openForm(view, {
    title: 'Add a device',
    icon: 'key',
    lead: [REACH_WORDS, 'The device\'s settings, with its private key, are shown once. The server keeps the device\'s public key only.'],
    fields: [name.element],
    command: () => (nameProblem(name.input.value, taken) ? null : { action: 'private.add', args: { name: name.input.value.trim() } }),
    buttons: [{
      label: 'Add the device',
      kind: 'default',
      async submit(button, slot) {
        const problem = nameProblem(name.input.value, taken);
        if (problem) {
          name.fault(problem);
          return false;
        }
        const args = { name: name.input.value.trim() };
        // The answer is a secret: it is asked once, never again by the page, and kept nowhere but in the dialog that shows it.
        const result = await view.api.run('private.add', args, {
          button, where: { slot }, words: { unlock: 'add a device', running: 'Adding the device', done: `${args.name} is added.` },
        });
        if (!result.ok) return false;
        made = { name: args.name, text: deviceFile(result.result) };
        return true;
      },
    }],
  });
  if (!(await dialog.closed)) return;
  reader.read();
  const { name: deviceName, text } = made;
  made = null;
  // The device is added either way; without its file it cannot be set up, and its private key is nowhere else.
  if (text === null) {
    view.notice(`${deviceName} is added, but the answer did not hold its settings. Remove ${deviceName} and add it again.`, { kind: 'warn' });
    return;
  }
  await showOnce(view, {
    title: `Settings for ${deviceName}`,
    label: 'Settings',
    text,
    file: fileName(deviceName),
    qr: true,
    warn: 'Scan this with the WireGuard app on your phone or laptop to add it to wg0. These settings hold the device\'s private key: the server keeps the public key only, and shows them only now.',
    loss: 'The device\'s private key is shown only now. Without it the device must be removed and added again.',
  });
}

/** Remove a device, after asking. admind refuses the device this browser comes through (ADM-137), and says so. */
export async function removeDevice(view, reader, { name, button, errors }) {
  const choice = await ask({
    title: `Remove ${name}?`,
    words: [`${name} can no longer reach this server through wg0. Its settings on the device stop working.`, 'The device can be added again, with new settings.'],
    iconName: 'warn',
    choices: [{ label: 'Keep it', value: 'keep', kind: 'default', safe: true }, { label: `Remove ${name}`, value: 'remove', kind: 'danger' }],
  });
  if (choice !== 'remove') return;
  const result = await view.api.run('private.remove', { name }, {
    button, where: { slot: errors }, words: { unlock: `remove ${name}`, running: 'Removing the device', done: `${name} is removed.` },
  });
  if (result.ok) reader.read();
  // admind refuses the device this browser comes through, since removing it would cut this browser off; its words are above, and this says where to go.
  if (result.error?.id === 'ADM-137') errors.append(h('p', { class: 'small' }, 'This browser is connected through that device. Open the console over this server\'s own network, or from another device, and remove it there.'));
}
