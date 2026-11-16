// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "This browser" on a fake browser: the steps in their order, what each
// refusal says, the ID that is kept, and that the subscription, which is a
// secret, is only in the body of the one action that takes it.

import assert from 'node:assert/strict';
import test from 'node:test';

import { pushChannel, PUBLIC_KEY } from './alerts-fixtures.mjs';
import { buttonOf, openAlerts, useStorage } from './alerts-view.mjs';

const words = (node) => (node.children ? node.children.map(words).join(' ') : node.text);
const text = (root) => words(root).replace(/\s+/g, ' ').trim();

const ID_KEY = 'dukovo-admin.alerts.push.id';
const SECRETS = /secret-endpoint|P256-SECRET|AUTH-SECRET/;

/** A browser that can get push: what it was asked is in `calls`. `current` is the subscription it has already. */
function fakeBrowser({ permission = 'granted', registerError = null, current = null } = {}) {
  const calls = [];
  let have = current;
  const subscription = {
    options: { applicationServerKey: new Uint8Array(65) },
    toJSON: () => ({ endpoint: 'https://push.example.net/secret-endpoint', keys: { p256dh: 'P256-SECRET', auth: 'AUTH-SECRET' } }),
    unsubscribe: async () => { calls.push(['unsubscribe']); have = null; return true; },
  };
  const registration = {
    pushManager: {
      getSubscription: async () => have,
      subscribe: async (options) => { calls.push(['subscribe', options]); have = subscription; return subscription; },
    },
  };
  return {
    calls,
    subscription,
    navigator: {
      userAgentData: { brands: [{ brand: 'Not.A/Brand' }, { brand: 'Chromium' }, { brand: 'Google Chrome' }], mobile: false },
      serviceWorker: {
        register: async (url, options) => {
          calls.push(['register', url, options]);
          if (registerError) throw registerError;
          return registration;
        },
        ready: Promise.resolve(registration),
        getRegistration: async () => registration,
      },
    },
    Notification: { requestPermission: async () => { calls.push(['permission']); return permission; } },
  };
}

/** What the page may see once a browser is added: the channel list changes after `alerts.push-add`. */
function pushWorld(added = { id: '0123456789abcdef' }) {
  const state = { added: false };
  return {
    state,
    reads: { '/api/1/alerts/channels': () => (state.added ? [pushChannel({ about: null, name: 'Google Chrome on a computer' })] : []) },
    answers: {
      'alerts.push-key': { ok: true, result: { format: 1, public_key: PUBLIC_KEY } },
      'alerts.push-add': () => {
        state.added = true;
        return { ok: true, result: { done: true, changed: true, output: [JSON.stringify({ format: 1, id: added.id })] } };
      },
    },
  };
}

const everything = (world) => {
  const found = [];
  const walk = (node) => {
    if (node.attributes) found.push(...node.attributes.values());
    if (node.text !== undefined) found.push(node.text);
    node.children?.forEach(walk);
  };
  walk(world.view.main);
  return found.join('\n');
};

test('Get alerts in this browser takes the steps in order and keeps only the ID', async (context) => {
  const kept = useStorage(context);
  const browser = fakeBrowser();
  const world = pushWorld();
  const page = await openAlerts(context, { world, browser });
  await page.press(page.card('browser'), 'Get alerts in this browser');
  assert.deepEqual(browser.calls.map((call) => call[0]), ['register', 'permission', 'subscribe']);
  assert.deepEqual(browser.calls[0].slice(1), ['/sw.js', { scope: '/' }]);
  assert.equal(browser.calls[2][1].userVisibleOnly, true);
  assert.equal(browser.calls[2][1].applicationServerKey.length, 65);
  assert.deepEqual(page.ranWith('alerts.push-key').length, 1);
  assert.deepEqual(page.ran('alerts.push-add'), [{
    subscription: '{"endpoint":"https://push.example.net/secret-endpoint","keys":{"p256dh":"P256-SECRET","auth":"AUTH-SECRET"}}',
    name: 'Google Chrome on a computer',
    severity: 'warning',
    cleared: true,
  }]);
  assert.equal(kept.get(ID_KEY), '"0123456789abcdef"');
  assert.doesNotMatch([...kept.values()].join('\n'), SECRETS);
  assert.doesNotMatch(everything(page), SECRETS);
  assert.doesNotMatch(JSON.stringify(page.notes) + JSON.stringify(page.problems), SECRETS);
  assert.match(text(page.card('browser')), /This browser gets alerts\./);
  assert.ok(buttonOf(page.card('browser'), 'Send a test'));
  assert.ok(buttonOf(page.card('browser'), 'Turn off in this browser'));
  assert.match(text(page.card('channels')), /push · Google Chrome on a computer.*This browser/, 'its row says it is this browser');
});

test('what the person chose is told to the machine, and the kinds only when not every one', async (context) => {
  useStorage(context);
  const browser = fakeBrowser();
  const page = await openAlerts(context, { world: pushWorld(), browser });
  const card = page.card('browser');
  const boxes = card.querySelectorAll('input').filter((one) => one.getAttribute('type') === 'checkbox');
  boxes[1].checked = false;
  card.querySelectorAll('input').find((one) => one.getAttribute('value') === 'critical').checked = true;
  await page.press(card, 'Get alerts in this browser');
  const [added] = page.ran('alerts.push-add');
  assert.equal(added.severity, 'critical');
  assert.equal(added.about, 'stopped,hardware');
});

test('a browser that said no gets words that say where to change it', async (context) => {
  const kept = useStorage(context);
  const browser = fakeBrowser({ permission: 'denied' });
  const page = await openAlerts(context, { world: pushWorld(), browser });
  await page.press(page.card('browser'), 'Get alerts in this browser');
  assert.match(text(page.card('browser')), /This browser said no\. Allow notifications for this site in the browser's settings, then try again\./);
  assert.deepEqual(page.ran('alerts.push-key'), [], 'no key is read and nothing is subscribed');
  assert.equal(browser.calls.some((call) => call[0] === 'subscribe'), false);
  assert.equal(kept.has(ID_KEY), false);
});

test('a worker the browser refuses for the certificate says the two ways', async (context) => {
  useStorage(context);
  const browser = fakeBrowser({ registerError: Object.assign(new Error('The operation is insecure.'), { name: 'SecurityError' }) });
  const page = await openAlerts(context, { world: pushWorld(), browser });
  await page.press(page.card('browser'), 'Get alerts in this browser');
  const card = text(page.card('browser'));
  assert.match(card, /This browser does not trust this server's certificate, so it cannot get push notices\. Two ways:/);
  assert.match(card, /Give the console a certificate from Let's Encrypt/);
  assert.match(card, /Trust this server's own certificate/);
  assert.match(card, /Until then, email and webhooks such as ntfy reach your phone\./);
  assert.equal(page.card('browser').querySelector('.banner a').getAttribute('href'), '/certificates');
  assert.equal(browser.calls.some((call) => call[0] === 'permission'), false, 'permission is not asked of a browser that cannot run the worker');
});

test('another failure of the worker says what the browser said', async (context) => {
  useStorage(context);
  const browser = fakeBrowser({ registerError: new TypeError('Failed to load the script') });
  const page = await openAlerts(context, { world: pushWorld(), browser });
  await page.press(page.card('browser'), 'Get alerts in this browser');
  assert.match(text(page.card('browser')), /could not start its notice service: Failed to load the script/);
  assert.doesNotMatch(text(page.card('browser')), /does not trust/);
});

test('a browser with no push says so, and offers email and webhooks', async (context) => {
  useStorage(context);
  const page = await openAlerts(context, { world: pushWorld() });
  assert.match(text(page.card('browser')), /This browser cannot get push notices\. Email and webhooks such as ntfy reach your phone\./);
  assert.equal(buttonOf(page.card('browser'), 'Get alerts in this browser'), undefined);
});

test('the privacy note is under the card, with the push service\'s part in it', async (context) => {
  useStorage(context);
  const page = await openAlerts(context, { world: pushWorld(), browser: fakeBrowser() });
  assert.match(text(page.card('browser')), /the push service cannot read them\. The push service does see this server's IP address/);
});

test('a name that is empty is refused before anything is asked of the browser', async (context) => {
  useStorage(context);
  const browser = fakeBrowser();
  const page = await openAlerts(context, { world: pushWorld(), browser });
  const name = page.card('browser').querySelector('#browser-name');
  name.value = '   ';
  await page.press(page.card('browser'), 'Get alerts in this browser');
  assert.match(text(page.card('browser')), /Give this browser a name/);
  assert.deepEqual(browser.calls, []);
});

test('a key that is not a push key is refused', async (context) => {
  useStorage(context);
  const browser = fakeBrowser();
  const world = pushWorld();
  world.answers['alerts.push-key'] = { ok: true, result: { format: 1, public_key: 'AAAA' } };
  const page = await openAlerts(context, { world, browser });
  await page.press(page.card('browser'), 'Get alerts in this browser');
  assert.match(text(page.card('browser')), /not a push key/);
  assert.deepEqual(page.ran('alerts.push-add'), []);
});

test('a subscription the browser already has for another key is ended, and a new one made', async (context) => {
  useStorage(context);
  const old = { options: { applicationServerKey: new Uint8Array(65).fill(1) }, unsubscribe: async () => { old.ended = true; } };
  const browser = fakeBrowser({ current: old });
  const page = await openAlerts(context, { world: pushWorld(), browser });
  await page.press(page.card('browser'), 'Get alerts in this browser');
  assert.equal(old.ended, true);
  assert.equal(browser.calls.filter((call) => call[0] === 'subscribe').length, 1);
});

test('Turn off asks the machine to forget this browser, then the browser, and forgets the ID', async (context) => {
  const kept = useStorage(context, { [ID_KEY]: '"0123456789abcdef"' });
  const browser = fakeBrowser({ current: null });
  const page = await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': [pushChannel()] } }, browser });
  assert.match(text(page.card('browser')), /This browser gets alerts\./);
  await page.press(page.card('browser'), 'Turn off in this browser');
  assert.deepEqual(page.ran('alerts.push-remove'), [{ id: '0123456789abcdef' }]);
  assert.equal(kept.has(ID_KEY), false);
});

test('an ID whose subscription the machine no longer lists is forgotten', async (context) => {
  const kept = useStorage(context, { [ID_KEY]: '"0123456789abcdef"' });
  const page = await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': [] } }, browser: fakeBrowser() });
  assert.equal(kept.has(ID_KEY), false);
  assert.ok(buttonOf(page.card('browser'), 'Get alerts in this browser'));
});

test('a read of the channels that failed does not make this browser forget its ID', async (context) => {
  const kept = useStorage(context, { [ID_KEY]: '"0123456789abcdef"' });
  const { ApiError } = await import('../api.js');
  await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': new ApiError({ title: 'no answer', status: 500 }) } }, browser: fakeBrowser() });
  assert.equal(kept.get(ID_KEY), '"0123456789abcdef"');
});

test('removing this browser\'s row lets the browser go of its subscription too', async (context) => {
  const kept = useStorage(context, { [ID_KEY]: '"0123456789abcdef"' });
  const browser = fakeBrowser({ current: null });
  const page = await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': [pushChannel()] } }, browser });
  const row = page.card('channels').querySelectorAll('li').find((one) => text(one).startsWith('push'));
  buttonOf(row, 'Remove').click();
  await page.emit('none');
  page.dialog().querySelector('form').fire('submit');
  await page.emit('none');
  assert.deepEqual(page.ran('alerts.push-remove'), [{ id: '0123456789abcdef' }]);
  assert.equal(kept.has(ID_KEY), false);
});

test('a browser that would not keep the ID says so', async (context) => {
  const kept = useStorage(context);
  Object.defineProperty(globalThis.localStorage, 'setItem', { value: () => { throw new Error('refused'); } });
  const page = await openAlerts(context, { world: pushWorld(), browser: fakeBrowser() });
  await page.press(page.card('browser'), 'Get alerts in this browser');
  assert.match(text(page.card('browser')), /would not keep a note of it/);
  assert.equal(kept.size, 0);
});
