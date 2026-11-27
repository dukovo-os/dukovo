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

test('a trial begun elsewhere says who began it when its row comes while the first read of the journal is out', async (context) => {
  const trial = {
    id: 'firewall', files: ['/system/settings/firewall'], services: ['firewall'], user: 1000, left_ms: 290_000,
  };
  const row = {
    action: 'firewall.add', status: 0, argv: ['firewall', 'add', '--try', '300', 'allow', 'tcp', '22'], who: 'alice', how: 'console', files: [],
  };
  let answerFirst;
  const first = new Promise((resolve) => { answerFirst = resolve; });
  let journalReads = 0;
  const world = await openFirewall(context, {
    reads: {
      '/api/1/firewall': { ...listed(), trial: { left_ms: 290_000 } },
      '/api/1/services': {
        show_version: 1, now_ms: 1_000_000, services: [{ name: 'firewall', state: 'running' }], trials: [trial], undone: [],
      },
      '/api/1/changes?limit=100': () => {
        journalReads += 1;
        return journalReads === 1 ? first : { changes: [row] };
      },
    },
  });
  await world.ready;
  assert.equal(journalReads, 1, 'the banner reads the journal when it first sees the trial');

  // The row is written, and the console says so, while that read is out;
  // then that read answers, from before the row.
  await world.emit('change');
  answerFirst({ changes: [] });
  await settle();
  assert.equal(journalReads, 2, 'the new row reads the journal again');
  const words = world.view.main.textContent;
  assert.match(words, /Begun by alice in this console\./);
  assert.doesNotMatch(words, /Begun at a terminal/);
});

test('a trial begun elsewhere reads the journal once at a time, and a read that fails says nothing', async (context) => {
  const trial = {
    id: 'firewall', files: ['/system/settings/firewall'], services: ['firewall'], user: 1000, left_ms: 290_000,
  };
  const row = {
    action: 'firewall.add', status: 0, argv: ['firewall', 'add', '--try', '300', 'allow', 'tcp', '22'], who: 'alice', how: 'console', files: [],
  };
  const reads = [];
  const world = await openFirewall(context, {
    reads: {
      '/api/1/firewall': { ...listed(), trial: { left_ms: 290_000 } },
      '/api/1/services': {
        show_version: 1, now_ms: 1_000_000, services: [{ name: 'firewall', state: 'running' }], trials: [trial], undone: [],
      },
      '/api/1/changes?limit=100': () => new Promise((resolve, reject) => { reads.push({ resolve, reject }); }),
    },
  });
  await world.ready;

  // New rows ask while the first read is out: they wait for it.
  await world.emit('change');
  await world.emit('change');
  assert.equal(reads.length, 1, 'a second read of the journal began while the first was out');

  // The first read fails, says nothing, and the one asked for meanwhile
  // follows it and finds the row.
  reads[0].reject(new Error('the journal is away'));
  await settle();
  assert.doesNotMatch(world.view.main.textContent, /Begun/);
  assert.equal(reads.length, 2, 'the read asked for meanwhile did not follow');
  reads[1].resolve({ changes: [row] });
  await settle();
  assert.match(world.view.main.textContent, /Begun by alice in this console\./);
});

test('a trial begun elsewhere says who began it when its row comes and no change is heard', async (context) => {
  const trial = {
    id: 'firewall', files: ['/system/settings/firewall'], services: ['firewall'], user: 1000, left_ms: 290_000,
  };
  const row = {
    action: 'firewall.add', status: 0, argv: ['firewall', 'add', '--try', '300', 'allow', 'tcp', '22'], who: 'alice', how: 'console', files: [],
  };
  let rows = [];
  const world = await openFirewall(context, {
    reads: {
      '/api/1/firewall': { ...listed(), trial: { left_ms: 290_000 } },
      '/api/1/services': {
        show_version: 1, now_ms: 1_000_000, services: [{ name: 'firewall', state: 'running' }], trials: [trial], undone: [],
      },
      '/api/1/changes?limit=100': () => ({ changes: rows }),
    },
  });
  await world.ready;
  assert.match(world.view.main.textContent, /Begun at a terminal\./, 'the first read of the journal has no row');

  // The row is kept, but this page hears no change: its next read of the
  // document reads the journal again.
  rows = [row];
  await world.every();
  assert.match(world.view.main.textContent, /Begun by alice in this console\./);
});
