// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The steps of Add a website (pages/websites/run.js): the apps that are
// missing first, the site, and for Let's Encrypt the certificate and HTTPS;
// what each failure leaves standing and the way on from it.

import assert from 'node:assert/strict';
import test from 'node:test';

import { failedTask, finished, list, missing } from './websites-fixtures.mjs';
import {
  buttonOf, choose, fieldOf, linkOf, openWebsites, refusal, settle, text, tick, type,
} from './websites-view.mjs';

const clocked = new WeakSet();
const open = (context, more = {}) => {
  if (!clocked.has(context)) {
    clocked.add(context);
    context.mock.timers.enable({ apis: ['setTimeout'] });
  }
  return openWebsites(context, { rest: 'add', ...more });
};

/** A change that starts the task `id`. */
const starts = (id) => ({ ok: true, task: id, result: { done: true, changed: true, output: [] } });

/** Types the shop's domain and, for Let's Encrypt, agrees to the terms; waits for the plan. */
async function fill(context, world, { name = 'shop.example.org', agree = true } = {}) {
  type(fieldOf(world.view.main, 'Domain'), name);
  if (agree) tick(fieldOf(world.view.main, 'I agree to'), true);
  context.mock.timers.tick(700);
  await settle();
}

/** Presses Add the website and waits for the steps. */
async function add(world) {
  await world.press(buttonOf(world.view.main, 'Add the website'));
  await settle();
}

const titles = (world) => world.view.main.querySelectorAll('.flow-step b').map((one) => one.textContent);
const states = (world) => world.view.main.querySelectorAll('.flow-step').map((one) => one.getAttribute('data-state'));
const banners = (world) => text(world.view.main.querySelector('.web-after'));

test('Let\'s Encrypt is three steps: the site over HTTP, the certificate, then HTTPS, each as an administrator with its own words', async (context) => {
  const world = await open(context, { world: { runs: { 'websites.add': starts(11), 'certificate.get': starts(12), 'websites.https': starts(13) } } });
  await fill(context, world);
  await add(world);
  assert.deepEqual(world.ran.filter(([action]) => action !== 'websites.plan' && action !== 'firewall.test').map(([action, args]) => [action, args]), [
    ['websites.add', { name: 'shop.example.org', kind: 'static', owner: 'chase', names: 'www.shop.example.org' }],
    ['certificate.get', { name: 'shop.example.org', names: 'shop.example.org\nwww.shop.example.org', agree_terms: true }],
    ['websites.https', { name: 'shop.example.org', certificate: 'shop.example.org' }],
  ]);
  const [[, , options]] = world.ran.filter(([action]) => action === 'websites.add');
  assert.equal(options.words.unlock, 'add the website shop.example.org');
  assert.equal(world.view.main.querySelector('h2#web-run-h').textContent, 'Adding shop.example.org');
  assert.deepEqual(titles(world), ['Add shop.example.org', 'Get the certificate for shop.example.org', 'Serve shop.example.org over HTTPS']);
  assert.deepEqual(states(world), ['done', 'done', 'done']);
});

test('when it is all done the site is live at its address, with its page and Add another', async (context) => {
  const world = await open(context, { world: { runs: { 'websites.add': starts(11), 'certificate.get': starts(12), 'websites.https': starts(13) } } });
  await fill(context, world);
  await add(world);
  assert.match(banners(world), /shop\.example\.org is live: https:\/\/shop\.example\.org\//);
  assert.equal(linkOf(world.view.main, 'https://shop.example.org/').getAttribute('href'), 'https://shop.example.org/');
  assert.equal(linkOf(world.view.main, 'Open its page').getAttribute('href'), '/websites/shop.example.org');
  assert.equal(linkOf(world.view.main, 'Add another').getAttribute('href'), '/websites/add');
  assert.equal(linkOf(world.view.main, 'Make its database'), undefined, 'a database only when it was asked for');
  assert.equal(world.view.main.querySelector('.web-add').hidden, true, 'the form is out of the way while the steps run');
});

test('a certificate that cannot be got leaves the site live over HTTP, and its page has the way to try again', async (context) => {
  const world = await open(context, {
    world: {
      runs: { 'websites.add': starts(11), 'certificate.get': starts(12), 'websites.https': starts(13) },
      reads: { '/api/1/tasks/12': failedTask(12, 'cert: the authority could not reach shop.example.org on port 80 (CRT-102)') },
    },
  });
  await fill(context, world);
  await add(world);
  assert.deepEqual(states(world), ['done', 'failed', 'skipped']);
  assert.equal(world.ranOf('websites.https').length, 0, 'HTTPS is not turned on without the certificate');
  assert.match(banners(world), /shop\.example\.org is live over HTTP\. The certificate was not got: the site's page says whether certd tries again, and has Get a certificate when it does not\./);
  assert.equal(buttonOf(world.view.main, 'Try again'), undefined, 'trying the certificate again is on the site\'s page');
  assert.equal(linkOf(world.view.main, 'Open its page').getAttribute('href'), '/websites/shop.example.org');
  assert.equal(buttonOf(world.view.main, 'Change the form'), undefined, 'the site is there: the form would add it again');
  const [error] = world.problems.at(-1);
  assert.equal(error.id, 'CRT-102');
  assert.equal(error.title, 'The certificate for shop.example.org was not got');
});

test('HTTPS that could not be turned on says the certificate is kept and Try again starts at that step', async (context) => {
  const world = await open(context, {
    world: {
      runs: { 'websites.add': starts(11), 'certificate.get': starts(12), 'websites.https': starts(13) },
      reads: { '/api/1/tasks/13': failedTask(13, 'website: nginx refuses its file (WEB-103)') },
    },
  });
  await fill(context, world);
  await add(world);
  assert.deepEqual(states(world), ['done', 'done', 'failed']);
  assert.match(banners(world), /shop\.example\.org is live over HTTP, and certd keeps the certificate shop\.example\.org\. Try again turns HTTPS on\./);
  const again = buttonOf(world.view.main, 'Try again');
  assert.ok(again);
  world.ran.length = 0;
  await world.press(again);
  assert.deepEqual(world.ran.map(([action]) => action), ['websites.https'], 'only the step that failed runs again');
  assert.match(text(world.view.main.querySelector('.web-run-slot')), /Done already: Add shop\.example\.org, Get the certificate for shop\.example\.org\./);
});

test('a site that was not added says nothing was changed, and the form comes back with what was typed', async (context) => {
  const world = await open(context, { world: { runs: { 'websites.add': refusal('WEB-101', 'Another site answers to shop.example.org') } } });
  await fill(context, world);
  await add(world);
  assert.deepEqual(states(world), ['failed', 'skipped', 'skipped']);
  assert.match(banners(world), /Nothing was changed: the command took its changes back\./);
  assert.ok(buttonOf(world.view.main, 'Try again'));
  assert.equal(world.view.main.querySelector('.web-add').hidden, true);
  await world.press(buttonOf(world.view.main, 'Change the form'));
  assert.equal(world.view.main.querySelector('.web-add').hidden, false);
  assert.equal(fieldOf(world.view.main, 'Domain').value, 'shop.example.org');
  assert.equal(world.view.main.querySelector('.web-run-slot').children.length, 0);
});

test('Try again after a site that was not added runs from the first step that failed', async (context) => {
  let tries = 0;
  const world = await open(context, {
    world: { runs: { 'websites.add': () => (tries++ === 0 ? refusal('WEB-103', 'nginx refuses its file') : starts(11)), 'certificate.get': starts(12), 'websites.https': starts(13) } },
  });
  await fill(context, world);
  await add(world);
  await world.press(buttonOf(world.view.main, 'Try again'));
  assert.deepEqual(world.ran.filter(([action]) => action.startsWith('websites.add') || action === 'certificate.get' || action === 'websites.https').map(([action]) => action),
    ['websites.add', 'websites.add', 'certificate.get', 'websites.https']);
  assert.match(banners(world), /is live/);
});

test('with no certificate it is one step, and the site is live over HTTP', async (context) => {
  const world = await open(context, { world: { runs: { 'websites.add': starts(11) } } });
  await fill(context, world, { agree: false });
  choose(world.view.main, 'None, HTTP only');
  await add(world);
  assert.deepEqual(titles(world), ['Add shop.example.org']);
  assert.match(banners(world), /shop\.example\.org is live: http:\/\/shop\.example\.org\//);
  const [[, args]] = world.ran.filter(([action]) => action === 'websites.add');
  assert.equal('certificate' in args, false);
});

test('this machine\'s certificate is sent with the site and no certificate is got; the browser warns until it trusts it', async (context) => {
  const world = await open(context, { world: { runs: { 'websites.add': starts(11) } } });
  await fill(context, world, { name: 'duke-nas.local', agree: false });
  await add(world);
  const [[, args]] = world.ran.filter(([action]) => action === 'websites.add');
  assert.deepEqual(args, {
    name: 'duke-nas.local', kind: 'static', owner: 'chase', certificate: 'this-machine',
  });
  assert.deepEqual(titles(world), ['Add duke-nas.local']);
  assert.match(banners(world), /duke-nas\.local is live: https:\/\/duke-nas\.local\//);
  assert.match(banners(world), /Browsers warn until they trust this machine's certificate; Certificates says how\./);
});

test('PHP with a database ends with a way to make the database for the site', async (context) => {
  const world = await open(context, { query: 'kind=php&database=1', world: { runs: { 'websites.add': starts(11), 'certificate.get': starts(12), 'websites.https': starts(13) } } });
  await fill(context, world);
  await add(world);
  assert.equal(linkOf(world.view.main, 'Make its database').getAttribute('href'), '/databases/add?site=shop.example.org');
  const [[, args]] = world.ran.filter(([action]) => action === 'websites.add');
  assert.equal(args.kind, 'php');
});

test('apps that are missing are installed first, each held to the hash of its sheet', async (context) => {
  const world = await open(context, {
    query: 'kind=php',
    world: { list: missing(), runs: { 'apps.install': starts(10), 'websites.add': starts(11), 'certificate.get': starts(12), 'websites.https': starts(13) } },
  });
  await fill(context, world);
  await add(world);
  assert.deepEqual(world.ran.filter(([action]) => action === 'apps.install').map(([, args]) => args), [
    { id: 'org.nginx.nginx', sha256: 'a'.repeat(64) }, { id: 'net.php.php', sha256: 'b'.repeat(64) },
  ]);
  assert.deepEqual(titles(world).slice(0, 3), ['Install nginx', 'Install PHP-FPM', 'Add shop.example.org']);
  assert.ok(world.ran.map(([action]) => action).indexOf('apps.install') < world.ran.map(([action]) => action).indexOf('websites.add'));
});

test('an install that fails stops the rest and says that nothing about the website was changed', async (context) => {
  const world = await open(context, { world: { list: missing(), runs: { 'apps.install': refusal('APP-104', 'The sheet changed') } } });
  await fill(context, world);
  await add(world);
  assert.equal(world.ranOf('websites.add').length, 0);
  assert.match(banners(world), /Nothing about the website was changed\./);
});

test('leaving the page while the steps run draws nothing after', async (context) => {
  const world = await open(context, { world: { runs: { 'websites.add': starts(11), 'certificate.get': starts(12), 'websites.https': starts(13) } } });
  await fill(context, world);
  world.view.main.querySelector('[data-action="add"]').click();
  world.leave();
  await settle();
  assert.equal(world.view.main.querySelector('.web-after').children.length, 0);
});
