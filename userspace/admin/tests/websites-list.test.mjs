// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The list of websites: what it reads, each site's row in words, nginx and
// PHP-FPM when they do not run, a reader, and Check the sites.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  checkBroken, checkOk, empty, list, missing, off, phpOff, problem,
} from './websites-fixtures.mjs';
import {
  buttonOf, linkOf, openWebsites, refusal, text,
} from './websites-view.mjs';

test('the list reads the sites and the certificates, and the head names Apps and has Add a website for an administrator', async (context) => {
  const world = await openWebsites(context);
  assert.deepEqual(world.calls, ['/api/1/websites', '/api/1/certificates']);
  const head = world.view.headed;
  assert.equal(head.title, 'Websites');
  assert.deepEqual(head.crumbs, [{ title: 'Apps', path: '/apps' }]);
  assert.equal(head.commands, true);
  assert.deepEqual(head.buttons.map((one) => text(one)), ['Add a website', 'Check the sites']);
  assert.equal(head.buttons[0].getAttribute('href'), '/websites/add');
});

test('each site is a row with its first name linked, the others under it, what it serves, its HTTPS and its state', async (context) => {
  const world = await openWebsites(context);
  const rows = world.view.main.querySelectorAll('tbody tr');
  assert.deepEqual(rows.map((row) => row.getAttribute('data-name')), ['shop.example.org', 'blog.example.org', 'duke-nas.local', 'legacy']);
  const shop = rows[0];
  assert.equal(shop.querySelector('a').getAttribute('href'), '/websites/shop.example.org');
  const cells = shop.querySelectorAll('td').map((cell) => text(cell));
  assert.deepEqual([cells[0].startsWith('shop.example.org'), cells[0].endsWith('www.shop.example.org')], [true, true]);
  assert.equal(shop.querySelector('small').textContent, 'www.shop.example.org');
  assert.equal(cells[1], 'PHP pages/shared/www/sites/shop.example.org');
  assert.equal(cells[2], 'Let\'s Encrypt · renews Dec 30');
  assert.equal(cells[3], 'Answers');
  assert.equal(text(rows[1].querySelectorAll('td')[2]), 'HTTP only');
  assert.equal(rows[1].querySelectorAll('td')[2].classes.has('web-tone-warn'), true, 'HTTP only is in the warning colour');
  assert.equal(text(rows[2].querySelectorAll('td')[2]), 'This machine\'s certificate');
  assert.equal(text(rows[3].querySelectorAll('td')[1]), 'Set up by hand');
});

test('a site changed by hand wears a pill that says it is kept as it was written', async (context) => {
  const world = await openWebsites(context);
  const pill = world.view.main.querySelector('tr[data-name="legacy"] .pill');
  assert.equal(text(pill), 'Changed by hand');
  assert.equal(pill.getAttribute('title'), 'Kept as you wrote it');
  assert.equal(world.view.main.querySelector('tr[data-name="shop.example.org"] .pill'), null);
});

test('a site with a problem anyone can see says it in its State column', async (context) => {
  const world = await openWebsites(context, { world: { list: problem() } });
  const state = world.view.main.querySelector('tbody tr').querySelectorAll('td')[3];
  assert.equal(text(state), 'Its folder /shared/www/sites/shop.example.org is not there');
  assert.equal(state.querySelector('.st').classes.has('warn'), true);
});

test('with the certificates unreadable the list still says which certificate each site has', async (context) => {
  const world = await openWebsites(context, { world: { reads: { '/api/1/certificates': refusal('ADM-123', 'no', 422) } } });
  assert.equal(text(world.view.main.querySelector('tr[data-name="shop.example.org"]').querySelectorAll('td')[2]), 'Certificate shop.example.org');
  assert.equal(world.problems.length, 0, 'nothing is shown of what the page does not need');
});

test('nginx that does not run is a line above the list with Turn it on, and the command', async (context) => {
  const world = await openWebsites(context, { world: { list: off() } });
  const line = world.view.main.querySelector('[data-line="nginx"]');
  assert.match(text(line), /^nginx is off: no site answers\./);
  assert.deepEqual(world.inlines.at(-1), ['websites.list', {}]);
  assert.ok(world.inlines.some(([action, args]) => action === 'service.enable' && args.name === 'nginx'));
  await world.press(buttonOf(line, 'Turn it on'));
  assert.deepEqual(world.ranOf('service.enable').map(([args]) => args), [{ name: 'nginx' }]);
  assert.equal(world.calls.filter((one) => one === '/api/1/websites').length, 2, 'the list is read again');
});

test('PHP-FPM that does not run is a line when a PHP site is there', async (context) => {
  const world = await openWebsites(context, { world: { list: phpOff() } });
  assert.match(text(world.view.main.querySelector('[data-line="php-fpm"]')), /^PHP-FPM is off: its PHP pages fail\./);
  assert.match(text(world.view.main.querySelector('tr[data-name="shop.example.org"]').querySelectorAll('td')[3]), /PHP-FPM is off/);
  await world.press(buttonOf(world.view.main.querySelector('[data-line="php-fpm"]'), 'Turn it on'));
  assert.deepEqual(world.ranOf('service.enable').map(([args]) => args), [{ name: 'php-fpm' }]);
});

test('nginx that is not installed says so on every site and offers no button for what only the apps do', async (context) => {
  const world = await openWebsites(context, { world: { list: { ...missing(), websites: list().websites } } });
  assert.match(text(world.view.main.querySelector('[data-line="nginx"]')), /^nginx is not installed: no site answers\./);
  assert.equal(buttonOf(world.view.main.querySelector('[data-line="nginx"]'), 'Turn it on'), undefined);
  assert.equal(text(world.view.main.querySelector('tbody tr').querySelectorAll('td')[3]), 'nginx is not installed');
});

test('no site yet says so, with Add a website for an administrator', async (context) => {
  const world = await openWebsites(context, { world: { list: empty() } });
  const none = world.view.main.querySelector('.web-empty');
  assert.match(text(none), /^No websites yet\./);
  assert.equal(linkOf(none, 'Add a website').getAttribute('href'), '/websites/add');
  assert.equal(world.view.main.querySelector('table'), null);
});

test('a reader sees the list with no button, and for no site the command to type', async (context) => {
  const world = await openWebsites(context, { session: { admin: false } });
  assert.deepEqual(world.view.headed.buttons, []);
  assert.match(text(world.view.main), /Only an administrator can add, change or remove a website\./);
  assert.equal(buttonOf(world.view.main, 'Check the sites'), undefined);
  assert.equal(world.view.main.querySelectorAll('tbody tr').length, 4);
  const bare = await openWebsites(context, { session: { admin: false }, world: { list: empty() } });
  assert.match(text(bare.view.main.querySelector('.web-empty')), /^No websites yet\.An administrator adds one\. At a terminal:elevate website add NAME/);
  assert.equal(linkOf(bare.view.main, 'Add a website'), undefined);
});

test('a list that cannot be read says so, with Try again that reads it again', async (context) => {
  const world = await openWebsites(context, { world: { reads: { '/api/1/websites': refusal('ADM-123', 'The command failed', 422) } } });
  assert.match(text(world.view.main), /The websites could not be read\./);
  assert.equal(world.problems.length, 1);
  world.state.list = list();
  world.listeners.length = 0;
  const before = world.calls.length;
  await world.press(buttonOf(world.view.main, 'Read again'));
  assert.ok(world.calls.length > before);
});

test('Check the sites asks nginx as an administrator and shows its line and the problems it found', async (context) => {
  const world = await openWebsites(context, { world: { runs: { 'websites.check': { ok: true, result: checkBroken() } } } });
  await world.press(world.view.headed.buttons[1]);
  const [[args, options]] = world.ranOf('websites.check');
  assert.deepEqual(args, {});
  assert.equal(options.words.unlock, 'check the websites');
  const shown = text(world.view.main);
  assert.match(shown, /nginx's check of the sites failed, so nothing is changed until it passes\./);
  assert.match(shown, /unknown directive "listn" in \/system\/settings\/websites\/blog\.example\.org\.conf:3/);
  assert.match(shown, /do not include the websites' folder/);
  assert.match(shown, /include \/system\/settings\/websites\/\*\.conf;/);
  assert.match(shown, /location \^~ \/sites\/ \{/);
  assert.match(shown, /What WEB-102 means/);
  const blog = world.view.main.querySelector('tr[data-name="blog.example.org"]').querySelectorAll('td')[3];
  assert.match(text(blog), /nginx refuses its file: unknown directive "listn"WEB-103$/);
  assert.equal(blog.querySelector('.st').classes.has('bad'), true);
});

test('a check that passes says so, and a refusal of the check goes where the page puts problems', async (context) => {
  const world = await openWebsites(context, { world: { runs: { 'websites.check': { ok: true, result: checkOk() } } } });
  await world.press(world.view.headed.buttons[1]);
  assert.match(text(world.view.main.querySelector('.web-check')), /nginx's check of the sites passed\./);
  assert.equal(world.view.main.querySelectorAll('.web-check pre').length, 0);
  const refused = await openWebsites(context, { world: { runs: { 'websites.check': refusal('ADM-122', 'Not allowed', 403) } } });
  await refused.press(refused.view.headed.buttons[1]);
  assert.equal(refused.problems.length, 1);
  assert.equal(refused.view.main.querySelector('.web-check').children.length, 0);
});
