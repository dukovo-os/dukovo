// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What to put in a client's settings, shown when a client was added.
// One that connects over SSH gets the command its settings run and a check
// to try from a terminal: nothing in either is secret. One that connects
// over HTTPS gets its token, which authd answered once: the dialog of
// secrets (lib/shown-once.js) shows it, asks before it is closed unkept
// and leaves nothing of it in the page. The settings name the machine as
// the browser reaches it, since the client is most often on the same
// computer as the browser.

import { h, icon, uniqueId } from '../../dom.js';
import { showOnce } from '../../lib/shown-once.js';
import { head, openModal } from '../../shell/dialogs.js';
import {
  certificateOf, httpsSettings, sshCheck, sshHost, sshSettings,
} from './model.js';
import { commandBox } from './parts.js';

const SETTINGS = 'Put this in your AI client\'s MCP settings';
const SSH_OK = 'It connects over SSH and signs in as you, with your SSH key. Nothing secret is kept on the other machine.';
const TOKEN_WARN = ' Copy it now, and keep it as you keep a password. It can read what this client reads. It can change nothing.';
const TOKEN_LOSS = 'The token is shown only now. Without it the client cannot connect, and you would remove the client and add it again.';
const CERT_COMMAND = 'cert show this-machine --pem';

/** A box of text to copy under the words that say what it is for. */
const captioned = (words, box) => h('div', { class: 'field' }, h('span', { class: 'label' }, words), box.element);

/** What a client that connects over HTTPS needs to know of the console's certificate, or null when another's vouches for it. */
function certificateNote({ own, fingerprint }) {
  if (own === false) return null;
  const command = h('span', { class: 'mono' }, CERT_COMMAND);
  const words = own === null
    ? [h('p', {}, 'If the AI client does not trust this server\'s certificate, ', command, ' prints the certificate to give it. With SSH there is none of this.')]
    : [
      h('p', {}, 'This server made its own certificate, so the AI client has to be told to trust it. ', command, ' prints the certificate, and its SHA-256 fingerprint is:'),
      h('p', { class: 'cmdtext' }, fingerprint.join('\n')),
      h('p', {}, 'With SSH there is none of this.'),
    ];
  return h('div', { class: 'banner info' }, icon('info'), h('div', {}, words));
}

/** The dialog for a client that connects over SSH. */
function sshDialog(view, { name }, { server, hostname }) {
  const id = uniqueId('cl-connect');
  const owner = view.session.name;
  const host = sshHost(hostname);
  const settings = commandBox('MCP settings', sshSettings({ server, host, owner, name }), 'MCP settings for the AI client');
  const check = commandBox('Says what the client may reach, and leaves', sshCheck({ host, owner, name }), 'A check from a terminal');
  const dialog = h('dialog', { class: 'dialog', 'aria-labelledby': `${id}-h` },
    h('form', { method: 'dialog' },
      head(`${id}-h`, 'check', `${name} is added`),
      h('div', { class: 'dialog-body' },
        h('div', { class: 'banner ok' }, icon('check'), h('div', {}, h('p', {}, SSH_OK))),
        captioned(SETTINGS, settings),
        captioned('Or try it from a terminal first', check),
        h('p', { class: 'small muted' }, 'If SSH still asks for a password, add your key first, under ',
          h('a', { href: `/accounts/${encodeURIComponent(owner)}#keys` }, 'SSH keys of your account'), '.')),
      h('div', { class: 'dialog-foot' }, h('button', { class: 'btn default', type: 'submit', autofocus: true }, 'Done'))));
  view.signal.addEventListener('abort', () => dialog.close(''), { once: true });
  return openModal(dialog);
}

/**
 * Shows how the client `added`, `{name, way, token}` as the Add dialog gave
 * it, is connected, and resolves when the dialog closes. `found` is what
 * the page read: `about`, `GET /api/1/console/about`'s answer, and
 * `machine`, the machine's own certificate, either of them null. `where`
 * is the address the browser reaches the console by.
 */
export async function openConnect(view, added, { about = null, machine = null } = {}, where = globalThis.location) {
  const server = view.host || where.hostname;
  if (added.way !== 'https') {
    await sshDialog(view, added, { server, hostname: where.hostname });
    return;
  }
  await showOnce(view, {
    title: `${added.name} is added`,
    label: SETTINGS,
    text: httpsSettings({ server, origin: where.origin, token: added.token }),
    warn: [h('b', {}, 'The token is shown once.'), TOKEN_WARN],
    loss: TOKEN_LOSS,
    kept: 'I have copied the token',
    note: certificateNote(certificateOf(about, machine)),
  });
}
