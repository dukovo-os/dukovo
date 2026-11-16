// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Add an account… (design 7.4) and the secret it takes (3.6, ADR 0561): the
// name and full name checked before the command is asked, the console right
// that follows the account, the key that is offered after it, and that the
// password is in the one call that makes the account and in no other place
// of the page: not the Show command panel, not an attribute, not a text.
// Which of the action's answers are real waits on the registry's accounts
// actions; the page is coded against design 13.2.

import assert from 'node:assert/strict';
import test from 'node:test';

import { keyLine } from './accounts-fixtures.mjs';
import {
  buttonOf, holdings, openAccounts, refusal, settle,
} from './accounts-view.mjs';

const SECRET = 'hunter2-SECRET-4471';

/** Opens the list as an unlocked administrator and presses Add an account…; gives the app and the dialog's inputs. */
async function openDialog(context, more = {}) {
  const app = await openAccounts(context, { unlocked: true, ...more });
  app.view.headed.buttons[0].click();
  await settle();
  const inputs = app.dialog().querySelectorAll('input');
  const [name, full, admin, signIn, password, again] = inputs;
  return {
    app, name, full, admin, signIn, password, again,
  };
}

/** Types `text` in `input` as the browser would. */
function type(input, text) {
  input.value = text;
  input.fire('input');
}

test('Add an account asks for a name, a full name, two switches and a password typed twice, with the console on', async (context) => {
  const { app, signIn, password } = await openDialog(context);
  const labels = app.dialog().querySelectorAll('label').map((one) => one.textContent);
  assert.deepEqual(labels.slice(0, 5), ['Name', 'Full name', 'Administrator', 'Can sign in to this console', 'Password']);
  assert.ok(labels.includes('Password, again'));
  assert.equal(signIn.getAttribute('role'), 'switch');
  assert.equal(signIn.hasAttribute('checked'), true, 'the console is on to begin with');
  assert.equal(password.getAttribute('type'), 'password');
  assert.ok(app.dialog().textContent.includes('Leave it empty to give one later; the account cannot sign in until it has one or an SSH key.'));
  assert.equal(app.dialog().getAttribute('aria-labelledby') !== null, true);
});

test('a name that cannot be one is told under the field and nothing is asked', async (context) => {
  const { app, name } = await openDialog(context);
  type(name, 'Sam');
  await app.submit();
  assert.ok(app.dialog().textContent.includes('Use lowercase letters, digits, _, - and ., starting with a letter.'));
  assert.equal(app.ran('accounts.add').length, 0);
  type(name, 'chase');
  await app.submit();
  assert.ok(app.dialog().textContent.includes('An account is already called chase.'));
  type(name, 'help');
  await app.submit();
  assert.ok(app.dialog().textContent.includes('kept for the helpers'));
  assert.equal(app.ran('accounts.add').length, 0);
});

test('a full name with a colon is refused before the command is', async (context) => {
  const { app, name, full } = await openDialog(context);
  type(name, 'sam2');
  type(full, 'Sam: Two');
  await app.submit();
  assert.ok(app.dialog().textContent.includes('A full name cannot hold a colon or a line break.'));
  assert.equal(app.ran('accounts.add').length, 0);
});

test('an account is made with what was typed, given the console it asked for, and the dialog offers a key for it', async (context) => {
  const { app, name, full, signIn, password, again } = await openDialog(context);
  type(name, 'sam2');
  type(full, 'Sam Two');
  signIn.checked = true;
  type(password, SECRET);
  type(again, SECRET);
  await app.submit();
  assert.deepEqual(app.ran('accounts.add'), [{ name: 'sam2', full_name: 'Sam Two', password: SECRET }]);
  assert.deepEqual(app.ran('accounts.set'), [{ name: 'sam2', console: true }]);
  assert.equal(app.dialog().querySelector('h2').textContent, 'Add an SSH key for sam2');
  assert.ok(app.dialog().textContent.includes('sam2 is added.'));
  buttonOf(app.dialog(), 'Not now').click();
  await settle();
  assert.deepEqual(app.went, ['/accounts/sam2'], 'the new account\'s page opens after the key is dealt with');
});

test('the key that the dialog offers is added for the new account, checked as authd reads it', async (context) => {
  const { app, name, password, again } = await openDialog(context);
  type(name, 'sam2');
  type(password, SECRET);
  type(again, SECRET);
  await app.submit();
  const line = app.dialog().querySelector('input');
  type(line, 'ssh-rsa AAAAB3');
  await app.submit();
  assert.equal(app.ran('accounts.key-add').length, 0);
  assert.ok(app.dialog().textContent.includes('not ssh-rsa'));
  const good = keyLine();
  type(line, good);
  await app.submit();
  assert.deepEqual(app.ran('accounts.key-add'), [{ name: 'sam2', line: good }]);
  assert.equal(app.dialog(), null);
  assert.deepEqual(app.went, ['/accounts/sam2']);
});

test('an administrator is not given a console switch to turn off: it stays on and is not asked for', async (context) => {
  const { app, name, admin, signIn } = await openDialog(context);
  type(name, 'root2');
  admin.checked = true;
  admin.fire('change');
  assert.equal(signIn.checked, true);
  assert.equal(signIn.disabled, true);
  await app.submit();
  assert.deepEqual(app.ran('accounts.add'), [{ name: 'root2', admin: true }]);
  assert.equal(app.ran('accounts.set').length, 0, 'an administrator always can sign in to the console');
  admin.checked = false;
  admin.fire('change');
  assert.equal(signIn.disabled, false);
});

test('a password left empty is not sent, and an account made without one is told so', async (context) => {
  const { app, name } = await openDialog(context);
  type(name, 'sam2');
  await app.submit();
  assert.deepEqual(app.ran('accounts.add'), [{ name: 'sam2' }]);
});

test('two passwords that differ ask nothing, and keep what was typed for the person to put right', async (context) => {
  const { app, name, password, again } = await openDialog(context);
  type(name, 'sam2');
  type(password, SECRET);
  type(again, 'something else');
  await app.submit();
  assert.equal(app.ran('accounts.add').length, 0);
  assert.ok(app.dialog().textContent.includes('The two are not the same'));
  assert.equal(password.value, SECRET);
});

test('an account that is refused leaves the dialog open with the refusal in it, and the typed password gone from the page', async (context) => {
  const refused = refusal('ACC-105', 'An account is already called sam2', 409);
  const { app, name, password, again } = await openDialog(context, { world: { answers: { 'accounts.add': refused } } });
  type(name, 'sam2');
  type(password, SECRET);
  type(again, SECRET);
  await app.submit();
  assert.ok(app.dialog(), 'the dialog stays');
  assert.equal(app.problems.length, 1);
  assert.equal(password.value, '');
  assert.equal(again.value, '');
  assert.equal(app.ran('accounts.set').length, 0);
  assert.equal(holdings(app.document.body).filter((one) => one.includes(SECRET)).length, 0);
});

test('the password is in the arguments of the call that makes the account, and nowhere in the page, the command shown or a read', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const { app, name, password, again } = await openDialog(context, { prefs: { commands: 'shown' } });
  type(name, 'sam2');
  type(password, SECRET);
  type(again, SECRET);
  app.dialog().querySelector('form').fire('input');
  context.mock.timers.tick(400);
  assert.deepEqual(app.panels.at(-1), ['accounts.add', { name: 'sam2', password: '' }], 'the command says that a password is typed, and carries none');
  assert.equal(holdings(app.document.body).filter((one) => one.includes(SECRET)).length, 0, 'not an attribute, not a text');
  await app.submit();
  const sent = JSON.stringify(app.ran('accounts.add'));
  assert.equal(sent.split(SECRET).length - 1, 1, 'once, in the arguments of the one call');
  const elsewhere = [app.panels, app.calls, app.posts, app.problems.map((one) => one[0].message), app.notices, app.went, app.ran('accounts.set')];
  assert.equal(JSON.stringify(elsewhere).includes(SECRET), false);
  assert.equal(password.value, '', 'the fields are empty once it is taken');
  assert.equal(JSON.stringify(app.view.prefs).includes(SECRET), false);
});

test('the command panel follows the typing once the person pauses, and is empty until the name is one', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const { app, name, full, admin } = await openDialog(context, { prefs: { commands: 'shown' } });
  assert.equal(app.panels.length, 0, 'no name, no command');
  type(name, 'sam2');
  type(full, 'Sam Two');
  admin.checked = true;
  app.dialog().querySelector('form').fire('input');
  assert.equal(app.panels.length, 0, 'the server is not asked at each key');
  context.mock.timers.tick(400);
  assert.deepEqual(app.panels, [['accounts.add', { name: 'sam2', full_name: 'Sam Two', admin: true }]]);
});

test('with commands hidden no panel is drawn', async (context) => {
  const { app, name } = await openDialog(context);
  type(name, 'sam2');
  assert.equal(app.dialog().querySelectorAll('.acc-command').length, 0);
});

test('Cancel closes the dialog and asks nothing', async (context) => {
  const { app, name } = await openDialog(context);
  type(name, 'sam2');
  buttonOf(app.dialog(), 'Cancel').click();
  await settle();
  assert.equal(app.dialog(), null);
  assert.equal(app.ran('accounts.add').length, 0);
  assert.deepEqual(app.went, []);
});

test('leaving the page closes a dialog that is open', async (context) => {
  const { app } = await openDialog(context);
  assert.ok(app.dialog());
  app.leave();
  await settle();
  assert.equal(app.dialog(), null);
});
