// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The header's access control in unlock.js while the console is locked: an
// administrator is offered Unlock, and anyone else reads only that their
// access is limited.

import assert from 'node:assert/strict';
import test from 'node:test';

import { usePage } from './fake-dom.mjs';
import { accessControl } from '../unlock.js';

test('a person who is no administrator reads Limited access, with no button and nothing more', (context) => {
  usePage(context);
  const control = accessControl({ admin: false });
  const access = control.querySelector('.access');
  assert.equal(access.textContent, 'Limited access');
  assert.equal(control.querySelector('button'), null);
});

test('a locked administrator reads Limited access and is offered Unlock', (context) => {
  usePage(context);
  const control = accessControl({ admin: true });
  const access = control.querySelector('.access');
  assert.equal(access.textContent, 'Limited access ·Unlock');
  assert.equal(access.querySelector('button').textContent, 'Unlock');
});
