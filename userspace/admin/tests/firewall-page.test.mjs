// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Firewall page on a fake document: the form that adds a rule, which the
// page opens by itself when the address says #add, and which a person may
// open and fill in before the first read of the rules has answered.

import assert from 'node:assert/strict';
import test, { afterEach, beforeEach } from 'node:test';

import { listed, openFirewall, settle } from './firewall-view.mjs';

const realObserver = globalThis.MutationObserver;
beforeEach(() => {
  globalThis.MutationObserver = class { observe() {} disconnect() {} };
});
afterEach(() => {
  if (realObserver === undefined) delete globalThis.MutationObserver;
  else globalThis.MutationObserver = realObserver;
});

test('a form opened before the first read of the rules answers keeps what was typed in it', async (context) => {
  let answer;
  const answered = new Promise((resolve) => { answer = resolve; });
  const world = await openFirewall(context, { reads: { '/api/1/firewall': () => answered } });
  await world.press('fw-add-open');
  await world.type('fw-ports', '8090');
  assert.equal(world.form().hidden, false);
  assert.equal(world.location.hash, '#add');

  answer(listed());
  await world.ready;
  await settle();
  assert.equal(world.form().hidden, false, 'the form is still shown');
  assert.equal(world.field('fw-ports').value, '8090');
});

test('an address that says #add opens the form once the rules are read', async (context) => {
  const world = await openFirewall(context, { hash: '#add' });
  await world.ready;
  assert.equal(world.form().hidden, false);
  assert.equal(world.field('fw-ports').value, '');
  assert.equal(world.location.hash, '#add');
});
