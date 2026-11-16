// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The email channel's fields, for the Email card and for the step of Finish
// setting up: who it is sent to, the provider's server and port, the user name
// and the password, who it is from, and what it is told of. The password is a
// secret field: it is read once, when the form is sent, and nowhere else.

import { h, uniqueId } from '../../dom.js';
import { makeSecretField } from '../../lib/secret-field.js';
import { peek } from './cards.js';
import { emailArgs, emailProblems } from './forms-model.js';
import { kindsFields, severityFields } from './told-fields.js';

/**
 * The fields for `channel`, the email channel as alertd lists it (null when
 * none is set), as `{element, check, take, shown, clear}`. `told: false`
 * leaves out how bad an alert has to be and the kinds (the step's form):
 * every alert and every kind are then the defaults. `check()` gives the first
 * mistake as `{words, input}` or null, looking at the password without taking
 * it; `take()` reads the password, once, and gives the arguments; `shown()` gives the arguments with an empty
 * password for the panel that shows the command, and reads nothing.
 */
export function makeEmailFields(view, { channel = null, told = true } = {}) {
  const id = uniqueId('email');
  const text = (name, attributes = {}) => h('input', {
    id: `${id}-${name}`, class: 'input', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', ...attributes,
  });
  const to = h('textarea', {
    id: `${id}-to`, class: 'input', rows: '2', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', 'aria-describedby': `${id}-to-hint`,
  });
  const server = text('server', { value: channel?.host ?? '', placeholder: 'mail.example.org', 'aria-describedby': `${id}-server-hint` });
  const port = h('select', { id: `${id}-port`, class: 'input' }, h('option', { value: '587' }, '587 (STARTTLS)'), h('option', { value: '465' }, '465 (TLS from the start)'));
  const user = text('user', { autocomplete: 'username' });
  const from = text('from', { placeholder: `alerts@${view.host || 'this-server'}`, 'aria-describedby': `${id}-from-hint` });
  const secret = makeSecretField({
    label: 'Password', kind: 'password', optional: true, hint: channel ? 'Leave empty to keep the password it has.' : null, signal: view.signal,
  });
  const severity = told ? severityFields(channel) : null;
  const kinds = told ? kindsFields(channel) : null;

  const field = (label, input, hint) => h('div', { class: 'field' },
    h('label', { for: input.id }, label), input, hint && h('span', { id: `${input.id}-hint`, class: 'hint' }, hint));

  const values = () => ({
    to: to.value,
    server: server.value,
    port: port.value,
    user: user.value,
    from: from.value,
    severity: severity?.values().severity ?? 'warning',
    cleared: severity?.values().cleared ?? true,
    about: kinds?.ids() ?? ['data', 'stopped', 'hardware'],
  });

  return {
    element: h('div', { class: 'al-form' },
      field('Send to', to, 'One address a line, or separated by commas.'),
      field('Mail server', server, 'Sent through your mail provider\'s server, always over TLS. Many providers want an app password made for this.'),
      field('Port', port),
      field('Sign in as', user),
      secret.element,
      field('From (optional)', from, `alerts@${view.host || 'this server'} unless your provider wants the account's own address.`),
      severity?.element,
      kinds?.element),

    /** The first mistake in what was typed, `{words, input}`, or null. */
    check() {
      const found = emailProblems(values());
      for (const [name, input] of [['to', to], ['server', server], ['from', from]]) {
        if (found[name]) return { words: found[name], input };
      }
      const typed = peek(secret) !== '';
      if (user.value.trim() !== '' && !typed && channel === null) {
        return { words: 'Type the password: the mail server asks for one with a user name.', input: secret.element.querySelector('input') };
      }
      if (user.value.trim() === '' && typed) return { words: 'A password goes with a user name: type the user name.', input: user };
      const words = kinds?.problem();
      return words ? { words, input: null } : null;
    },

    /** Reads the password, once, with the fields: the arguments of `alerts.email`. */
    take: () => emailArgs(values(), secret.take() ?? ''),

    /** The arguments for the panel that shows the command: a typed password is `''`. */
    shown() {
      return { ...emailArgs(values(), ''), ...(peek(secret) === '' ? {} : { password: '' }) };
    },

    clear: secret.clear,
  };
}
