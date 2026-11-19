// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// shell/router.js: a router follows the clicks on the console's own links
// and the browser's back and forward buttons until the shell's signal ends,
// which is when the shell stops. A person who signs out and in again in the
// same tab gets a new router, so the old one must follow nothing more: it
// would show the page with the old session.

import assert from 'node:assert/strict';
import test from 'node:test';

import { usePage } from './fake-dom.mjs';
import { createRouter } from '../shell/router.js';

const tick = () => new Promise((resolve) => setImmediate(resolve));

/**
 * A page whose document and window are event targets, and whose address
 * moves as the History API moves it. Gives `{click(href), popstate(),
 * unload()}`, each of which gives the event that was sent, and `main`.
 */
function useBrowser(context) {
  const page = usePage(context);
  const where = new URL('https://nas.example/');
  const window = new EventTarget();
  const clicks = Object.assign(new EventTarget(), { closest: () => null });
  page.document.addEventListener = clicks.addEventListener.bind(clicks);
  page.document.removeEventListener = clicks.removeEventListener.bind(clicks);
  const was = ['window', 'location', 'history'].map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)]);
  const move = (_, __, path) => { where.href = new URL(path, where).href; };
  Object.defineProperty(globalThis, 'window', { value: window, configurable: true });
  Object.defineProperty(globalThis, 'location', { get: () => where, configurable: true });
  Object.defineProperty(globalThis, 'history', { value: { pushState: move, replaceState: move }, configurable: true });
  context.after(() => {
    for (const [name, descriptor] of was) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  });
  const send = (target, type) => {
    const event = new Event(type, { cancelable: true });
    Object.assign(event, { button: 0, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false });
    target.dispatchEvent(event);
    return event;
  };
  return {
    ...page,
    main: page.document.createElement('main'),
    asof: page.document.createElement('p'),
    click(href) {
      const link = { getAttribute: (name) => (name === 'href' ? href : null), hasAttribute: () => false };
      clicks.closest = () => link;
      return send(clicks, 'click');
    },
    popstate: () => send(window, 'popstate'),
    unload: () => send(window, 'beforeunload'),
  };
}

/**
 * A router over two pages that note each address they show in `shown`, and
 * what a page says when it is left in `leaving`. The shell's `signal` ends
 * with `ending`.
 */
function routerOn(browser, shown, { leaving = null, ending = new AbortController() } = {}) {
  const page = (id, path) => ({
    id, title: id, path, css: [], nav: null, children: [], needs: 'person',
    load: async () => ({ show: async (view) => { shown.push(view.path); view.leaving = leaving; } }),
  });
  return createRouter({
    pages: [page('overview', '/'), page('account', '/account')],
    shell: {
      signal: ending.signal,
      session: () => ({ name: 'alice', admin: true }),
      prefs: { value: { zone: 'Europe/Prague' } },
      commands: { reset() {} },
      closeTransient() {},
      nav: { update() {} },
      bus: { emit() {} },
      makeView: (route) => ({ ...route, head() {} }),
      problem: (error) => { throw error; },
    },
    main: browser.main,
    asof: browser.asof,
  });
}

test('a router follows a click on a link of the console and the back button until the shell stops', async (context) => {
  const browser = useBrowser(context);
  const shown = [];
  const router = routerOn(browser, shown);
  await router.start();
  assert.deepEqual(shown, ['/']);
  browser.click('/account');
  await tick();
  assert.deepEqual(shown, ['/', '/account']);
  browser.popstate();
  await tick();
  assert.deepEqual(shown, ['/', '/account', '/account']);
});

test('a router of a shell that stopped follows no click and no back button, and the new router of the same page does', async (context) => {
  const browser = useBrowser(context);
  const before = [];
  const ending = new AbortController();
  const old = routerOn(browser, before, { ending });
  await old.start();
  ending.abort();
  old.stop();

  const after = [];
  await routerOn(browser, after).start();
  assert.deepEqual([before, after], [['/'], ['/']]);

  const click = browser.click('/account');
  await tick();
  assert.equal(click.defaultPrevented, true, 'the new router took the click');
  assert.deepEqual([before, after], [['/'], ['/', '/account']], 'the old router showed nothing for the click');
  browser.popstate();
  await tick();
  assert.deepEqual([before, after], [['/'], ['/', '/account', '/account']], 'the old router showed nothing for the back button');
});

test('a router of a shell that stopped does not hold the tab for a page that has input it would lose', async (context) => {
  const browser = useBrowser(context);
  const ending = new AbortController();
  const router = routerOn(browser, [], { leaving: () => 'You have not saved it.', ending });
  await router.start();
  assert.equal(browser.unload().defaultPrevented, true, 'a router that runs holds the tab');
  ending.abort();
  router.stop();
  assert.equal(browser.unload().defaultPrevented, false);
});
