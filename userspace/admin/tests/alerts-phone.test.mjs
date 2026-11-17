// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Setting up a phone: the card of steps on a computer, with the code and the
// certificate's fingerprint, and the panel that the code opens on the phone.

import assert from 'node:assert/strict';
import test from 'node:test';

import { PUBLIC_KEY, pushChannel } from './alerts-fixtures.mjs';
import { buttonOf, openAlerts, reads, useStorage } from './alerts-view.mjs';

const words = (node) => (node.children ? node.children.map(words).join(' ') : node.text);
const text = (root) => words(root).replace(/\s+/g, ' ').trim();

const OTHER = { name: 'this-machine', fingerprint_sha256: '00:11:22:33' };

test('the card says each step with the address the code holds, and the fingerprint', async (context) => {
  useStorage(context);
  const page = await openAlerts(context);
  const card = text(page.card('phone'));
  assert.match(card, /Set up a phone/);
  assert.match(card, /Scan this with the phone's camera\./);
  assert.match(card, /https:\/\/10\.0\.0\.5:8443\/alerts\?phone=1/);
  assert.match(card, /Check the certificate\. The phone warns that it does not know this server's certificate\. Compare the fingerprint it shows with this one, and go on only if they are the same:/);
  assert.match(card, /AA:BB:CC:DD:EE:FF:00:11 22:33:44:55:66:77:88:99/);
  assert.match(card, /Sign in on the phone with your account\./);
  assert.match(card, /Push notices need a certificate the phone trusts: This browser does not trust this server's certificate/);
  assert.match(card, /Add it to the home screen from the browser's menu: it then opens like an app\. No app store, no account\./);
  assert.ok(page.card('phone').querySelector('svg'), 'the code is drawn');
});

test('the code is the QR code of the address the card says', async (context) => {
  useStorage(context);
  const { qrMatrix, qrPath } = await import('../qr.js');
  const page = await openAlerts(context);
  const drawn = page.card('phone').querySelector('svg');
  assert.equal(drawn.querySelector('path').getAttribute('d'), qrPath(qrMatrix('https://10.0.0.5:8443/alerts?phone=1').modules));
  assert.match(drawn.getAttribute('aria-label'), /QR code of this console's address/);
});

test('another certificate reached by its name says that push can work and that the phone should trust it', async (context) => {
  useStorage(context);
  const page = await openAlerts(context, { world: { reads: { ...reads(), '/api/1/certificates/this-machine': OTHER } }, hostname: 'duke-nas.example.org' });
  const card = text(page.card('phone'));
  assert.match(card, /https:\/\/duke-nas\.example\.org:8443\/alerts\?phone=1/);
  assert.match(card, /The phone should trust this server's certificate\. If it warns, compare the fingerprint it shows with this one/);
  assert.match(card, /Turn on alerts on the phone: the page there shows how\./);
  assert.doesNotMatch(card, /Push notices need a certificate/);
});

test('Done on the phone reads the channels again, so that its row shows', async (context) => {
  useStorage(context);
  const page = await openAlerts(context);
  const before = page.calls.filter((path) => path === '/api/1/alerts/channels').length;
  await page.press(page.card('phone'), 'Done on the phone: show it');
  assert.equal(page.calls.filter((path) => path === '/api/1/alerts/channels').length, before + 1);
});

test('on a phone the card of steps is left out', async (context) => {
  useStorage(context);
  const page = await openAlerts(context, { width: 390 });
  assert.equal(page.card('phone'), null);
});

test('a machine with no address says so instead of drawing a code', async (context) => {
  useStorage(context);
  const none = { host: 'duke-nas', addresses: [], port: 8443, fingerprint: 'AA' };
  const page = await openAlerts(context, { world: { reads: { '/api/1/console/about': none } } });
  assert.match(text(page.card('phone')), /no network address that a phone could reach/);
});

test('the code opens a panel on the phone, with Signed in and the way to get alerts', async (context) => {
  useStorage(context);
  const page = await openAlerts(context, { query: 'phone=1', width: 390 });
  const panel = text(page.card('phone'));
  assert.match(panel, /Set up this phone/);
  assert.match(panel, /Signed in/);
  assert.match(panel, /Use the browser's menu: Add to the home screen\./);
  assert.ok(buttonOf(page.card('phone'), 'Get alerts on this phone'));
  assert.ok(buttonOf(page.card('phone'), 'Not now'));
  assert.equal(page.card('phone').querySelector('svg'), null, 'the phone needs no code of its own address');
});

test('the browser\'s offer to add to the home screen becomes a button', async (context) => {
  useStorage(context);
  const page = await openAlerts(context, { query: 'phone=1', width: 390 });
  const listener = page.events.find(([type]) => type === 'beforeinstallprompt')?.[1];
  assert.ok(listener, 'the page listens for the offer');
  let prompted = 0;
  let prevented = false;
  listener({ preventDefault: () => { prevented = true; }, prompt: () => { prompted += 1; } });
  assert.equal(prevented, true);
  await page.press(page.card('phone'), 'Add to the home screen');
  assert.equal(prompted, 1);
});

test('Not now puts the panel away and keeps it away', async (context) => {
  const kept = useStorage(context);
  const page = await openAlerts(context, { query: 'phone=1', width: 390 });
  await page.press(page.card('phone'), 'Not now');
  assert.equal(page.card('phone').hidden, true);
  assert.equal(kept.get('dukovo-admin.alerts.phone.done'), 'true');
  const again = await openAlerts(context, { query: 'phone=1', width: 390 });
  assert.equal(again.card('phone'), null);
});

test('any other value of phone draws no panel', async (context) => {
  useStorage(context);
  const page = await openAlerts(context, { query: 'phone=yes', width: 390 });
  assert.equal(page.card('phone'), null);
});

test('the panel gets alerts through the same steps as the card, and then offers a test', async (context) => {
  useStorage(context);
  const added = { value: false };
  const calls = [];
  const subscription = { options: { applicationServerKey: new Uint8Array(65) }, toJSON: () => ({ endpoint: 'https://push.example.net/x', keys: { p256dh: 'a', auth: 'b' } }), unsubscribe: async () => {} };
  const registration = { pushManager: { getSubscription: async () => null, subscribe: async () => subscription } };
  const browser = {
    navigator: { serviceWorker: { register: async () => registration, ready: Promise.resolve(registration), getRegistration: async () => registration } },
    Notification: { requestPermission: async () => { calls.push('permission'); return 'granted'; } },
  };
  const page = await openAlerts(context, {
    query: 'phone=1',
    width: 390,
    browser,
    world: {
      reads: { '/api/1/alerts/channels': () => (added.value ? [pushChannel({ about: null })] : []) },
      answers: {
        'alerts.push-key': { ok: true, result: { format: 1, public_key: PUBLIC_KEY } },
        'alerts.push-add': () => {
          added.value = true;
          return { ok: true, result: { done: true, changed: true, output: [JSON.stringify({ format: 1, id: '0123456789abcdef' })] } };
        },
      },
    },
  });
  await page.press(page.card('phone'), 'Get alerts on this phone');
  assert.deepEqual(calls, ['permission']);
  assert.equal(page.ran('alerts.push-add').length, 1);
  assert.match(text(page.card('phone')), /This phone gets alerts\./);
  assert.ok(buttonOf(page.card('phone'), 'Send a test'));
});

test('the certificate words show in the panel when the phone does not trust the certificate', async (context) => {
  useStorage(context);
  const browser = {
    navigator: { serviceWorker: { register: async () => { throw Object.assign(new Error('insecure'), { name: 'SecurityError' }); }, ready: new Promise(() => {}), getRegistration: async () => null } },
    Notification: { requestPermission: async () => 'granted' },
  };
  const page = await openAlerts(context, { query: 'phone=1', width: 390, browser });
  await page.press(page.card('phone'), 'Get alerts on this phone');
  assert.match(text(page.card('phone')), /This browser does not trust this server's certificate, so it cannot get push notices\. Two ways:/);
});
