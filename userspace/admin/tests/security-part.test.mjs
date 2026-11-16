// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Security page gives Finish setting up and /setup (design 8.11):
// the steps "Sign in over SSH with a key" and "Reach this console from
// anywhere, privately", and no need of its own, since the Health card shows
// the checks.

import assert from 'node:assert/strict';
import test from 'node:test';

import { needs, steps } from '../pages/security/overview.js';
import { keyLine } from './accounts-fixtures.mjs';
import { fakeCx } from './overview-fixtures.mjs';
import { usePage } from './fake-dom.mjs';
import { ssh, wireguard } from './security-fixtures.mjs';
import { settle, usePageWithProperties } from './security-view.mjs';

const step = (id) => steps.find((one) => one.id === id);
const withKeys = { account: { name: 'chase' }, keys: [{ name: 'laptop' }] };
const without = { account: { name: 'chase' }, keys: [] };

test('the part has no need of its own, and exports the steps in the order of Finish setting up', async () => {
  assert.deepEqual(await needs(fakeCx({})), []);
  assert.deepEqual(steps.map((one) => [one.id, one.order, one.points]), [['ssh-keys', 20, 0], ['private', 40, 0]]);
  assert.deepEqual(steps.map((one) => one.action), ['account.key-add', 'private.serve']);
});

// ---- Sign in over SSH with a key -------------------------------------------------------------

test('the key step is offered where SSH is on, and not where it is off or cannot be read', async () => {
  const key = step('ssh-keys');
  assert.equal(await key.available(fakeCx({ '/api/1/ssh': ssh() })), true);
  assert.equal(await key.available(fakeCx({ '/api/1/ssh': ssh({ on: false }) })), false);
  assert.equal(await key.available(fakeCx({})), false);
});

test('the key step is done once the person\'s own account has a key', async () => {
  const key = step('ssh-keys');
  assert.equal(await key.done(fakeCx({ '/api/1/account/show': withKeys })), true);
  assert.equal(await key.summary(fakeCx({ '/api/1/account/show': withKeys })), 'Your key is in');
  assert.equal(await key.done(fakeCx({ '/api/1/account/show': without })), false);
  assert.equal(await key.summary(fakeCx({ '/api/1/account/show': without })), 'No key yet');
  assert.equal(await key.done(fakeCx({})), false, 'an account that cannot be read is not done');
  assert.equal(await key.summary(fakeCx({})), 'No key yet');
});

test('the key form takes the public key line, refuses what is not one, and points to Keys only afterwards', async (context) => {
  usePage(context);
  const form = step('ssh-keys').form({});
  const input = form.element.querySelector('textarea');
  input.value = '   ';
  assert.throws(() => form.args(), (error) => /Paste the key/.test(error.words));
  input.value = 'ssh-rsa AAAAB3NzaC1yc2E laptop';
  assert.throws(() => form.args(), (error) => /Ed25519 and ECDSA/.test(error.words));
  input.value = `${keyLine()}\n`;
  assert.deepEqual(form.args(), { line: keyLine() });
  const next = form.element.querySelector('p[role="status"]');
  assert.equal(next.hasAttribute('hidden'), true);
  await form.after();
  assert.equal(input.value, '', 'the line is not kept');
  assert.equal(next.hasAttribute('hidden'), false);
  assert.equal(next.querySelector('a').getAttribute('href'), '/security#ssh');
});

// ---- Reach this console from anywhere, privately ------------------------------------------------

test('private access is offered to every machine, and is done once a device is on it', async () => {
  const priv = step('private');
  assert.equal(await priv.available(fakeCx({})), true);
  assert.equal(await priv.done(fakeCx({ '/api/1/private': wireguard() })), true);
  assert.equal(await priv.summary(fakeCx({ '/api/1/private': wireguard() })), 'A device is connected');
  const empty = wireguard();
  empty.interfaces[0].devices = [];
  assert.equal(await priv.done(fakeCx({ '/api/1/private': empty })), false);
  assert.equal(await priv.summary(fakeCx({ '/api/1/private': empty })), 'Set up, with no device yet');
  assert.equal(await priv.summary(fakeCx({ '/api/1/private': { interfaces: [] } })), 'Not set up');
  assert.equal(await priv.done(fakeCx({})), false, 'a server that does not answer is not done');
  assert.equal(await priv.summary(fakeCx({})), 'Not set up');
});

/** The step's form on a page whose view is `view`, once its fields are drawn. */
async function privateForm(context, view) {
  usePageWithProperties(context);
  const form = step('private').form(view);
  assert.equal(form.element.getAttribute('aria-busy'), 'true', 'the fields are read when the step is shown');
  assert.throws(() => form.args(), /not drawn yet/);
  await settle();
  return form;
}

test('the private form has the set-up fields of the page, starting on the console\'s own name', async (context) => {
  const form = await privateForm(context, { host: 'duke-lab' });
  assert.equal(form.element.hasAttribute('aria-busy'), false);
  const inputs = form.element.querySelectorAll('input');
  assert.deepEqual(inputs.map((one) => one.getAttribute('value')), ['duke-lab', '51820', '10.8.0.1/24']);
  assert.match(form.element.textContent, /Your router must send UDP port 51820 to this server\./);
  assert.deepEqual(form.args(), { endpoint: 'duke-lab' });
});

test('a field that is wrong stops the step with its words', async (context) => {
  const form = await privateForm(context, { host: 'duke-lab' });
  const port = form.element.querySelectorAll('input')[1];
  port.value = '22';
  assert.throws(() => form.args(), (error) => /Write a port from 1024 to 65535/.test(error.words));
});

test('after the network is made, the port is opened and the person goes to the page where a device is added', async (context) => {
  const ran = [];
  const went = [];
  const view = {
    host: 'duke-lab', api: { run: async (action, args, options) => { ran.push([action, args, options]); return { ok: true, result: {} }; } }, go: (path) => went.push(path),
  };
  const form = await privateForm(context, view);
  form.element.querySelectorAll('input')[1].value = '51999';
  await form.after();
  assert.deepEqual(ran.map(([action, args]) => [action, args]), [['firewall.add', { action: 'allow', protocol: 'udp', ports: '51999' }]]);
  assert.equal(ran[0][2].words.done, 'WireGuard\'s port is open.');
  assert.deepEqual(went, ['/security#private']);
});
