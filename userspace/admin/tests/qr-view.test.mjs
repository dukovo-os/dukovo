// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The QR code that hands a link to a phone: the words around it as plain
// values, and the picture, the link, the fingerprint and the countdown as the
// page builds them.

import assert from 'node:assert/strict';
import test from 'node:test';

import { phoneLink, qrView } from '../lib/qr-view.js';
import {
  countdownWords, fingerprintGroups, leftWords, pictureWords, WARNING_WORDS,
} from '../lib/qr-view-model.js';
import { qrMatrix, qrPath } from '../qr.js';
import { usePage } from './fake-dom.mjs';

const URL_TEXT = 'https://192.168.1.20:8443/once/3q2-7wHq9Zk0aB1cD2eF3g/report.txt';
const FINGERPRINT = Array.from({ length: 32 }, (_, at) => (at * 7 + 10).toString(16).padStart(2, '0').toUpperCase()).join(':');

test('a fingerprint is shown in four groups of eight bytes', () => {
  const groups = fingerprintGroups(FINGERPRINT);
  assert.equal(groups.length, 4);
  assert.deepEqual(groups.map((group) => group.split(':').length), [8, 8, 8, 8]);
  assert.equal(groups.join(':'), FINGERPRINT);
});

test('a fingerprint in lower case, or without colons, is the same bytes in groups', () => {
  assert.deepEqual(fingerprintGroups('ab:cd:ef'), ['AB:CD:EF']);
  assert.deepEqual(fingerprintGroups('abcdef'), ['AB:CD:EF']);
  assert.deepEqual(fingerprintGroups(` ${FINGERPRINT.toLowerCase()} `), fingerprintGroups(FINGERPRINT));
});

test('a fingerprint that is not bytes is kept as it came, and none is no group', () => {
  assert.deepEqual(fingerprintGroups('not a fingerprint'), ['not a fingerprint']);
  assert.deepEqual(fingerprintGroups('ABC'), ['ABC']);
  assert.deepEqual(fingerprintGroups(''), []);
  assert.deepEqual(fingerprintGroups(null), []);
});

test('the time left is in minutes and seconds, rounded up', () => {
  assert.equal(leftWords(600_000), '10 minutes');
  assert.equal(leftWords(599_001), '10 minutes');
  assert.equal(leftWords(580_000), '9 minutes 40 seconds');
  assert.equal(leftWords(61_000), '1 minute 1 second');
  assert.equal(leftWords(1), '1 second');
  assert.equal(leftWords(0), '');
  assert.equal(leftWords(-5), '');
});

test('the countdown says when the link ends and when it has', () => {
  assert.equal(countdownWords(65_000), 'The link ends in 1 minute 5 seconds.');
  assert.equal(countdownWords(0), 'The link has ended.');
});

test('the words name the warning and say that the link works once, for 10 minutes', () => {
  assert.match(WARNING_WORDS, /warns/);
  assert.match(WARNING_WORDS, /compare the fingerprint/);
  assert.match(WARNING_WORDS, /works once, for 10 minutes/);
  assert.equal(pictureWords('report.txt'), 'QR code of a link to report.txt, for 10 minutes');
});

test('the picture is the code qr.js makes of the text, named for a screen reader', (context) => {
  usePage(context);
  const picture = qrView(URL_TEXT, 'The link');
  assert.equal(picture.getAttribute('role'), 'img');
  assert.equal(picture.getAttribute('aria-label'), 'The link');
  const path = picture.querySelector('path');
  assert.equal(path.getAttribute('d'), qrPath(qrMatrix(URL_TEXT).modules));
});

test('a text longer than a code holds has no picture', (context) => {
  usePage(context);
  assert.equal(qrView('x'.repeat(413), 'too long'), null);
  assert.notEqual(qrView('x'.repeat(412), 'longest'), null);
});

test('the link is shown as text beside its picture, with the fingerprint in groups', (context) => {
  usePage(context);
  const { element, stop } = phoneLink({ url: URL_TEXT, fingerprint: FINGERPRINT, expires_ms: 1_000_000 + 600_000, clock: () => 1_000_000 });
  stop();
  assert.equal(element.querySelector('svg').getAttribute('aria-label'), 'QR code of a link to report.txt, for 10 minutes');
  const shown = element.querySelectorAll('.cmdtext').map((node) => node.textContent);
  assert.equal(shown[0], URL_TEXT);
  assert.equal(shown[1], fingerprintGroups(FINGERPRINT).join(' '));
  assert.match(element.textContent, /warns that it does not know this server's certificate/);
  assert.equal(element.querySelector('[role="timer"]').textContent, 'The link ends in 10 minutes.');
});

test('the countdown counts down each second and says once that the link has ended', (context) => {
  usePage(context);
  context.mock.timers.enable({ apis: ['setInterval'] });
  let now = 0;
  let ended = 0;
  const { element } = phoneLink({
    url: URL_TEXT, fingerprint: FINGERPRINT, expires_ms: 3000, clock: () => now, onEnded: () => { ended += 1; },
  });
  const timer = element.querySelector('[role="timer"]');
  assert.equal(timer.textContent, 'The link ends in 3 seconds.');
  now = 1000;
  context.mock.timers.tick(1000);
  assert.equal(timer.textContent, 'The link ends in 2 seconds.');
  now = 3000;
  context.mock.timers.tick(1000);
  assert.equal(timer.textContent, 'The link has ended.');
  assert.equal(ended, 1);
  now = 4000;
  context.mock.timers.tick(5000);
  assert.equal(ended, 1);
});

test('a link that has ended already says so at once and stops nothing that runs', async (context) => {
  usePage(context);
  context.mock.timers.enable({ apis: ['setInterval'] });
  let ended = 0;
  const { element } = phoneLink({ url: URL_TEXT, fingerprint: FINGERPRINT, expires_ms: 500, clock: () => 900, onEnded: () => { ended += 1; } });
  assert.equal(element.querySelector('[role="timer"]').textContent, 'The link has ended.');
  await Promise.resolve();
  assert.equal(ended, 1);
});

test('leaving the page stops the countdown', (context) => {
  usePage(context);
  context.mock.timers.enable({ apis: ['setInterval'] });
  const controller = new AbortController();
  let now = 0;
  const { element } = phoneLink({
    url: URL_TEXT, fingerprint: FINGERPRINT, expires_ms: 5000, signal: controller.signal, clock: () => now,
  });
  controller.abort();
  now = 2000;
  context.mock.timers.tick(2000);
  assert.equal(element.querySelector('[role="timer"]').textContent, 'The link ends in 5 seconds.');
});

test('a link too long for a code is still shown as text', (context) => {
  usePage(context);
  const { element, stop } = phoneLink({ url: `https://h/${'x'.repeat(420)}`, fingerprint: '', expires_ms: 10_000, clock: () => 0 });
  stop();
  assert.equal(element.querySelector('svg'), null);
  assert.match(element.textContent, /too long for a QR code/);
  assert.equal(element.querySelectorAll('.cmdtext').length, 1);
});
