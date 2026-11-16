// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The actions that the Alerts page and the Alerts card of Security run,
// against the registry of admind: each one is listed with the route and the
// method the pages expect, and what they send for it is what its schema takes.
// The pages are written against the design's IDs while the actions are
// written, so a test whose action is not in the registry yet is skipped, and
// the skip says so; each runs once the action is listed.

import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import test from 'node:test';

import { emailArgs, webhookArgs } from '../pages/alerts/forms-model.js';
import { aboutArg, channelArg } from '../pages/alerts/told-model.js';
import { listedActions } from './registry.mjs';

const LISTED = new Map(listedActions().map((action) => [action.id, action]));

const KINDS = ['data', 'stopped', 'hardware'];
const EMAIL = {
  to: 'ops@example.org\nadmin@example.org', server: 'mail.example.org', from: '', user: 'ops', port: '587', severity: 'warning', cleared: true, about: KINDS,
};
const WEBHOOK = {
  kind: 'ntfy', room: '', severity: 'critical', cleared: false, about: ['data'],
};
const ID = '0123456789abcdef';
const SUBSCRIPTION = JSON.stringify({ endpoint: 'https://push.example.org/send/abc', keys: { p256dh: 'BP'.repeat(40), auth: 'au'.repeat(8) } });

/** What the pages send, for the action `id`, and the method and route they expect. */
const CALLS = Object.freeze({
  'alerts.list': { method: 'GET', path: '/api/1/alerts', args: [{}] },
  'alerts.channels': { method: 'GET', path: '/api/1/alerts/channels', args: [{}] },
  'alerts.test': { method: 'POST', path: '/api/1/alerts/test', args: [{}] },
  'alerts.email': {
    method: 'POST',
    path: '/api/1/alerts/email',
    args: [emailArgs(EMAIL, 'secret'), emailArgs({ ...EMAIL, user: '', port: '465', about: ['hardware'] }, '')],
    needs: ['about'],
  },
  'alerts.email-remove': { method: 'POST', path: '/api/1/alerts/email/remove', args: [{}] },
  'alerts.webhook-add': {
    method: 'POST',
    path: '/api/1/alerts/webhooks',
    args: [
      webhookArgs(WEBHOOK, 'https://ntfy.sh/duke-nas-1a2b3c', ''),
      webhookArgs({ ...WEBHOOK, kind: 'matrix', room: '!abc:example.org', about: KINDS }, 'https://matrix.example.org/hook', 'token'),
      webhookArgs({ ...WEBHOOK, kind: 'json' }, 'http://10.0.2.2:8080/alerts', 'token'),
    ],
    needs: ['about'],
  },
  'alerts.webhook-remove': { method: 'POST', path: '/api/1/alerts/webhooks/{number}/remove', args: [{ number: 2 }] },
  'alerts.push-key': { method: 'GET', path: '/api/1/alerts/push/key', args: [{}] },
  'alerts.push-add': {
    method: 'POST',
    path: '/api/1/alerts/push',
    args: [
      { subscription: SUBSCRIPTION, name: 'Firefox on a laptop', severity: 'warning', cleared: true },
      { subscription: SUBSCRIPTION, name: 'Phone', severity: 'critical', cleared: false, about: aboutArg(['data', 'hardware']) },
    ],
    needs: ['about'],
  },
  'alerts.push-remove': { method: 'POST', path: '/api/1/alerts/push/{id}/remove', args: [{ id: ID }] },
  'alerts.channel-about': {
    method: 'POST',
    path: '/api/1/alerts/channels/about',
    args: [
      { channel: channelArg({ kind: 'email' }), about: aboutArg(['data', 'stopped']) },
      { channel: channelArg({ kind: 'webhook', number: 1 }), about: aboutArg(KINDS) },
      { channel: channelArg({ kind: 'push', id: ID }), about: aboutArg(['hardware']) },
    ],
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

/** The reason a test is skipped while its action is not in the registry, or does not take what the pages send: it names what waits. */
function waits(id, needs = []) {
  const action = LISTED.get(id);
  if (!action) return `waits for the action ${id}, which is not in lab/actions.toml yet`;
  const missing = needs.filter((key) => action.input.properties[key] === undefined);
  return missing.length > 0 ? `waits for ${id} to take ${missing.join(', ')}` : false;
}

for (const [id, call] of Object.entries(CALLS)) {
  test(`${id}: the route is ${call.method} ${call.path}, and what the pages send is what it takes`, { skip: waits(id, call.needs) }, () => {
    const action = LISTED.get(id);
    assert.equal(action.method, call.method);
    assert.equal(action.path, call.path);
    for (const args of call.args) assert.equal(misfit(action.input, args), '', JSON.stringify(args));
  });
}

test('the check of arguments finds an unknown one, a missing one and one out of its limits', () => {
  const schema = { properties: { id: { type: 'string', pattern: '^[0-9a-f]{16}$' }, more: { type: 'string', enum: ['a'] } }, required: ['id'] };
  assert.equal(misfit(schema, { id: ID }), '');
  assert.equal(misfit(schema, {}), 'id is required');
  assert.equal(misfit(schema, { id: ID, other: 1 }), 'other is not an argument');
  assert.equal(misfit(schema, { id: 'xyz' }), 'id does not match ^[0-9a-f]{16}$');
  assert.equal(misfit(schema, { id: ID, more: 'b' }), 'more must be one of a');
});

test('the pages run the actions of the design that this file names, and no more', () => {
  const folders = ['pages/alerts/', 'pages/security/'].map((name) => new URL(`../${name}`, import.meta.url));
  const source = folders.flatMap((folder) => readdirSync(folder).filter((name) => name.endsWith('.js')).map((name) => readFileSync(new URL(name, folder), 'utf8'))).join('\n');
  const named = new Set([...source.matchAll(/'(alerts\.[a-z-]+)'/g)].map((found) => found[1]));
  // The Alerts page reads the active alerts by their route, not by the action's ID.
  assert.deepEqual([...named].sort(), Object.keys(CALLS).filter((id) => id !== 'alerts.list').sort());
});
