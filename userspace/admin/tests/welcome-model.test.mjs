// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the first sign-in says and decides (design 12.1): where it ends,
// which steps it has, and the words of the zone step and of the offer.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  isUnknownZone, offerWords, serverClockWords, stepsOf, summaryOf, thenPath, useServerWords, zoneWords,
} from '../pages/welcome/model.js';

test('the page that was asked for is kept when it is a path of the console, with its query', () => {
  assert.equal(thenPath('/services'), '/services');
  assert.equal(thenPath('/services/nginx?tab=logs'), '/services/nginx?tab=logs');
  assert.equal(thenPath('/'), '/');
});

test('an address that could leave the console, or come back here, is the Overview', () => {
  for (const raw of [undefined, null, '', 'services', 'https://example.org/', '//example.org/', '/\\example.org', '/\t/example.org', '/a\nb', '/welcome', '/welcome?then=/', '/welcome/x', '/welcome#a']) {
    assert.equal(thenPath(raw), '/', JSON.stringify(raw));
  }
  assert.equal(thenPath('/welcomed'), '/welcomed', 'a page whose name starts with it is a page');
});

test('the steps are the zone and the display, and the second step only when the account has none', () => {
  assert.deepEqual(stepsOf({ second: false }).map((step) => step.id), ['zone', 'display']);
  assert.deepEqual(stepsOf({ second: true }).map((step) => [step.id, step.title]), [['zone', 'Time zone'], ['display', 'Display'], ['second', 'Second step']]);
});

test('the zone step says where the zone comes from only when it is the browser\'s', () => {
  assert.equal(zoneWords({ zone: 'Europe/Prague', browser: 'Europe/Prague' }), 'Times in the console are shown in Europe/Prague, your browser\'s time zone.');
  assert.equal(zoneWords({ zone: 'UTC', browser: 'Europe/Prague' }), 'Times in the console are shown in UTC.');
});

test('the server\'s clock is said from console.about, and left out when it says none', () => {
  assert.equal(serverClockWords({ zone: 'UTC' }), 'The server\'s clock is in UTC.');
  assert.equal(serverClockWords({ zone: '' }), null);
  assert.equal(serverClockWords({}), null);
  assert.equal(serverClockWords(null), null);
  assert.equal(useServerWords('UTC'), 'Use the server\'s: UTC');
});

test('only ADM-125 about the zone field is a zone the server does not know', () => {
  assert.equal(isUnknownZone({ id: 'ADM-125', field: 'zone' }), true);
  assert.equal(isUnknownZone({ id: 'ADM-125', field: 'theme' }), false);
  assert.equal(isUnknownZone({ id: 'ADM-123', field: 'zone' }), false);
  assert.equal(isUnknownZone(null), false);
});

test('each step has a line from what was chosen', () => {
  const chosen = { zone: 'Europe/Prague', display: { theme: 'high-contrast', text: 125, motion: 'less' }, second: false };
  assert.equal(summaryOf('zone', chosen), 'Europe/Prague');
  assert.equal(summaryOf('display', chosen), 'High contrast, 125% text, less motion');
  assert.equal(summaryOf('display', { display: { theme: 'dark', text: 100, motion: 'full' } }), 'Dark, 100% text');
  assert.equal(summaryOf('second', chosen), 'Not added yet');
  assert.equal(summaryOf('second', { second: true }), 'Added');
  assert.equal(summaryOf('zone', {}), '');
  assert.equal(summaryOf('other', chosen), '');
});

test('the offer counts the steps in words', () => {
  assert.deepEqual(offerWords('duke-nas', 5), { title: 'Set up duke-nas now?', detail: 'Five steps, a few minutes each.' });
});
