// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The actions that the Security page runs, against the registry of admind
// (design 8.2, 13.2 and 22): each one is listed with the route and the method
// the page expects, and what the page sends for it is what its schema takes.
// The page is written against the design's IDs while the actions are written,
// so a test whose action is not in the registry yet, or does not take an
// argument the page sends yet, is skipped, and the skip says so; each runs
// once the action is listed.

import assert from 'node:assert/strict';
import test from 'node:test';

import { DEFAULTS, openPort, serveArgs } from '../pages/security/private-form-model.js';
import { ruleChange } from '../pages/security/rules-form-model.js';
import { callsOf, setArgs } from '../pages/security/ssh-form-model.js';
import { keyLine } from './accounts-fixtures.mjs';
import { listedActions } from './registry.mjs';

const LISTED = new Map(listedActions().map((action) => [action.id, action]));

const SSH_NOW = { on: true, passwords: true, keys: true, allow_users: [], allow_groups: [] };
const SSH_NARROWER = {
  on: true, passwords: false, keys: true, allow_users: ['alice', 'owner'], allow_groups: ['admins'],
};
const RULES = { second_step: 'administrators', session_hours: 12, idle_minutes: 30 };

/** The call of `action` among the calls that the form of SSH makes. */
const sshCall = (before, after, seconds, action) => callsOf(before, after, seconds).find((call) => call.action === action).args;

/** What the page sends, for the action `id`: the method and route it expects, and the arguments, each list one that must fit. `needs` are arguments that the action must take, else the test waits for them. */
const CALLS = Object.freeze({
  'checks.last': { method: 'GET', path: '/api/1/checks/last', args: [{}] },
  'checks.run': { method: 'POST', path: '/api/1/checks/run', args: [{}] },
  'security.sign-in': { method: 'GET', path: '/api/1/security/sign-in', args: [{}] },
  'security.sign-in-set': {
    method: 'POST',
    path: '/api/1/security/sign-in',
    args: [ruleChange(RULES, { second_step: 'everyone', hours: '8', minutes: '15' }).args, ruleChange(RULES, { second_step: 'if-set', hours: '12', minutes: '30' }).args],
  },
  'security.blocked': { method: 'GET', path: '/api/1/security/blocked', args: [{}] },
  'security.unblock': { method: 'POST', path: '/api/1/security/unblock', args: [{ address: '203.0.113.41/32' }, { address: '2001:db8::1/128' }] },
  'security.never-block': { method: 'POST', path: '/api/1/security/never-block', args: [{ network: '192.168.7.0/24' }, { network: '203.0.113.9' }] },
  'security.never-block-remove': { method: 'POST', path: '/api/1/security/never-block/remove', args: [{ network: '10.8.0.0/24' }] },
  'ssh.show': { method: 'GET', path: '/api/1/ssh', args: [{}] },
  'ssh.set': {
    method: 'POST',
    path: '/api/1/ssh',
    args: [
      setArgs(SSH_NOW, SSH_NARROWER),
      sshCall(SSH_NOW, SSH_NARROWER, 300, 'ssh.set'),
      setArgs({ ...SSH_NOW, passwords: false }, SSH_NOW),
      setArgs(SSH_NARROWER, SSH_NOW),
    ],
    needs: ['try'],
  },
  'ssh.on': { method: 'POST', path: '/api/1/ssh/on', args: [{}] },
  'ssh.off': { method: 'POST', path: '/api/1/ssh/off', args: [{}, sshCall(SSH_NOW, { ...SSH_NOW, on: false }, 600, 'ssh.off')], needs: ['try'] },
  'ssh.keep': { method: 'POST', path: '/api/1/ssh/trial/keep', args: [{}] },
  'ssh.undo': { method: 'POST', path: '/api/1/ssh/trial/undo', args: [{}] },
  'console.reach': { method: 'POST', path: '/api/1/console/reach', args: [{ reach: 'private' }, { reach: 'any', try: 60 }, { reach: 'private', try: 600 }], needs: ['try'] },
  'console.keep': { method: 'POST', path: '/api/1/console/trial/keep', args: [{}] },
  'console.undo': { method: 'POST', path: '/api/1/console/trial/undo', args: [{}] },
  'private.show': { method: 'GET', path: '/api/1/private', args: [{}] },
  'private.serve': {
    method: 'POST',
    path: '/api/1/private/serve',
    args: [
      serveArgs({ endpoint: 'duke-lab', port: String(DEFAULTS.port), address: DEFAULTS.address }),
      serveArgs({ endpoint: 'home.example.org', port: '51999', address: '10.9.0.1/24' }),
    ],
  },
  'private.add': { method: 'POST', path: '/api/1/private/devices/add', args: [{ name: 'phone' }, { name: 'a-laptop_2' }] },
  'private.remove': { method: 'POST', path: '/api/1/private/devices/remove', args: [{ name: 'phone' }] },
  'firewall.add': { method: 'POST', path: '/api/1/firewall/add', args: [openPort(DEFAULTS.port), openPort(51_999)] },
  'service.start': { method: 'POST', path: '/api/1/services/{name}/start', args: [{ name: 'firewall' }] },
  'accounts.list': { method: 'GET', path: '/api/1/accounts', args: [{}] },
  'accounts.last': { method: 'GET', path: '/api/1/sign-ins/last', args: [{}] },
  'account.key-add': { method: 'POST', path: '/api/1/account/keys/add', args: [{ line: keyLine() }] },
});

/** Why `args` do not fit `schema`, or empty text: the keys, the ones that are required, and the limits of each string and number. */
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
    if (typeof value === 'number') {
      if (rule.minimum !== undefined && value < rule.minimum) return `${key} is too small`;
      if (rule.maximum !== undefined && value > rule.maximum) return `${key} is too big`;
    }
  }
  return '';
}

/** The reason a test is skipped while its action is not in the registry, or does not take what the page sends: it names what waits, for the report of what is open. */
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

test('the check of arguments finds an unknown one, a missing one and one out of its limits', () => {
  const schema = {
    properties: { name: { type: 'string', minLength: 1, maxLength: 8 }, port: { type: 'number', minimum: 1024, maximum: 65_535 } },
    required: ['name'],
  };
  assert.equal(misfit(schema, { name: 'phone', port: 51_820 }), '');
  assert.equal(misfit(schema, { port: 51_820 }), 'name is required');
  assert.equal(misfit(schema, { name: 'phone', other: 1 }), 'other is not an argument');
  assert.equal(misfit(schema, { name: 'a-long-name' }), 'name is too long');
  assert.equal(misfit(schema, { name: 'phone', port: 80 }), 'port is too small');
});

test('the page runs no action that this file does not name, and none of the names is made up', async () => {
  const { readFileSync, readdirSync } = await import('node:fs');
  const folder = new URL('../pages/security/', import.meta.url);
  const source = readdirSync(folder).filter((name) => name.endsWith('.js')).map((name) => readFileSync(new URL(name, folder), 'utf8')).join('\n');
  // The Alerts card's actions are listed with the Alerts page's, in alerts-actions.test.mjs.
  const named = new Set([...source.matchAll(/(?:run\(|action: |action = )'((?!alerts\.)[a-z-]+\.[a-z-]+)'/g)].map((found) => found[1]));
  // The sign-in rules and the reads are asked by their route, not by an id.
  for (const id of named) assert.ok(id in CALLS, `${id} is named by the page and not tested here`);
  for (const id of ['checks.run', 'security.unblock', 'ssh.keep', 'console.undo', 'private.add', 'private.remove']) assert.ok(named.has(id) || source.includes(`'${id}'`), `${id} is run by the page`);
});
