// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Private access on the Security page (design 8.8 and 9.3): the card for a
// server that has none and for one that has, Set up, Add a device with an
// answer that is a secret and is shown once, and Remove.

import assert from 'node:assert/strict';
import test from 'node:test';

import { added, LONG_ENDPOINT, SECRET, wireguard } from './security-fixtures.mjs';
import {
  buttonOf, control, fieldOf, openSecurity, refusal, settle, type,
} from './security-view.mjs';

const OK = { ok: true, result: {} };
const text = (page) => page.card('private').textContent;
const NOT_SET_UP = { ok: true, result: { interfaces: [] } };
const SET_UP = { ok: true, result: wireguard() };

const everything = (page) => [page.view.main, ...page.document.body.querySelectorAll('dialog')].flatMap((node) => [node, ...node.descendants]).flatMap((node) => [...node.attributes.values(), node.textContent]);

// ---- the card -----------------------------------------------------------------------------

test('a server without private access says what it is and offers to set it up', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'private.show': NOT_SET_UP } } });
  assert.match(text(page), /A phone or laptop with the WireGuard app joins this server's own small network, wg0, from anywhere\./);
  assert.match(text(page), /The console stays off the internet and answers only on this network and through WireGuard\./);
  assert.ok(control(page.card('private'), 'private-set-up'));
});

test('a server with private access lists its devices, when each last connected and what it sent and received', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'private.show': SET_UP } } });
  assert.match(text(page), /wg0 · 10\.8\.0\.1\/24 · UDP port 51820 · devices reach it at home\.example\.org/);
  const rows = page.card('private').querySelectorAll('tr').slice(1);
  assert.equal(rows.length, 2);
  assert.match(rows[0].textContent, /phone10\.8\.0\.2\/32Connected 2 minutes agoreceived 1\.23 KB, sent 5\.68 KBRemove/);
  assert.match(rows[1].textContent, /laptop10\.8\.0\.3\/32Never connected/);
  assert.ok(control(page.card('private'), 'private-add'));
});

test('private access with no device yet says so', async (context) => {
  const none = wireguard();
  none.interfaces[0].devices = [];
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'private.show': { ok: true, result: none } } } });
  assert.match(text(page), /No device yet\. Add one to reach this console privately\./);
});

test('the devices are read again every 10 seconds while the console is unlocked', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const before = page.ran('private.show').length;
  await page.tick(10_000);
  assert.equal(page.ran('private.show').length, before + 1);
});

test('a locked administrator is asked to unlock, a reader is told who sees it, and neither reads the devices', async (context) => {
  const locked = await openSecurity(context, { unlocked: false });
  assert.match(text(locked), /Unlock to see private access\./);
  assert.equal(locked.ran('private.show').length, 0);
  const reader = await openSecurity(context, { session: { admin: false } });
  assert.match(text(reader), /Administrators see private access\./);
  assert.equal(reader.ran('private.show').length, 0);
});

test('a server without the command says that private access comes with a later update', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'private.show': refusal('ADM-129', 'No such command', 404) } } });
  assert.match(text(page), /sets up private access with a later update of Dukovo/);
});

// ---- Set up -------------------------------------------------------------------------------

const openSetUp = async (page) => {
  control(page.card('private'), 'private-set-up').click();
  await settle();
  return page.dialog();
};

test('Set up starts on the console\'s own name, the default port and network, and tells what the router must do', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'private.show': NOT_SET_UP } } });
  const dialog = await openSetUp(page);
  assert.equal(dialog.querySelector('h2').textContent, 'Set up private access');
  assert.equal(fieldOf(dialog, 'Address devices reach this server at').value, 'duke-lab');
  assert.equal(fieldOf(dialog, 'UDP port').value, '51820');
  assert.equal(fieldOf(dialog, 'This server\'s address and network on wg0').value, '10.8.0.1/24');
  assert.match(dialog.textContent, /Your router must send UDP port 51820 to this server\./);
  type(fieldOf(dialog, 'UDP port'), '51999');
  assert.match(dialog.textContent, /Your router must send UDP port 51999 to this server\./);
});

test('Set up runs private.serve and then firewall.add for the same port, and reads the devices again', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'private.show': NOT_SET_UP, 'private.serve': OK, 'firewall.add': OK } } });
  const dialog = await openSetUp(page);
  const reads = page.ran('private.show').length;
  buttonOf(dialog, 'Set up').click();
  await settle();
  assert.deepEqual(page.allRan().filter((action) => action === 'private.serve' || action === 'firewall.add'), ['private.serve', 'firewall.add']);
  assert.deepEqual(page.ran('private.serve'), [{ endpoint: 'duke-lab' }], 'what is the default is not sent');
  assert.deepEqual(page.ran('firewall.add'), [{ action: 'allow', protocol: 'udp', ports: '51820' }]);
  assert.equal(page.dialog(), null);
  assert.equal(page.ran('private.show').length, reads + 1);
});

test('a port and a network that are not the defaults are sent, and the firewall opens that port', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'private.show': NOT_SET_UP, 'private.serve': OK, 'firewall.add': OK } } });
  const dialog = await openSetUp(page);
  type(fieldOf(dialog, 'Address devices reach this server at'), 'home.example.org');
  type(fieldOf(dialog, 'UDP port'), '51999');
  type(fieldOf(dialog, 'This server\'s address and network on wg0'), '10.9.0.1/24');
  buttonOf(dialog, 'Set up').click();
  await settle();
  assert.deepEqual(page.ran('private.serve'), [{ endpoint: 'home.example.org', port: 51_999, address: '10.9.0.1/24' }]);
  assert.deepEqual(page.ran('firewall.add'), [{ action: 'allow', protocol: 'udp', ports: '51999' }]);
});

test('a field that is wrong is said under it, and nothing is run', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'private.show': NOT_SET_UP } } });
  const dialog = await openSetUp(page);
  type(fieldOf(dialog, 'UDP port'), '80');
  type(fieldOf(dialog, 'This server\'s address and network on wg0'), '10.8.0.1');
  buttonOf(dialog, 'Set up').click();
  await settle();
  assert.match(dialog.textContent, /Write a port from 1024 to 65535\./);
  assert.match(dialog.textContent, /Write the interface's address and prefix, such as 10\.8\.0\.1\/24/);
  assert.equal(page.ran('private.serve').length, 0);
  assert.equal(page.dialog(), dialog);
});

test('when the network is made and the firewall refuses, the dialog stays and a second press makes the network again, which changes nothing', async (context) => {
  const answers = { 'private.show': NOT_SET_UP, 'private.serve': OK, 'firewall.add': refusal('NET-100', 'The rule was refused') };
  const page = await openSecurity(context, { unlocked: true, world: { answers } });
  const dialog = await openSetUp(page);
  buttonOf(dialog, 'Set up').click();
  await settle();
  assert.equal(page.dialog(), dialog);
  assert.match(dialog.querySelector('.sec-errors').textContent, /The rule was refused/);
  answers['firewall.add'] = OK;
  buttonOf(dialog, 'Set up').click();
  await settle();
  assert.equal(page.ran('private.serve').length, 2);
  assert.equal(page.ran('firewall.add').length, 2);
  assert.equal(page.dialog(), null);
});

test('a refused set-up does not open the port', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'private.show': NOT_SET_UP, 'private.serve': refusal('NET-181', 'wg0 could not be made') } } });
  buttonOf(await openSetUp(page), 'Set up').click();
  await settle();
  assert.equal(page.ran('firewall.add').length, 0);
});

test('a locked console is asked to unlock before Set up opens', async (context) => {
  const page = await openSecurity(context, { unlocked: false });
  assert.equal(control(page.card('private'), 'private-set-up'), null, 'the card is not read, so there is no button yet');
  assert.ok(buttonOf(page.card('private'), 'Unlock'));
});

// ---- Add a device --------------------------------------------------------------------------

const openAdd = async (page) => {
  control(page.card('private'), 'private-add').click();
  await settle();
  return page.dialog();
};

/** A change's answer, `{done, changed, output}`, in which `wireguard add NAME --json` printed `device` as one line. */
const addWorld = (device) => ({ answers: { 'private.show': SET_UP, 'private.add': { ok: true, result: { done: true, changed: true, output: [JSON.stringify(device)] } } } });

test('the name is one to 32 letters, digits, - and _, not starting with -, and not one that a device has', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: addWorld(added('x')) });
  const dialog = await openAdd(page);
  const rule = /Use 1 to 32 letters, digits, - and _, not starting with -\./;
  for (const [name, words] of [['', /Write a name for the device/], ['my phone', rule], ['-phone', rule], ['phone', /A device already has that name\./], ['x'.repeat(33), rule]]) {
    type(fieldOf(dialog, 'Name of the device'), name);
    buttonOf(dialog, 'Add the device').click();
    await settle();
    assert.match(dialog.textContent, words);
  }
  assert.equal(page.ran('private.add').length, 0);
});

test('Add runs private.add once with the name, and shows the settings once, with their private key', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: addWorld(added('tablet')) });
  const dialog = await openAdd(page);
  type(fieldOf(dialog, 'Name of the device'), 'tablet');
  buttonOf(dialog, 'Add the device').click();
  await settle();
  assert.deepEqual(page.ran('private.add'), [{ name: 'tablet' }]);
  const shown = page.dialog();
  assert.equal(shown.querySelector('h2').textContent, 'Settings for tablet');
  assert.match(shown.textContent, /Scan this with the WireGuard app on your phone or laptop to add it to wg0\./);
  assert.ok(shown.querySelector('svg'), 'a device\'s file, of about 240 bytes, fits a QR code');
  const asText = buttonOf(shown, 'Show the settings as text');
  assert.equal(shown.querySelector('pre').hasAttribute('hidden'), true);
  asText.click();
  assert.equal(shown.querySelector('pre').hasAttribute('hidden'), false);
  assert.match(shown.querySelector('pre').textContent, new RegExp(SECRET));
  assert.ok(buttonOf(shown, 'Save as file'));
  assert.ok(buttonOf(shown, 'Copy'));
});

test('an answer without the device\'s file shows no empty settings, and says to add the device again', async (context) => {
  const world = { answers: { 'private.show': SET_UP, 'private.add': { ok: true, result: { done: true, changed: true, output: ['tablet is added.'] } } } };
  const page = await openSecurity(context, { unlocked: true, world });
  const dialog = await openAdd(page);
  type(fieldOf(dialog, 'Name of the device'), 'tablet');
  buttonOf(dialog, 'Add the device').click();
  await settle();
  assert.equal(page.ran('private.add').length, 1);
  assert.ok(!page.dialog() || !/Settings for tablet/.test(page.dialog().textContent), 'no settings are shown');
  assert.deepEqual(page.notices, [['tablet is added, but the answer did not hold its settings. Remove tablet and add it again.', { kind: 'warn' }]]);
});

test('a file too long for a QR code is shown as text and says so', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: addWorld(added('tablet', { endpoint: LONG_ENDPOINT })) });
  const dialog = await openAdd(page);
  type(fieldOf(dialog, 'Name of the device'), 'tablet');
  buttonOf(dialog, 'Add the device').click();
  await settle();
  const shown = page.dialog();
  assert.equal(shown.querySelector('svg'), null);
  assert.match(shown.textContent, /It is too long for a QR code\. Copy it or save it as a file\./);
  assert.equal(shown.querySelector('pre').hasAttribute('hidden'), false);
});

test('Add says what a device reaches, and asks only its name', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: addWorld(added('tablet')) });
  const dialog = await openAdd(page);
  assert.match(dialog.textContent, /The device reaches this server through wg0: the console and the server's other services, not the other machines of its networks\./);
  assert.equal(dialog.querySelectorAll('input').length, 1);
  assert.equal(dialog.querySelectorAll('input[type="radio"]').length, 0);
});

test('the key is gone from the page when the person has kept it, and the page asks for it once', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: addWorld(added('tablet')) });
  const dialog = await openAdd(page);
  type(fieldOf(dialog, 'Name of the device'), 'tablet');
  buttonOf(dialog, 'Add the device').click();
  await settle();
  assert.ok(everything(page).some((held) => held.includes(SECRET)), 'it is on the page while it is shown');
  assert.equal(page.view.main.textContent.includes(SECRET), false, 'and nowhere but in its dialog');
  buttonOf(page.dialog(), 'I have kept it').click();
  await settle();
  assert.equal(page.dialog(), null);
  assert.equal(everything(page).some((held) => held.includes(SECRET)), false);
  assert.equal(page.ran('private.add').length, 1);
  assert.equal(page.panels.some(([, args]) => JSON.stringify(args).includes(SECRET)), false);
});

test('Escape on the settings asks first, and going back keeps them', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: addWorld(added('tablet')) });
  const dialog = await openAdd(page);
  type(fieldOf(dialog, 'Name of the device'), 'tablet');
  buttonOf(dialog, 'Add the device').click();
  await settle();
  const shown = page.dialog();
  shown.fire('cancel');
  await settle();
  const question = page.dialog();
  assert.equal(question.querySelector('h2').textContent, 'Close without keeping it?');
  assert.match(question.textContent, /The device's private key is shown only now/);
  buttonOf(question, 'Go back').click();
  await settle();
  assert.equal(page.dialog(), shown);
});

test('the devices are read again when one is added', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: addWorld(added('tablet')) });
  const before = page.ran('private.show').length;
  const dialog = await openAdd(page);
  type(fieldOf(dialog, 'Name of the device'), 'tablet');
  buttonOf(dialog, 'Add the device').click();
  await settle();
  assert.equal(page.ran('private.show').length, before + 1);
});

test('a refused add stays in the dialog, with the refusal, and shows no settings', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'private.show': SET_UP, 'private.add': refusal('NET-185', 'A device already has that name') } } });
  const dialog = await openAdd(page);
  type(fieldOf(dialog, 'Name of the device'), 'tablet');
  buttonOf(dialog, 'Add the device').click();
  await settle();
  assert.equal(page.dialog(), dialog);
  assert.match(dialog.querySelector('.sec-errors').textContent, /A device already has that name/);
});

// ---- Remove --------------------------------------------------------------------------------

const removeButton = (page, name) => page.card('private').querySelectorAll('button').find((one) => one.getAttribute('aria-label') === `Remove ${name}`);

test('Remove asks first, and Keep it removes nothing', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'private.show': SET_UP } } });
  removeButton(page, 'laptop').click();
  await settle();
  const question = page.dialog();
  assert.equal(question.querySelector('h2').textContent, 'Remove laptop?');
  assert.match(question.textContent, /laptop can no longer reach this server through wg0\./);
  buttonOf(question, 'Keep it').click();
  await settle();
  assert.equal(page.ran('private.remove').length, 0);
});

test('Remove runs private.remove for the device and reads the devices again', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'private.show': SET_UP, 'private.remove': OK } } });
  const before = page.ran('private.show').length;
  removeButton(page, 'laptop').click();
  await settle();
  buttonOf(page.dialog(), 'Remove laptop').click();
  await settle();
  const [call] = page.ranWith('private.remove');
  assert.deepEqual(call[1], { name: 'laptop' });
  assert.equal(call[2].words.done, 'laptop is removed.');
  assert.equal(page.ran('private.show').length, before + 1);
});

test('the device this browser comes through is refused (ADM-137), and the card says where to go', async (context) => {
  const refused = refusal('ADM-137', 'This browser connects through phone', 409);
  const page = await openSecurity(context, { unlocked: true, session: { from: '10.8.0.2' }, world: { answers: { 'private.show': SET_UP, 'private.remove': refused } } });
  removeButton(page, 'phone').click();
  await settle();
  buttonOf(page.dialog(), 'Remove phone').click();
  await settle();
  const errors = page.card('private').querySelector('.sec-errors');
  assert.match(errors.textContent, /This browser connects through phone/);
  assert.match(errors.textContent, /This browser is connected through that device\. Open the console over this server's own network, or from another device, and remove it there\./);
});

test('a reader has no Remove and no Add', async (context) => {
  const page = await openSecurity(context, { session: { admin: false } });
  assert.equal(removeButton(page, 'phone'), undefined);
  assert.equal(control(page.card('private'), 'private-add'), null);
});
