// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// One disk's panel of the Storage page on a fake document: where its links to
// the Hardware page lead.

import assert from 'node:assert/strict';
import test from 'node:test';

import { makeTime } from '../lib/time.js';
import { makeDisk } from '../pages/storage/disk.js';
import { usePage } from './fake-dom.mjs';
import { NOW, show } from './storage-fixtures.mjs';

test('the panel of a disk leads to the disk tests and to the temperatures of the Hardware page, when it has one', (context) => {
  const { document } = usePage(context);
  const open = (hardware) => {
    const view = {
      session: { admin: true },
      time: makeTime('UTC', () => NOW * 1000),
      slot: document.createElement('div'),
      api: { inline: () => document.createElement('div') },
      problem() {},
    };
    const panel = makeDisk(view, {
      name: 'sda', scrubber: { paint() {}, button: () => document.createElement('div') }, steps() {}, reread() {}, links: { hardware, backups: false },
    });
    panel.set(show());
    return panel.element;
  };
  const found = open(true);
  const links = found.querySelectorAll('a').filter((one) => one.textContent.includes('Hardware page'));
  assert.deepEqual(links.map((one) => [one.textContent, one.getAttribute('href')]), [
    ['Temperatures on the Hardware page', '/hardware#temps'],
    ['Run one on the Hardware page', '/hardware#disks'],
  ]);
  assert.equal(links[0].hidden, false);
  const without = open(false);
  assert.equal(without.querySelectorAll('a').filter((one) => one.textContent === 'Run one on the Hardware page').length, 0);
  assert.equal(without.querySelector('a.more').hidden, true);
});
