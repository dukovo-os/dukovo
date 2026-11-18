// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The question before Lock (design 13.4): administrator shells end with the
// unlock, so a console that has some says so first, and one that has none
// locks without a question.

import assert from 'node:assert/strict';
import test from 'node:test';

import { adminShells, lockOk, lockTitle, signOutOk } from '../shell/dialogs.js';

const terminal = (...admins) => ({ id: 1, panes: admins.map((admin, id) => ({ id, admin })) });

test('administrator shells are counted over every terminal in the list', () => {
  assert.equal(adminShells({ terminals: [terminal(false, true), terminal(true, true, false)] }), 3);
  assert.equal(adminShells({ terminals: [terminal(false)] }), 0);
  assert.equal(adminShells({ terminals: [] }), 0);
  assert.equal(adminShells([terminal(true)]), 1, 'a bare list is read too');
  assert.equal(adminShells({ terminals: [{ id: 1 }] }), 0, 'a terminal without panes has none');
  assert.equal(adminShells(null), 0);
  assert.equal(adminShells({}), 0);
});

test('the question names the number of shells', () => {
  assert.equal(lockTitle(1), 'Lock and end 1 administrator shell?');
  assert.equal(lockTitle(2), 'Lock and end 2 administrator shells?');
});

test('Lock goes on without a question when there are no administrator shells or admind cannot say', async () => {
  assert.equal(await lockOk(async () => ({ terminals: [terminal(false)] })), true);
  assert.equal(await lockOk(async () => ({ terminals: [] })), true);
  assert.equal(await lockOk(async () => { throw new Error('ADM-104'); }), true);
});

test('Sign out and Lock do not wait long on a server that does not say which terminals run', async () => {
  const never = () => new Promise(() => {});
  assert.equal(await signOutOk({ get: never, post: async () => null }, 20), true);
  assert.equal(await lockOk(never, 20), true);
  const refused = async () => { throw new Error('no such route'); };
  assert.equal(await signOutOk({ get: refused, post: async () => null }, 20), true);
  assert.equal(await lockOk(refused, 20), true);
});
