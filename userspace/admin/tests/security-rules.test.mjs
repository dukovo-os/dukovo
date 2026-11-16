// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The sign-in rules and the blocked addresses of the Security page (design
// 8.4, 8.5 and 8.9, 22.7): what each card says, the form that changes the
// rules and the dialog that lists the networks that are never blocked.

import assert from 'node:assert/strict';
import test from 'node:test';

import { blocked, settings } from './security-fixtures.mjs';
import {
  buttonOf, choose, control, fieldOf, openSecurity, refusal, settle, type,
} from './security-view.mjs';

const text = (page, id) => page.card(id).textContent;

// ---- the sign-in rules --------------------------------------------------------------

test('the sign-in card says who gives a second step and how long a session lasts, and whose rules these are', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  assert.match(text(page, 'sign-in'), /Who gives a second step: administrators, and anyone who has added one\./);
  assert.match(text(page, 'sign-in'), /A session lasts 12 hours after its sign-in, and ends after 30 minutes with no console page open\./);
  assert.match(text(page, 'sign-in'), /SSH signs in by its own settings, below\./);
  assert.equal(page.card('sign-in').hasAttribute('aria-busy'), false);
});

test('a reader reads the rules and is given no button to change them', async (context) => {
  const page = await openSecurity(context, { session: { admin: false } });
  assert.match(text(page, 'sign-in'), /Who gives a second step/);
  assert.equal(control(page.card('sign-in'), 'sign-in-change').hasAttribute('hidden'), true);
});

test('a read of the rules that fails says so on the card, and keeps the problem under its title', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/security/sign-in': refusal('ADM-135', 'admind could not answer', 500) } } });
  assert.match(text(page, 'sign-in'), /The sign-in rules could not be read/);
  assert.equal(page.problems[0][1].slot, page.card('sign-in').querySelector('.sec-errors'));
});

test('the form starts on the rules now, sends only what changed, and reads the rules again', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  control(page.card('sign-in'), 'sign-in-change').click();
  await settle();
  const dialog = page.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Change the sign-in rules');
  const save = buttonOf(dialog, 'Save the rules');
  assert.equal(save.hasAttribute('disabled'), true, 'nothing has changed yet');
  type(fieldOf(dialog, 'Hours a session lasts after its sign-in'), '8');
  assert.equal(save.hasAttribute('disabled'), false);
  assert.match(dialog.textContent, /Sessions older than 8 hours end within 30 seconds/);
  const reads = page.calls.filter((path) => path === '/api/1/security/sign-in').length;
  save.click();
  await settle();
  const [call] = page.ranWith('security.sign-in-set');
  assert.deepEqual(call[1], { session_hours: 8 });
  assert.equal(call[2].words.done, 'The sign-in rules are changed.');
  assert.equal(page.dialog(), null, 'the dialog closes when the change is made');
  assert.equal(page.calls.filter((path) => path === '/api/1/security/sign-in').length, reads + 1);
});

test('a stronger second step says that accounts without one add one, and nobody is locked out', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  control(page.card('sign-in'), 'sign-in-change').click();
  await settle();
  const dialog = page.dialog();
  choose(dialog, 'everyone');
  assert.match(dialog.textContent, /Accounts that have none add one at their next sign-in, before anything else\. Nobody is locked out\./);
  buttonOf(dialog, 'Save the rules').click();
  await settle();
  assert.deepEqual(page.ran('security.sign-in-set'), [{ second_step: 'everyone' }]);
});

test('a number out of bounds is refused where it is typed, and nothing is sent', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  control(page.card('sign-in'), 'sign-in-change').click();
  await settle();
  const dialog = page.dialog();
  const minutes = fieldOf(dialog, 'Minutes with no console page open before a session ends');
  type(minutes, '2');
  // Said as it is typed, in the form's status line, with the focus left where it is.
  assert.match(dialog.querySelector('.sec-follows').textContent, /Write a whole number of minutes from 5 to 1440\./);
  assert.equal(minutes.getAttribute('aria-invalid'), 'true');
  assert.equal(buttonOf(dialog, 'Save the rules').hasAttribute('disabled'), true);
  buttonOf(dialog, 'Save the rules').click();
  await settle();
  assert.equal(page.ran('security.sign-in-set').length, 0);
  assert.equal(page.dialog(), dialog);
  // A number in bounds takes the problem away.
  type(minutes, '45');
  assert.doesNotMatch(dialog.querySelector('.sec-follows').textContent, /Write a whole number/);
  assert.equal(minutes.hasAttribute('aria-invalid'), false);
  assert.equal(buttonOf(dialog, 'Save the rules').hasAttribute('disabled'), false);
});

test('a refusal is shown in the dialog, which stays open', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'security.sign-in-set': refusal('ADM-140', 'The rules were refused') } } });
  control(page.card('sign-in'), 'sign-in-change').click();
  await settle();
  const dialog = page.dialog();
  type(fieldOf(dialog, 'Hours a session lasts after its sign-in'), '24');
  buttonOf(dialog, 'Save the rules').click();
  await settle();
  assert.equal(page.dialog(), dialog);
  assert.match(dialog.querySelector('.sec-errors').textContent, /The rules were refused/);
});

test('a locked console asks for the unlock before the form opens', async (context) => {
  const page = await openSecurity(context, { unlocked: false });
  assert.equal(page.dialog(), null);
  control(page.card('sign-in'), 'sign-in-change').click();
  await settle();
  assert.match(page.dialog().textContent, /change the sign-in rules/);
  assert.equal(page.dialog().querySelector('h2').textContent === 'Change the sign-in rules', false);
});

// ---- the blocked addresses -------------------------------------------------------------

test('the blocked card lists each address with why, until when, and Unblock, and says the rule in words', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const card = page.card('blocked');
  const rows = card.querySelectorAll('tr').slice(1);
  assert.equal(rows.length, 2);
  assert.match(rows[0].textContent, /203\.0\.113\.41outside3 wrong passwords over SSH in 2 minutes, for admin11:19 AMUnblock/);
  assert.match(text(page, 'blocked'), /Never blocked: 192\.168\.1\.0\/24 and 10\.8\.0\.0\/24/);
  assert.match(text(page, 'blocked'), /5 wrong tries in 10 minutes block an address for 10 minutes; each new block of it doubles, up to a day\./);
  assert.match(text(page, 'blocked'), /An administrator's own address is never blocked while they are signed in\./);
});

test('no block says so', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'security.blocked': { ok: true, result: blocked({ blocks: [] }) } } } });
  assert.match(text(page, 'blocked'), /No address is blocked now\./);
  assert.equal(page.card('blocked').querySelectorAll('table').length, 0);
});

test('blocking that is off is a warning that names the file and the line', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'security.blocked': { ok: true, result: blocked({ on: false, blocks: [] }) } } } });
  assert.match(text(page, 'blocked'), /Blocking is off\. Remove the line on = false from \/system\/settings\/blocking\.toml/);
});

test('Unblock runs security.unblock for the address as the firewall keeps it, and reads the blocks again', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const before = page.ran('security.blocked').length;
  buttonOf(page.card('blocked'), 'Unblock').click();
  await settle();
  const [call] = page.ranWith('security.unblock');
  assert.deepEqual(call[1], { address: '203.0.113.41/32' });
  assert.equal(call[2].words.done, '203.0.113.41 is unblocked.');
  assert.equal(page.ran('security.blocked').length, before + 1);
});

test('the blocks are read again every 30 seconds while the console is unlocked', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const before = page.ran('security.blocked').length;
  await page.tick(30_000);
  assert.equal(page.ran('security.blocked').length, before + 1);
});

test('a locked administrator is asked to unlock and no block is read; a reader is told who sees them', async (context) => {
  const locked = await openSecurity(context, { unlocked: false });
  assert.match(text(locked, 'blocked'), /Unlock to see blocked addresses\./);
  assert.equal(locked.ran('security.blocked').length, 0);
  assert.ok(buttonOf(locked.card('blocked'), 'Unlock'));
  const reader = await openSecurity(context, { session: { admin: false } });
  assert.match(text(reader, 'blocked'), /Administrators see blocked addresses\./);
  assert.equal(reader.ran('security.blocked').length, 0);
  assert.equal(buttonOf(reader.card('blocked'), 'Unlock'), undefined);
});

test('a server without the firewall command says that blocks come with a later update', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'security.blocked': refusal('ADM-129', 'No such command', 404) } } });
  assert.match(text(page, 'blocked'), /shows blocked addresses with a later update of Dukovo/);
  assert.equal(page.card('blocked').querySelector('.sec-errors').textContent, '');
});

test('a read of the blocks that fails says so and keeps the problem under the title', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'security.blocked': refusal('ADM-135', 'admind could not answer', 500) } } });
  assert.match(text(page, 'blocked'), /The blocked addresses could not be read\./);
});

// ---- the never-block list (22.7) --------------------------------------------------------

const openNever = async (page) => {
  buttonOf(page.card('blocked'), 'change').click();
  await settle();
  return page.dialog();
};

test('the never-block dialog lists the networks and removes one', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const dialog = await openNever(page);
  assert.equal(dialog.querySelector('h2').textContent, 'Never blocked');
  assert.match(dialog.textContent, /192\.168\.1\.0\/24/);
  dialog.querySelectorAll('button').find((one) => one.getAttribute('aria-label') === 'Remove 10.8.0.0/24').click();
  await settle();
  assert.deepEqual(page.ran('security.never-block-remove'), [{ network: '10.8.0.0/24' }]);
});

test('a private network is added without a question', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const dialog = await openNever(page);
  type(fieldOf(dialog, 'Add a network'), '192.168.7.0/24');
  buttonOf(dialog, 'Add to the list').click();
  await settle();
  assert.deepEqual(page.ran('security.never-block'), [{ network: '192.168.7.0/24' }]);
  assert.equal(page.dialog(), dialog, 'the dialog stays, to add another');
  assert.equal(fieldOf(dialog, 'Add a network').value, '');
});

test('a network outside the private ranges is asked about first, and nothing is added until the person says so (22.7)', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const dialog = await openNever(page);
  type(fieldOf(dialog, 'Add a network'), '203.0.113.0/24');
  buttonOf(dialog, 'Add to the list').click();
  await settle();
  const question = page.dialog();
  assert.notEqual(question, dialog);
  assert.equal(question.querySelector('h2').textContent, 'Never block this network?');
  assert.match(question.textContent, /Addresses in 203\.0\.113\.0\/24 are never blocked, however often they get a password wrong\./);
  assert.equal(page.ran('security.never-block').length, 0);
  buttonOf(question, 'Go back').click();
  await settle();
  assert.equal(page.ran('security.never-block').length, 0);
  buttonOf(dialog, 'Add to the list').click();
  await settle();
  buttonOf(page.dialog(), 'Never block it').click();
  await settle();
  assert.deepEqual(page.ran('security.never-block'), [{ network: '203.0.113.0/24' }]);
});

test('a private network wider than a /16 is asked about too', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const dialog = await openNever(page);
  type(fieldOf(dialog, 'Add a network'), '10.0.0.0/8');
  buttonOf(dialog, 'Add to the list').click();
  await settle();
  assert.equal(page.dialog().querySelector('h2').textContent, 'Never block this network?');
});

test('text that is no network is refused under the field', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const dialog = await openNever(page);
  type(fieldOf(dialog, 'Add a network'), 'home');
  buttonOf(dialog, 'Add to the list').click();
  await settle();
  assert.match(dialog.textContent, /Write an address or a network, such as 192\.168\.1\.0\/24\./);
  assert.equal(page.ran('security.never-block').length, 0);
});

test('the dialog reads the list again after a change and draws what the server says', async (context) => {
  const answers = { 'security.blocked': { ok: true, result: blocked({ never: ['192.168.1.0/24'] }) } };
  const page = await openSecurity(context, { unlocked: true, world: { answers } });
  const dialog = await openNever(page);
  answers['security.blocked'] = { ok: true, result: blocked({ never: [] }) };
  dialog.querySelectorAll('button').find((one) => one.getAttribute('aria-label') === 'Remove 192.168.1.0/24').click();
  await settle();
  assert.match(dialog.textContent, /No network is on the list\./);
  assert.match(text(page, 'blocked'), /Never blocked: no network/);
});

test('settings are read as admind gives them: a sign-in answer without rules shows the defaults', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/security/sign-in': settings({ sign_in: {} }) } } });
  assert.match(text(page, 'sign-in'), /A session lasts 12 hours/);
});
