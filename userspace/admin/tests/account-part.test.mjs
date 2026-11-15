// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The account's part of the Overview (design 3.5): an administrator with no
// second step is told to add one, from the account's own list.

import assert from 'node:assert/strict';
import test from 'node:test';

import { hasSecondStep, needsOf } from '../pages/account/needs.js';
import { needs } from '../pages/account/overview.js';
import { fakeCx, secondStep } from './overview-fixtures.mjs';

const admin = { name: 'chase', admin: true };
const reader = { name: 'dana', admin: false };

test('an administrator with no second step is told to add one', () => {
  assert.deepEqual(needsOf(admin, secondStep), [{
    id: 'account-second-step',
    severity: 'warn',
    title: 'Add a second step to your account',
    detail: 'Unlocking the console now asks for your password alone. An authenticator app or a passkey makes that safer.',
    href: '/second-step',
    icon: 'key',
    at: null,
  }]);
});

test('an app, or a passkey, is a second step', () => {
  assert.equal(hasSecondStep({ app: { added: 1 }, passkeys: [] }), true);
  assert.equal(hasSecondStep({ app: null, passkeys: [{ id: 'k' }] }), true);
  assert.equal(hasSecondStep({ app: null, passkeys: [] }), false);
  assert.equal(hasSecondStep(null), false);
  assert.deepEqual(needsOf(admin, { app: { added: 1 }, passkeys: [] }), []);
  assert.deepEqual(needsOf(admin, { app: null, passkeys: [{ id: 'k' }] }), []);
});

test('a reader is not told, and the part does not even read for one', async () => {
  assert.deepEqual(needsOf(reader, secondStep), []);
  const cx = fakeCx({ '/api/1/account/second-step': secondStep }, reader);
  assert.deepEqual(await needs(cx), []);
  assert.deepEqual(cx.calls, []);
});

test('the part reads the account\'s own list, not the session\'s kinds', async () => {
  const cx = fakeCx({ '/api/1/account/second-step': secondStep }, { ...admin, kinds: ['app', 'passkey'] });
  assert.equal((await needs(cx)).length, 1);
  assert.deepEqual(cx.calls, ['/api/1/account/second-step']);
  const has = fakeCx({ '/api/1/account/second-step': { app: { added: 1 }, passkeys: [] } }, { ...admin, kinds: [] });
  assert.deepEqual(await needs(has), []);
  await assert.rejects(needs(fakeCx({}, admin)), /no answer/);
});
