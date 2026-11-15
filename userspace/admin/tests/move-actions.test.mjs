// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The actions that the Move to new hardware page runs, against the registry
// of admind (design 13.2 and 22): each one is listed with the route and the
// method the page expects, and what the page sends for it is what its schema
// takes. The page is written against the design's IDs while the actions are
// written, so a test whose action is not in the registry yet is skipped, and
// the skip says so; each runs once the action is listed.

import assert from 'node:assert/strict';
import test from 'node:test';

import { argsOf, cardLines, defaultPicks } from '../pages/move/model.js';
import { OLD0, NEW0, plan } from './move-fixtures.mjs';
import { listedActions } from './registry.mjs';

const LISTED = new Map(listedActions().map((action) => [action.id, action]));

const FOLDER = '/volumes/LAB/dukovo-backup/lab-machine';
const KEY_FILE = '/volumes/KEYS/lab-machine-recovery.key';
const CARDS = cardLines(plan(), defaultPicks(plan()));

/** What the page sends, for the action `id`, and the method and route it expects. */
const CALLS = Object.freeze({
  'move.find': { method: 'GET', path: '/api/1/move/sources', args: () => ({}) },
  // Section 22: a read that takes a secret is a POST, so that the key is in no address.
  'move.plan': { method: 'POST', path: '/api/1/move/plan', args: () => argsOf({ source: FOLDER, file: KEY_FILE }) },
  'move.stage': { method: 'POST', path: '/api/1/move/stage', args: () => argsOf({ source: FOLDER, file: KEY_FILE, cards: CARDS, at: plan().at }) },
  'move.cancel': { method: 'POST', path: '/api/1/move/cancel', args: () => ({}) },
});

/** More calls of the same actions, with a typed key and a backup place, which must fit as well. */
const AND = Object.freeze({
  'move.plan': [() => argsOf({ source: 'nas', typed: 'AGE-SECRET-KEY-1TYPED' }), () => argsOf({ source: FOLDER, file: KEY_FILE, cards: `${OLD0}=automatic` })],
  'move.stage': [() => argsOf({ source: 'nas', typed: 'AGE-SECRET-KEY-1TYPED', cards: `${OLD0}=${NEW0}`, at: plan().at })],
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
    for (const more of AND[id] ?? []) assert.equal(misfit(action.input, more()), '');
  });
}

test('the check of arguments finds an unknown one, a missing one and one out of its limits', () => {
  const schema = {
    properties: { source: { type: 'string', minLength: 1, maxLength: 8 }, key: { type: 'string' } },
    required: ['source'],
  };
  assert.equal(misfit(schema, { source: '/s', key: 'k' }), '');
  assert.equal(misfit(schema, { key: 'k' }), 'source is required');
  assert.equal(misfit(schema, { source: '/s', other: 1 }), 'other is not an argument');
  assert.equal(misfit(schema, { source: '/long/long' }), 'source is too long');
});

test('the page runs the four actions of the design that it names, and no more', () => {
  assert.deepEqual(Object.keys(CALLS).sort(), ['move.cancel', 'move.find', 'move.plan', 'move.stage']);
  for (const call of Object.values(CALLS)) assert.equal(typeof call.args(), 'object');
});

test('the page gives the action one of the key file and the key, and never both', () => {
  for (const make of [...Object.values(CALLS).map((call) => call.args), ...Object.values(AND).flat()]) {
    const args = make();
    assert.ok(!('key' in args && 'key_file' in args), 'both a key file and a key');
  }
  assert.deepEqual(Object.keys(argsOf({ source: 'nas', file: KEY_FILE, typed: 'k' })).sort(), ['key', 'source']);
});
