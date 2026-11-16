// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The services' part of the Overview and the trials of design 8.6: a change
// that Spark undoes unless someone keeps it is a warning that says when it is
// undone, as the time of day and not a countdown, so that the list stays still.

import assert from 'node:assert/strict';
import test from 'node:test';

import { makeTime } from '../lib/time.js';
import { failedOf, needsOf, trialNeeds } from '../pages/services/needs.js';
import { needs } from '../pages/services/overview.js';
import { document, NOW } from './services-fixtures.mjs';
import { fakeCx, services as overviewServices } from './overview-fixtures.mjs';

// 10:12 UTC now, as the document's fixtures have it.
const utcTime = makeTime('UTC', () => NOW * 1000);

const trial = (id, left_ms) => ({ id, files: [], services: [], user: 0, left_ms });

test('a service\'s trial is a warning that names the service and the time it is undone', () => {
  const [found] = trialNeeds({ trials: [trial('service.nginx', 42_000)] }, NOW, utcTime);
  assert.deepEqual(found, {
    id: 'trial-service.nginx',
    severity: 'warn',
    title: 'The settings of nginx are being tried',
    detail: 'They are undone at 10:12 AM unless they are kept.',
    href: '/services/nginx',
    icon: 'services',
    at: null,
    covers: [],
  });
});

test('the network\'s and the firewall\'s trials have the words of design 8.6 and their own pages', () => {
  const [network, firewall] = trialNeeds({ trials: [trial('network', 290_000), trial('firewall', 120_000)] }, NOW, utcTime);
  assert.deepEqual([network.title, network.detail, network.href, network.icon], [
    'Network settings are being tried', 'They are undone at 10:16 AM unless they are kept.', '/networking', 'network',
  ]);
  assert.deepEqual([firewall.title, firewall.detail, firewall.href, firewall.icon], [
    'Firewall rules are being tried', 'They are undone at 10:14 AM unless they are kept.', '/firewall', 'firewall',
  ]);
  assert.equal(network.severity, 'warn');
  assert.equal(firewall.severity, 'warn');
});

test('SSH\'s and the console\'s reach trials lead to the Security page', () => {
  const [ssh, reach] = trialNeeds({ trials: [trial('ssh', 290_000), trial('console', 120_000)] }, NOW, utcTime);
  assert.deepEqual([ssh.title, ssh.detail, ssh.href, ssh.icon], [
    'SSH settings are being tried', 'They are undone at 10:16 AM unless they are kept.', '/security', 'security',
  ]);
  assert.deepEqual([reach.title, reach.href, reach.icon], ['Settings of who may reach the console are being tried', '/security', 'security']);
});

test('the time is the person\'s own, and a time on another day says the day', () => {
  const prague = makeTime('Europe/Prague', () => NOW * 1000);
  assert.equal(trialNeeds({ trials: [trial('network', 60_000)] }, NOW, prague)[0].detail, 'They are undone at 11:13 AM unless they are kept.');
  const [late] = trialNeeds({ trials: [trial('network', 15 * 3600 * 1000)] }, NOW, utcTime);
  assert.equal(late.detail, 'They are undone at Nov 15 1:12 AM unless they are kept.');
});

test('the words stay still while the countdown moves, so the list does not change under a reader', () => {
  const first = trialNeeds({ trials: [trial('network', 290_000)] }, NOW, utcTime)[0];
  const later = trialNeeds({ trials: [trial('network', 250_000)] }, NOW + 40, utcTime)[0];
  assert.deepEqual(later, first);
  assert.doesNotMatch(first.detail, /second|minute|\d:\d\d left|in \d/);
});

test('a trial that is another one is a warning with no page, and one with no time does not guess it', () => {
  const [other, nothing] = trialNeeds({ trials: [trial('storage', 60_000), { id: 'network' }] }, NOW, utcTime);
  assert.deepEqual([other.title, other.href, other.icon], ['Settings are being tried', null, 'info']);
  assert.equal(nothing.detail, 'They are undone unless they are kept.');
  assert.equal(trialNeeds({ trials: [trial('network', NaN)] }, NOW, utcTime)[0].detail, 'They are undone unless they are kept.');
});

test('a service\'s name is made safe in the address, and an id with no name is no service', () => {
  assert.equal(trialNeeds({ trials: [trial('service.a b/c', 1000)] }, NOW, utcTime)[0].href, '/services/a%20b%2Fc');
  const [bare] = trialNeeds({ trials: [trial('service.', 1000)] }, NOW, utcTime);
  assert.deepEqual([bare.title, bare.href], ['Settings are being tried', null]);
  assert.equal(trialNeeds({ trials: [trial('constructor', 1000)] }, NOW, utcTime)[0].title, 'Settings are being tried');
});

test('there are no needs from a document with no trials, or one that is not the document', () => {
  assert.deepEqual(trialNeeds(overviewServices, NOW, utcTime), []);
  for (const answer of [null, undefined, {}, { trials: null }, { trials: {} }, { trials: [null, {}, { id: '' }, { id: 4 }] }]) {
    assert.deepEqual(trialNeeds(answer, NOW, utcTime), []);
  }
});

test('the part gives the failed services and the trials, from one read of the document', async () => {
  const realNow = Date.now;
  Date.now = () => NOW * 1000;
  try {
    const cx = fakeCx({ '/api/1/services': document });
    cx.time = utcTime;
    const found = await needs(cx);
    assert.deepEqual(found.map((one) => one.id), ['service-failed-backup-sync', 'trial-service.nginx', 'trial-network']);
    assert.deepEqual(found.map((one) => one.severity), ['bad', 'warn', 'warn']);
    assert.deepEqual(cx.calls, ['/api/1/services']);
    assert.deepEqual(found.slice(0, 1), needsOf(document, NOW));
  } finally {
    Date.now = realNow;
  }
});

test('a service that is only being tried is not failed, and a failed one is not a trial', () => {
  assert.deepEqual(failedOf(document).map((one) => one.name), ['backup-sync']);
  assert.deepEqual(needsOf({ ...document, services: document.services.slice(0, 2) }, NOW), []);
});
