// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The actions that the page that adds an app runs, against the registry of
// admind: each one is listed with the route and the method the page expects,
// and what the page sends for it is what its schema takes. The page is
// written against the design's IDs while the actions are written, so a test
// whose action is not in the registry yet is skipped, and the skip says so;
// each runs once the action is listed.

import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import test from 'node:test';

import { searchPath, sheetPath, wordsOf } from '../lib/catalog-model.js';
import { sheetOf } from '../lib/sheet-model.js';
import { installStep } from '../pages/add-app/install.js';
import { nginx, SHA } from './catalog-fixtures.mjs';
import { listedActions } from './registry.mjs';

const LISTED = new Map(listedActions().map((action) => [action.id, action]));

/** What the page sends to install the app of the sheet of nginx, which is what its one step holds. */
const INSTALL = installStep(sheetOf(nginx(), { admin: true })).args;

/** What the page sends, for the action `id`, and the method and route they expect. */
const CALLS = Object.freeze({
  'apps.search': {
    method: 'GET', path: '/api/1/apps/catalog/search', args: [{}, { words: wordsOf('nginx').words.join(' ') }, { words: wordsOf('A b c d E').words.join(' ') }],
  },
  'apps.sheet': { method: 'GET', path: '/api/1/apps/catalog/{id}/sheet', args: [{ id: 'org.nginx.nginx' }] },
  'apps.install': {
    method: 'POST', path: '/api/1/apps/catalog/{id}/install', args: [INSTALL], needs: ['sha256'],
  },
});

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

/** The reason a test is skipped while its action is not in the registry, or does not take what the page sends: it names what waits. */
function waits(id, needs = []) {
  const action = LISTED.get(id);
  if (!action) return `waits for the action ${id}, which is not in lab/actions.toml yet`;
  const missing = needs.filter((key) => action.input.properties[key] === undefined);
  return missing.length > 0 ? `waits for ${id} to take ${missing.join(', ')}` : false;
}

for (const [id, call] of Object.entries(CALLS)) {
  test(`${id}: the route is ${call.method} ${call.path}, and what the page sends is what it takes`, { skip: waits(id, call.needs) }, () => {
    const action = LISTED.get(id);
    assert.equal(action.method, call.method);
    assert.equal(action.path, call.path);
    for (const args of call.args) assert.equal(misfit(action.input, args), '', JSON.stringify(args));
  });
}

test('the page reads the routes that this file names, with the words and the ID the actions take', () => {
  assert.equal(searchPath([]), CALLS['apps.search'].path);
  assert.equal(searchPath(['nginx', 'web']), `${CALLS['apps.search'].path}?words=nginx%20web`);
  assert.equal(sheetPath('org.nginx.nginx'), CALLS['apps.sheet'].path.replace('{id}', 'org.nginx.nginx'));
  assert.deepEqual(CALLS['apps.search'].args[2], { words: 'a b c d e' }, 'the words are lowercase and one text');
});

test('the install the page runs is the action, a task, with the ID and the sheet\'s hash and nothing else', () => {
  const step = installStep(sheetOf(nginx(), { admin: true }));
  assert.equal(step.action, 'apps.install');
  assert.equal(step.wait, 'task');
  assert.deepEqual(step.args, { id: 'org.nginx.nginx', sha256: SHA });
});

test('the page runs the install by its action and reads the other two by their routes', () => {
  const folder = new URL('../pages/add-app/', import.meta.url);
  const source = readdirSync(folder).filter((name) => name.endsWith('.js')).map((name) => readFileSync(new URL(name, folder), 'utf8')).join('\n');
  const named = new Set([...source.matchAll(/'(apps\.[a-z-]+)'/g)].map((found) => found[1]));
  assert.deepEqual([...named], ['apps.install']);
});

test('the check of arguments finds an unknown one, a missing one and one out of its limits', () => {
  const schema = { properties: { id: { type: 'string', pattern: '^[0-9a-f]{4}$' }, more: { type: 'string', enum: ['a'] } }, required: ['id'] };
  assert.equal(misfit(schema, { id: '00ff' }), '');
  assert.equal(misfit(schema, {}), 'id is required');
  assert.equal(misfit(schema, { id: '00ff', other: 1 }), 'other is not an argument');
  assert.equal(misfit(schema, { id: 'xyz' }), 'id does not match ^[0-9a-f]{4}$');
  assert.equal(misfit(schema, { id: '00ff', more: 'b' }), 'more must be one of a');
});
