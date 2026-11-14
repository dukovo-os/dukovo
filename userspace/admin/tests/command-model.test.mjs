// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/command-model.js and lib/panel-model.js: the cache key, the tabs, the
// diff's lines with their words, and the journal's change that tells a
// file's story.

import assert from 'node:assert/strict';
import test from 'node:test';

import { showKey } from '../lib/command-model.js';
import {
  changeFor, diffLines, diffOf, FOOTNOTE, NO_CHANGE, TABS, textOfTab,
} from '../lib/panel-model.js';

test('the panel has four tabs: Command, API, Ansible and Files', () => {
  assert.deepEqual(TABS.map((tab) => tab.label), ['Command', 'API', 'Ansible', 'Files']);
  assert.match(FOOTNOTE, /What you see here is what runs\.$/);
});

test('one key for an action and its arguments, whatever their order', () => {
  assert.equal(showKey('service.restart', { name: 'nginx', force: true }), showKey('service.restart', { force: true, name: 'nginx' }));
  assert.notEqual(showKey('service.restart', { name: 'nginx' }), showKey('service.restart', { name: 'sshd' }));
  assert.notEqual(showKey('a', {}), showKey('b', {}));
});

test('a diff\'s lines say in words what a screen reader cannot see', () => {
  const unified = '--- a.toml (before)\n+++ a.toml (after)\n@@ -1,3 +1,3 @@\n [limits]\n-memory = "256M"\n+memory = "512M"\n';
  const lines = diffLines(unified);
  assert.deepEqual(lines.map((line) => line.kind), ['head', 'head', 'hunk', 'ctx', 'del', 'add']);
  assert.deepEqual(lines.map((line) => line.words), ['', '', 'Lines: ', '', 'Taken away: ', 'Added: ']);
  assert.equal(lines[4].text, '-memory = "256M"');
});

test('the newest change that touched a path tells its story', () => {
  const changes = [
    { id: 9, files: [{ path: '/b' }] },
    { id: 8, files: [{ path: '/a' }, { path: '/b' }] },
    { id: 7, files: [{ path: '/a' }] },
    { id: 6 },
  ];
  assert.equal(changeFor(changes, '/a').id, 8);
  assert.equal(changeFor(changes, '/c'), null);
});

test('the diff of a path is its text, or the journal\'s reason there is none', () => {
  const change = {
    files: [
      { path: '/a', diff: '+x\n', diff_truncated: false },
      { path: '/secret', no_diff: 'it is a secret' },
      { path: '/same' },
    ],
  };
  assert.deepEqual(diffOf(change, '/a'), { text: '+x\n', why: null, truncated: false });
  assert.equal(diffOf(change, '/secret').why, 'No diff is shown: it is a secret.');
  assert.equal(diffOf(change, '/same').why, NO_CHANGE);
  assert.equal(diffOf(change, '/missing').why, NO_CHANGE);
});

test('a tab copies its own text, and Files the diffs together', () => {
  const shown = { command: 'elevate service nginx restart', api: 'curl x', ansible: '- name: x', files: [] };
  assert.equal(textOfTab(shown, 'command'), 'elevate service nginx restart');
  assert.equal(textOfTab(shown, 'ansible'), '- name: x');
  assert.equal(textOfTab(shown, 'files', ['+a', null, '+b']), '+a\n+b');
  assert.equal(textOfTab(shown, 'missing'), '');
});
