// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "Add a webhook…": the kind, the address and the token (both secret fields,
// since an ntfy topic's address or a Slack webhook's address is the secret
// itself), the room for Matrix, and what it is told. Read when the button is
// pressed.

import { h, uniqueId } from '../../dom.js';
import { openFormDialog } from '../../lib/form-dialog.js';
import { makeSecretField } from '../../lib/secret-field.js';
import { peek, refuse } from './cards.js';
import {
  roomProblem, takesToken, tokenProblem, urlProblem, webhookArgs, WEBHOOKS,
} from './forms-model.js';
import { kindsFields, severityFields } from './told-fields.js';

const JSON_HINT = 'Works with ntfy, Matrix, Slack, Discord or a program of your own.';

/** Opens the dialog; gives whether a webhook was added. */
export function openWebhook(view) {
  const id = uniqueId('hook');
  const kind = h('select', { id: `${id}-kind`, class: 'input', 'aria-describedby': `${id}-kind-hint` },
    WEBHOOKS.map((one) => h('option', { value: one.id }, one.label)));
  const kindHint = h('span', { id: `${id}-kind-hint`, class: 'hint' });
  const url = makeSecretField({ label: 'Address', kind: 'key', hint: 'An ntfy topic\'s address, or the address a Slack or Discord webhook gives.', signal: view.signal });
  const token = makeSecretField({ label: 'Token', kind: 'key', optional: true, hint: 'The access token, when the service asks for one.', signal: view.signal });
  const room = h('input', {
    id: `${id}-room`, class: 'input', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', placeholder: '!abc:example.org',
  });
  const roomField = h('div', { class: 'field' }, h('label', { for: room.id }, 'Room'), room, h('span', { class: 'hint' }, 'The room\'s ID, which starts with an exclamation mark.'));
  const severity = severityFields();
  const kinds = kindsFields();

  /** Shows the fields that the kind takes. */
  const draw = () => {
    token.element.hidden = !takesToken(kind.value);
    roomField.hidden = kind.value !== 'matrix';
    kindHint.textContent = kind.value === 'json' ? JSON_HINT : '';
  };
  kind.addEventListener('change', draw);
  draw();

  const values = () => ({
    kind: kind.value, room: room.value, ...severity.values(), about: kinds.ids(),
  });
  /** The first mistake, `{words, input}`, looking at the secrets without taking them. */
  function mistake() {
    const now = values();
    const address = peek(url);
    const word = address === '' ? 'Type the address.' : urlProblem(now.kind, address);
    if (word) return { words: word, input: url.element.querySelector('input') };
    const where = roomProblem(now.kind, now.room);
    if (where) return { words: where, input: room };
    const secret = takesToken(now.kind) ? tokenProblem(now.kind, peek(token)) : null;
    if (secret) return { words: secret, input: token.element.querySelector('input') };
    const words = kinds.problem();
    return words ? { words, input: null } : null;
  }

  const done = openFormDialog(view, {
    title: 'Add a webhook',
    icon: 'bell',
    lead: ['Alerts are posted to the address you give, over https.'],
    fields: [
      h('div', { class: 'field' }, h('label', { for: kind.id }, 'Kind'), kind, kindHint),
      url.element,
      token.element,
      roomField,
      severity.element,
      kinds.element,
    ],
    label: 'Add the webhook',
    command: () => (mistake() ? null : { action: 'alerts.webhook-add', args: { ...webhookArgs(values(), '', ''), ...(takesToken(kind.value) && peek(token) !== '' ? { token: '' } : {}) } }),
    async submit(button, slot) {
      const found = mistake();
      if (found) {
        refuse(slot, found.words, found.input);
        return false;
      }
      const result = await view.api.run('alerts.webhook-add', webhookArgs(values(), url.take(), takesToken(kind.value) ? token.take() : ''), {
        button, where: { slot }, words: { unlock: 'add a webhook', running: 'Adding the webhook', done: 'The webhook is added.' },
      });
      return result.ok === true;
    },
  });
  return done.finally(() => { url.clear(); token.clear(); });
}
