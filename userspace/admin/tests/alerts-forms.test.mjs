// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The email form and the webhook dialog: what they refuse at the field, which
// action each runs with what, and that a password, an address or a token is
// only in the body of that action, and out of the page's text, its
// attributes, its notices and what the browser keeps, whichever way the form
// is left.

import assert from 'node:assert/strict';
import test from 'node:test';

import { emailChannel } from './alerts-fixtures.mjs';
import { buttonOf, openAlerts, useStorage } from './alerts-view.mjs';

const words = (node) => (node.children ? node.children.map(words).join(' ') : node.text);
const text = (root) => words(root).replace(/\s+/g, ' ').trim();

/** The field of `root` whose id ends with `-name`. */
const controls = (root) => ['input', 'textarea', 'select'].flatMap((tag) => root.querySelectorAll(tag));
const field = (root, name) => controls(root).find((one) => one.id.endsWith(`-${name}`));

/** The password field of the email form: the one input that is not named by id. */
const secretOf = (root) => root.querySelectorAll('input').find((one) => /^secret-\d+$/.test(one.id));

/** Everything the page shows as words or attributes, to look a secret up in. */
function everything(world) {
  const nodes = [world.view.main, ...world.document.body.querySelectorAll('dialog')];
  const found = [];
  const walk = (node) => {
    if (node.attributes) for (const value of node.attributes.values()) found.push(value);
    if (node.text !== undefined) found.push(node.text);
    for (const child of node.children ?? []) walk(child);
  };
  nodes.forEach(walk);
  return found.join('\n');
}

const fillEmail = (world, values) => {
  const card = world.card('email');
  field(card, 'to').value = values.to ?? 'ops@example.org';
  field(card, 'server').value = values.server ?? 'mail.example.org';
  field(card, 'port').value = values.port ?? '587';
  field(card, 'user').value = values.user ?? '';
  field(card, 'from').value = values.from ?? '';
  if (values.password !== undefined) secretOf(card).value = values.password;
  return card;
};

test('Save sends the email form to alerts.email, the password only in that call', async (context) => {
  const kept = useStorage(context);
  const world = await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': [] } } });
  const card = fillEmail(world, { to: 'ops@example.org, admin@example.org', user: 'ops', password: 'hunter2-secret', port: '465' });
  await world.press(card, 'Save');
  assert.deepEqual(world.ran('alerts.email'), [{
    to: 'ops@example.org\nadmin@example.org', server: 'mail.example.org', port: '465', user: 'ops', severity: 'warning', cleared: true, password: 'hunter2-secret',
  }]);
  assert.equal(secretOf(card).value, '', 'the password field is empty after');
  assert.doesNotMatch(everything(world), /hunter2-secret/);
  assert.doesNotMatch([...kept.values()].join('\n'), /hunter2-secret/);
  assert.doesNotMatch(JSON.stringify(world.notes), /hunter2-secret/);
  assert.match(text(card), /Saved\./);
  assert.ok(buttonOf(card, 'Send a test alert'), 'the page offers a test after Save');
});

test('the command shown for the email form holds an empty password, never the typed one', async (context) => {
  useStorage(context);
  const world = await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': [] } } });
  const card = fillEmail(world, { user: 'ops', password: 'hunter2-secret' });
  card.querySelector('.al-form').fire('input');
  await new Promise((resolve) => setTimeout(resolve, 500));
  const panel = card.querySelector('[data-panel]');
  assert.ok(panel, 'the command shows once every field is right');
  assert.match(panel.getAttribute('data-panel'), /"password":""/);
  assert.doesNotMatch(panel.getAttribute('data-panel'), /hunter2-secret/);
});

test('the email form says what is wrong at the field and sends nothing', async (context) => {
  useStorage(context);
  const world = await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': [] } } });
  const card = fillEmail(world, { to: 'nobody' });
  await world.press(card, 'Save');
  assert.match(text(world.card('email').querySelector('.al-errors')), /nobody is not an email address\./);
  assert.equal(field(card, 'to').getAttribute('aria-invalid'), 'true');
  assert.deepEqual(world.ran('alerts.email'), []);
  field(card, 'to').value = 'ops@example.org';
  field(card, 'user').value = 'ops';
  await world.press(card, 'Save');
  assert.match(text(world.card('email').querySelector('.al-errors')), /Type the password/, 'a user name goes with a password');
  assert.deepEqual(world.ran('alerts.email'), []);
  field(card, 'user').value = '';
  secretOf(card).value = 'typed-but-no-user';
  await world.press(card, 'Save');
  assert.match(text(world.card('email').querySelector('.al-errors')), /A password goes with a user name/);
  assert.equal(secretOf(card).value, 'typed-but-no-user', 'what was typed stays for the person to put right');
  assert.deepEqual(world.ran('alerts.email'), []);
});

test('on an email channel that is set, an empty password keeps the one it has', async (context) => {
  useStorage(context);
  const world = await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': [emailChannel()] } } });
  const card = world.card('email');
  assert.match(text(card), /Leave empty to keep the password it has\./);
  assert.equal(field(card, 'server').getAttribute('value'), 'mail.example.org', 'the server is what the status shows');
  fillEmail(world, { user: 'ops' });
  await world.press(card, 'Save');
  const [args] = world.ran('alerts.email');
  assert.equal('password' in args, false, 'no password member: the command keeps the one it has');
  assert.equal(args.user, 'ops');
});

test('Remove email asks first and runs alerts.email-remove', async (context) => {
  useStorage(context);
  const world = await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': [emailChannel()] } } });
  await world.press(world.card('email'), 'Remove email');
  assert.match(text(world.dialog()), /Remove email to ops@example\.org and 1 more\?/);
  world.dialog().querySelector('form').fire('submit');
  await world.emit('none');
  assert.deepEqual(world.ran('alerts.email-remove'), [{}]);
});

test('a page left with a password typed empties the field', async (context) => {
  useStorage(context);
  const world = await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': [] } } });
  const card = fillEmail(world, { password: 'leaving-secret' });
  assert.equal(secretOf(card).value, 'leaving-secret');
  world.leave();
  assert.equal(secretOf(card).value, '');
});

/** Opens the webhook dialog, and gives the fields of it. */
async function openHook(world) {
  await world.press(world.card('channels'), 'Add a webhook…');
  const dialog = world.dialog();
  return {
    dialog,
    kind: field(dialog, 'kind'),
    room: field(dialog, 'room'),
    secrets: dialog.querySelectorAll('input').filter((one) => /^secret-\d+$/.test(one.id)),
    submit: async () => {
      dialog.querySelector('form').fire('submit');
      await world.emit('none');
    },
  };
}

test('Add a webhook sends the address and the token in the action and nowhere else', async (context) => {
  const kept = useStorage(context);
  const world = await openAlerts(context);
  const hook = await openHook(world);
  hook.kind.value = 'ntfy';
  hook.kind.fire('change');
  const [url, token] = hook.secrets;
  url.value = 'https://ntfy.sh/duke-nas-secret-topic';
  token.value = 'tk_secret_token';
  await hook.submit();
  assert.deepEqual(world.ran('alerts.webhook-add'), [{
    kind: 'ntfy', url: 'https://ntfy.sh/duke-nas-secret-topic', token: 'tk_secret_token', severity: 'warning', cleared: true,
  }]);
  assert.equal(url.value, '');
  assert.equal(token.value, '');
  assert.doesNotMatch(everything(world), /secret-topic|tk_secret_token/);
  assert.doesNotMatch([...kept.values()].join('\n'), /secret-topic|tk_secret_token/);
  assert.doesNotMatch(JSON.stringify(world.notes) + JSON.stringify(world.problems), /secret-topic|tk_secret_token/);
});

test('a Slack webhook has no token field, and a Matrix one asks for its room', async (context) => {
  useStorage(context);
  const world = await openAlerts(context);
  const hook = await openHook(world);
  const tokenField = hook.secrets[1].closest('.secret-field');
  hook.kind.value = 'slack';
  hook.kind.fire('change');
  assert.equal(tokenField.hidden, true);
  hook.kind.value = 'matrix';
  hook.kind.fire('change');
  assert.equal(tokenField.hidden, false);
  assert.equal(hook.room.closest('.field').hidden, false);
  hook.secrets[0].value = 'https://matrix.example.org/hook';
  hook.secrets[1].value = 'tok';
  await hook.submit();
  assert.match(text(hook.dialog), /Type the room's ID/);
  assert.deepEqual(world.ran('alerts.webhook-add'), []);
  assert.equal(hook.secrets[0].value, 'https://matrix.example.org/hook', 'a mistake elsewhere leaves what was typed');
  hook.room.value = '!abc:example.org';
  await hook.submit();
  assert.deepEqual(world.ran('alerts.webhook-add'), [{
    kind: 'matrix', url: 'https://matrix.example.org/hook', token: 'tok', room: '!abc:example.org', severity: 'warning', cleared: true,
  }]);
});

test('a webhook address that is not https is refused at its field', async (context) => {
  useStorage(context);
  const world = await openAlerts(context);
  const hook = await openHook(world);
  hook.kind.value = 'ntfy';
  hook.kind.fire('change');
  hook.secrets[0].value = 'http://ntfy.sh/topic';
  await hook.submit();
  assert.match(text(hook.dialog), /https only/);
  assert.deepEqual(world.ran('alerts.webhook-add'), []);
  hook.kind.value = 'json';
  hook.kind.fire('change');
  hook.secrets[0].value = 'http://10.0.2.2:8080/alerts';
  await hook.submit();
  assert.deepEqual(world.ran('alerts.webhook-add')[0].url, 'http://10.0.2.2:8080/alerts');
});

test('cancelling the webhook dialog empties the secrets', async (context) => {
  useStorage(context);
  const world = await openAlerts(context);
  const hook = await openHook(world);
  hook.secrets[0].value = 'https://ntfy.sh/leaving-secret';
  hook.secrets[1].value = 'leaving-token';
  buttonOf(hook.dialog, 'Cancel').click();
  await world.emit('none');
  assert.equal(hook.secrets[0].value, '');
  assert.equal(hook.secrets[1].value, '');
});

test('the webhook command shown holds empty secrets', async (context) => {
  useStorage(context);
  const world = await openAlerts(context);
  world.view.prefs.commands = 'shown';
  const hook = await openHook(world);
  hook.kind.value = 'ntfy';
  hook.secrets[0].value = 'https://ntfy.sh/shown-secret';
  hook.secrets[1].value = 'shown-token';
  hook.dialog.querySelector('form').fire('input');
  await new Promise((resolve) => setTimeout(resolve, 500));
  const panel = hook.dialog.querySelector('[data-panel]');
  assert.ok(panel);
  assert.match(panel.getAttribute('data-panel'), /"url":""/);
  assert.match(panel.getAttribute('data-panel'), /"token":""/);
  assert.doesNotMatch(everything(world), /shown-secret|shown-token/);
});
