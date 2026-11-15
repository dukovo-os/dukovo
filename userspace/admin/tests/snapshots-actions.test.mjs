// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The actions that the Snapshots and backups page runs, against the registry
// of admind (design 13.2 and 22): each one is listed with the route and the
// method the page expects, and what the page sends for it is what its schema
// takes. The page is written against the design's IDs while the actions are
// written, so a test whose action is not in the registry yet is skipped, and
// the skip says so; each runs once the action is listed.

import assert from 'node:assert/strict';
import test from 'node:test';

import { addArgs } from '../pages/snapshots/form-model.js';
import { listedActions } from './registry.mjs';

const LISTED = new Map(listedActions().map((action) => [action.id, action]));

/** What the page sends, for the action `id`, and the method and route it expects. */
const CALLS = Object.freeze({
  'backups.add': {
    method: 'POST',
    path: '/api/1/backups/add',
    args: () => addArgs({
      where: 'ssh', volume: null, address: 'backup@nas.local:/srv/dukovo', name: 'nas', every: 'day', verify: 'week', format: 'restic',
    }).args,
  },
  'backups.run': { method: 'POST', path: '/api/1/backups/{name}/run', args: () => ({ name: 'nas' }) },
  'backups.verify': { method: 'POST', path: '/api/1/backups/{name}/verify', args: () => ({ name: 'nas' }) },
  'backups.restore': { method: 'POST', path: '/api/1/backups/{name}/restore', args: () => ({ name: 'nas', place: '/users', at: '2026-11-14-030212' }) },
  'backups.show': { method: 'GET', path: '/api/1/backups/{name}', args: () => ({ name: 'nas' }) },
  // Section 22: a read whose answer is a secret is a POST, and the page never refreshes or retries it.
  'backups.key': { method: 'POST', path: '/api/1/backups/{name}/key', args: () => ({ name: 'nas' }) },
  'backups.remove': { method: 'POST', path: '/api/1/backups/{name}/remove', args: () => ({ name: 'nas' }) },
  'backups.ssh-key': { method: 'GET', path: '/api/1/backups/ssh-key', args: () => ({}) },
  'snapshots.take': { method: 'POST', path: '/api/1/snapshots/take', args: () => ({ place: '/users', name: 'before-the-move' }) },
  'snapshots.restore': {
    method: 'POST', path: '/api/1/snapshots/restore', args: () => ({ snapshot: '2026-11-13-0302', label: 'Nov 13 3:02 AM', paths: '/users/owner/a.txt\n/users/owner/b' }),
  },
  'snapshots.restore-all': {
    method: 'POST', path: '/api/1/snapshots/restore/all', args: () => ({ snapshot: '2026-11-13-0302', label: 'Nov 13 3:02 AM', paths: '/shared/site/index.html' }),
  },
  'snapshots.browse-all': {
    method: 'GET', path: '/api/1/snapshots/browse/all', args: () => ({ place: '/users', snapshot: '2026-11-13-0302', folder: '/users/owner' }),
  },
});

/** The reason a test is skipped while its action is not in the registry: it names the action, for the report of what waits. */
const waits = (id) => (LISTED.has(id) ? false : `waits for the action ${id}, which is not in lab/actions.toml yet`);

/** Why `args` do not fit `schema`, or empty text: the keys, the ones that are required, and the limits of each string. */
function misfit(schema, args) {
  const known = Object.keys(schema.properties);
  for (const key of Object.keys(args)) if (!known.includes(key)) return `${key} is not an argument`;
  for (const key of schema.required) if (args[key] === undefined) return `${key} is required`;
  for (const [key, value] of Object.entries(args)) {
    const rule = schema.properties[key];
    if (rule.enum && !rule.enum.includes(value)) return `${key} must be one of ${rule.enum.join(', ')}`;
    if (typeof value === 'string') {
      if (rule.maxLength !== undefined && Buffer.byteLength(value) > rule.maxLength) return `${key} is too long`;
      if (rule.minLength !== undefined && value.length < rule.minLength) return `${key} is too short`;
      if (rule.pattern && !new RegExp(rule.pattern).test(value)) return `${key} does not match ${rule.pattern}`;
    }
  }
  return '';
}

for (const [id, call] of Object.entries(CALLS)) {
  test(`${id}: the route is ${call.method} ${call.path}, and what the page sends is what it takes`, { skip: waits(id) }, () => {
    const action = LISTED.get(id);
    assert.equal(action.method, call.method);
    assert.equal(action.path, call.path);
    assert.equal(misfit(action.input, call.args()), '');
  });
}

// The check itself is tried against a schema of its own, so that it is known to find what it is for when the actions are there.
test('the check of arguments finds an unknown one, a missing one and one out of its limits', () => {
  const schema = {
    properties: { name: { type: 'string', minLength: 1, maxLength: 4, pattern: '^[a-z]+$' }, every: { enum: ['day', 'week'] }, place: { type: 'string' } },
    required: ['name'],
  };
  assert.equal(misfit(schema, { name: 'nas', every: 'day' }), '');
  assert.equal(misfit(schema, { every: 'day' }), 'name is required');
  assert.equal(misfit(schema, { name: 'nas', other: 1 }), 'other is not an argument');
  assert.equal(misfit(schema, { name: 'nas', every: 'hand' }), 'every must be one of day, week');
  assert.equal(misfit(schema, { name: 'naaas' }), 'name is too long');
  assert.equal(misfit(schema, { name: '' }), 'name is too short');
  assert.equal(misfit(schema, { name: 'N1' }), 'name does not match ^[a-z]+$');
});

test('the page runs the twelve actions of the design that it names, and no more', () => {
  assert.deepEqual(Object.keys(CALLS).sort(), [
    'backups.add', 'backups.key', 'backups.remove', 'backups.restore', 'backups.run', 'backups.show', 'backups.ssh-key', 'backups.verify',
    'snapshots.browse-all', 'snapshots.restore', 'snapshots.restore-all', 'snapshots.take',
  ]);
  for (const call of Object.values(CALLS)) assert.equal(typeof call.args(), 'object');
});
