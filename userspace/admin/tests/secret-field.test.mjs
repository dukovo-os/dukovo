// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/secret-field.js (design 3.5 and 3.6, ADR 0561): what the field asks of
// the browser, how Show works, and that a typed secret lives in its input
// until `take()` gives it once, and reaches nothing else: no attribute, no
// text of the page, no storage, no address and no line of the console's log.

import assert from 'node:assert/strict';
import test from 'node:test';

import { usePage } from './fake-dom.mjs';

const MARKER = 'correct-horse-MARKER-7731';
const OTHER = 'battery-staple-MARKER-1904';

/** A field of `options` on a fresh page, with the parts of it a test types into and reads. */
async function field(context, options = {}) {
  const { document } = usePage(context);
  const { makeSecretField } = await import('../lib/secret-field.js');
  const secret = makeSecretField({ label: 'New password', ...options });
  document.body.append(secret.element);
  const inputs = secret.element.querySelectorAll('input');
  const slot = secret.element.querySelector('[role="alert"]');
  const toggle = secret.element.querySelector('button');
  return { document, secret, inputs, slot, toggle };
}

test('importing the module does nothing and needs no page', async () => {
  assert.equal(typeof globalThis.document, 'undefined');
  const module = await import('../lib/secret-field.js');
  assert.equal(typeof module.makeSecretField, 'function');
});

test('a password is asked for as a new one, and neither spelled nor corrected nor named', async (context) => {
  const { inputs } = await field(context);
  assert.equal(inputs.length, 1);
  const [input] = inputs;
  assert.equal(input.getAttribute('type'), 'password');
  assert.equal(input.getAttribute('autocomplete'), 'new-password');
  assert.equal(input.getAttribute('spellcheck'), 'false');
  assert.equal(input.getAttribute('autocapitalize'), 'off');
  assert.equal(input.getAttribute('autocorrect'), 'off');
  // An input with no name is sent by no form, whatever submits it.
  assert.equal(input.hasAttribute('name'), false);
});

test('a key is not offered to the browser to keep, and is set in the mono face', async (context) => {
  const { inputs } = await field(context, { kind: 'key', label: 'Recovery key' });
  assert.equal(inputs[0].getAttribute('autocomplete'), 'off');
  assert.ok(inputs[0].classes.has('mono'));
});

test('a password that already exists lets the browser fill it in, is typed once, and is otherwise a password field', async (context) => {
  const { secret, inputs } = await field(context, { kind: 'current', label: 'Your password' });
  assert.equal(inputs.length, 1);
  assert.equal(inputs[0].getAttribute('type'), 'password');
  assert.equal(inputs[0].getAttribute('autocomplete'), 'current-password');
  assert.ok(inputs[0].classes.has('input') && !inputs[0].classes.has('mono'));
  assert.equal(inputs[0].hasAttribute('name'), false);
  inputs[0].value = 'the one I have';
  assert.equal(secret.take(), 'the one I have');
  assert.equal(inputs[0].value, '', 'it is given once, as a new password is');
});

test('each field has a label that is seen, and the first is the one the label names', async (context) => {
  const { secret, inputs } = await field(context, { confirm: true });
  const labels = secret.element.querySelectorAll('label');
  assert.deepEqual(labels.map((label) => [label.getAttribute('for'), label.textContent]), [
    [inputs[0].id, 'New password'],
    [inputs[1].id, 'New password, again'],
  ]);
  assert.equal(inputs[1].getAttribute('autocomplete'), 'new-password');
});

test('Show is a toggle that says whether it is on, and changes how every field is drawn', async (context) => {
  const { inputs, toggle } = await field(context, { confirm: true });
  assert.equal(toggle.getAttribute('aria-pressed'), 'false');
  assert.equal(toggle.getAttribute('aria-label'), 'Show new password');
  toggle.click();
  assert.equal(toggle.getAttribute('aria-pressed'), 'true');
  assert.deepEqual(inputs.map((input) => input.getAttribute('type')), ['text', 'text']);
  assert.equal(toggle.textContent, 'Show', 'a toggle keeps its name and says its state with aria-pressed');
  toggle.click();
  assert.deepEqual(inputs.map((input) => input.getAttribute('type')), ['password', 'password']);
});

test('what is typed is given once, and the fields are empty after', async (context) => {
  const { secret, inputs, toggle } = await field(context, { confirm: true });
  toggle.click();
  inputs[0].value = MARKER;
  inputs[1].value = MARKER;
  assert.equal(secret.take(), MARKER);
  assert.deepEqual(inputs.map((input) => input.value), ['', '']);
  assert.deepEqual(inputs.map((input) => input.getAttribute('type')), ['password', 'password'], 'what is typed next is hidden again');
  assert.equal(toggle.getAttribute('aria-pressed'), 'false');
  // The second call finds nothing: the value was handed over once.
  assert.equal(secret.take(), null);
});

test('an empty field is refused under the field, by its label, and the focus goes there', async (context) => {
  const { document, secret, inputs, slot } = await field(context);
  assert.equal(secret.take(), null);
  assert.match(slot.textContent, /Type the new password\./);
  assert.equal(inputs[0].getAttribute('aria-invalid'), 'true');
  assert.equal(inputs[0].getAttribute('aria-describedby'), slot.id);
  assert.equal(document.activeElement, slot);
  inputs[0].fire('input');
  assert.equal(slot.textContent, '', 'typing takes the refusal away');
  assert.equal(inputs[0].hasAttribute('aria-invalid'), false);
});

test('an optional field may stay empty, and then gives the empty text', async (context) => {
  const { secret } = await field(context, { optional: true });
  assert.equal(secret.take(), '');
});

test('two fields that differ give nothing, and keep what was typed for the person to put right', async (context) => {
  const { secret, inputs, slot } = await field(context, { confirm: true });
  inputs[0].value = MARKER;
  inputs[1].value = OTHER;
  assert.equal(secret.take(), null);
  assert.match(slot.textContent, /The two are not the same\./);
  assert.equal(inputs[1].getAttribute('aria-invalid'), 'true');
  assert.deepEqual(inputs.map((input) => input.value), [MARKER, OTHER]);
  assert.equal(slot.textContent.includes('MARKER'), false, 'the refusal never quotes what was typed');
  inputs[1].value = MARKER;
  assert.equal(secret.take(), MARKER);
});

test('a refusal of the server is shown under the field, in the server\'s own words', async (context) => {
  const { secret, inputs, slot } = await field(context);
  secret.fail({ id: 'ADM-123', title: 'That password is too short', detail: 'Use at least 8 characters.', fix: '' });
  assert.match(slot.textContent, /That password is too short\. Use at least 8 characters\./);
  assert.equal(inputs[0].getAttribute('aria-invalid'), 'true');
});

test('clear empties the fields, and so does the page being left', async (context) => {
  const leaving = new AbortController();
  const { secret, inputs } = await field(context, { confirm: true, signal: leaving.signal });
  inputs[0].value = MARKER;
  inputs[1].value = MARKER;
  secret.clear();
  assert.deepEqual(inputs.map((input) => input.value), ['', '']);
  inputs[0].value = MARKER;
  leaving.abort();
  assert.equal(inputs[0].value, '');
});

test('a field is a password, a current password or a key, and only a new password is typed twice', async (context) => {
  usePage(context);
  const { makeSecretField } = await import('../lib/secret-field.js');
  assert.throws(() => makeSecretField({ label: 'x', kind: 'token' }), /a password, a current password or a key, not token/);
  assert.throws(() => makeSecretField({ label: 'x', kind: 'key', confirm: true }), /only a password is typed twice/);
  assert.throws(() => makeSecretField({ label: 'x', kind: 'current', confirm: true }), /only a password is typed twice/);
});

/** Everything on the page that a secret must not be in: each attribute and text of every element. */
const pageText = (document) => [document.body, ...document.body.descendants]
  .flatMap((node) => [...node.attributes.values(), node.textContent]);

test('a secret typed, refused, shown and taken is in no attribute or text of the page, and touches no storage, address or log', async (context) => {
  const touched = [];
  const watch = (name) => new Proxy({}, {
    get: (_, key) => { touched.push(`${name}.${String(key)}`); return () => touched.push(`${name}()`); },
    set: (_, key) => { touched.push(`${name}.${String(key)} =`); return true; },
  });
  const was = {};
  for (const name of ['localStorage', 'sessionStorage', 'history', 'location']) {
    was[name] = Object.getOwnPropertyDescriptor(globalThis, name);
    Object.defineProperty(globalThis, name, { value: watch(name), configurable: true });
  }
  const logged = [];
  for (const name of ['log', 'info', 'warn', 'error', 'debug']) context.mock.method(console, name, (...words) => logged.push(words.join(' ')));
  context.after(() => {
    for (const [name, descriptor] of Object.entries(was)) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  });

  const { document, secret, inputs, toggle } = await field(context, { confirm: true });
  inputs[0].value = MARKER;
  inputs[1].value = OTHER;
  inputs.forEach((input) => input.fire('input'));
  assert.equal(secret.take(), null);
  toggle.click();
  assert.equal(pageText(document).some((text) => text.includes('MARKER')), false, 'not in an attribute or a text, while it is typed');
  inputs[1].value = MARKER;
  assert.equal(secret.take(), MARKER);

  assert.equal(pageText(document).some((text) => text.includes('MARKER')), false);
  assert.deepEqual(touched, []);
  assert.deepEqual(logged, []);
});
