// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Add a website (pages/websites/add.js, plan.js, certificate-card.js and
// preview.js): the form and what it says will happen, its faults, the plan
// that is read as an administrator, the files to preview, and a reader.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  closed, list, missing, plan,
} from './websites-fixtures.mjs';
import {
  buttonOf, choose, dialogsOf, fieldOf, linkOf, openWebsites, settle, text, tick, type,
} from './websites-view.mjs';

// The clock is the test's: nothing waits for real time, and a timer that was not stopped is seen.
const clocked = new WeakSet();
const open = (context, more = {}) => {
  if (!clocked.has(context)) {
    clocked.add(context);
    context.mock.timers.enable({ apis: ['setTimeout'] });
  }
  return openWebsites(context, { rest: 'add', ...more });
};

/** Lets the page read the plan of what was typed: the wait after the last key, then what the read starts. */
const planned = async (context) => {
  context.mock.timers.tick(700);
  await settle();
};

/** The rows of What happens, by their keys. */
const rowsOf = (world) => Object.fromEntries(world.view.main.querySelectorAll('.web-rows li').map((one) => [one.getAttribute('data-row'), text(one)]));

/** Types the domain of the shop and waits for the plan. */
async function typeShop(context, world, name = 'shop.example.org') {
  type(fieldOf(world.view.main, 'Domain'), name);
  await planned(context);
}

test('a reader is told who adds a site, with the command, and nothing is read or run', async (context) => {
  const world = await open(context, { session: { admin: false } });
  const { view } = world;
  assert.equal(view.headed.title, 'Add a website');
  assert.deepEqual(view.headed.crumbs, [{ title: 'Apps', path: '/apps' }, { title: 'Websites', path: '/websites' }]);
  assert.match(text(view.main), /Only an administrator can add, change or remove a website\./);
  assert.match(text(view.main), /elevate website add NAME/);
  assert.equal(fieldOf(view.main, 'Domain'), null);
  assert.deepEqual(world.calls, []);
  assert.equal(world.ran.length, 0);
});

test('an administrator reads the sites, the certificates, the accounts, the backups, the snapshots and SSH, and has the form', async (context) => {
  const world = await open(context);
  const { view } = world;
  assert.deepEqual(world.calls.slice(0, 6).sort(), ['/api/1/accounts', '/api/1/backups', '/api/1/certificates', '/api/1/snapshots', '/api/1/ssh', '/api/1/websites']);
  for (const label of ['Domain', 'More names', 'Folder', 'Who may change them']) assert.ok(fieldOf(view.main, label), label);
  assert.deepEqual(view.main.querySelectorAll('section.card h2').map((one) => one.textContent), ['Address', 'What it serves', 'Files', 'Certificate', 'What happens']);
  assert.deepEqual(view.main.querySelectorAll('fieldset legend').map((one) => one.textContent), ['What it serves', 'How the website gets its certificate']);
});

test('what it serves is chosen from two cards, Files first, and a query can choose PHP and a database too', async (context) => {
  const world = await open(context);
  const cards = (root) => root.querySelectorAll('fieldset')[0].querySelectorAll('label.radio-card');
  assert.deepEqual(cards(world.view.main).map((one) => one.querySelector('b').textContent), ['Files from a folder', 'PHP pages']);
  assert.equal(cards(world.view.main)[0].querySelector('input').checked, true);
  const php = await open(context, { query: 'kind=php&database=1' });
  assert.equal(cards(php.view.main)[1].querySelector('input').checked, true);
  assert.equal(fieldOf(php.view.main, 'Also make a PostgreSQL database for it').checked, true);
});

test('the domain fills the folder, the www name and the certificate: Let\'s Encrypt for a name on the internet', async (context) => {
  const world = await open(context);
  const { view } = world;
  type(fieldOf(view.main, 'Domain'), 'Shop.Example.org');
  assert.equal(fieldOf(view.main, 'Folder').value, '/shared/www/sites/shop.example.org');
  const www = fieldOf(view.main, 'Also answer for www.shop.example.org');
  assert.equal(www.checked, true, 'a name that is not the machine\'s has www by default');
  const [card] = view.main.querySelectorAll('fieldset')[1].querySelectorAll('label.radio-card input');
  assert.equal(card.checked, true);
  assert.equal(card.getAttribute('value'), 'letsencrypt');
});

test('the folder the person typed is kept when the domain changes, and www is not ticked again once it was unticked', async (context) => {
  const world = await open(context);
  const { view } = world;
  type(fieldOf(view.main, 'Domain'), 'shop.example.org');
  type(fieldOf(view.main, 'Folder'), '/shared/www/sites/mine');
  tick(fieldOf(view.main, 'Also answer for www.shop.example.org'), false);
  type(fieldOf(view.main, 'Domain'), 'shop2.example.org');
  assert.equal(fieldOf(view.main, 'Folder').value, '/shared/www/sites/mine');
  assert.equal(fieldOf(view.main, 'Also answer for www.shop2.example.org').checked, false);
});

test('a name of this machine is served with this machine\'s certificate, and has no www by default', async (context) => {
  const world = await open(context);
  const { view } = world;
  type(fieldOf(view.main, 'Domain'), 'duke-nas.local');
  const chosen = view.main.querySelectorAll('fieldset')[1].querySelectorAll('label.radio-card input').find((one) => one.checked);
  assert.equal(chosen.getAttribute('value'), 'machine');
  assert.equal(fieldOf(view.main, 'Also answer for www.duke-nas.local').checked, false);
});

test('a name with no dot cannot have a certificate from Let\'s Encrypt, and says why', async (context) => {
  const world = await open(context);
  type(fieldOf(world.view.main, 'Domain'), 'intranet');
  const cards = world.view.main.querySelectorAll('fieldset')[1].querySelectorAll('label.radio-card');
  const first = cards[0];
  assert.equal(first.querySelector('input').disabled, true);
  assert.match(text(first), /Let's Encrypt gives certificates to names with a dot, and intranet has none\./);
  assert.equal(cards.find((one) => one.querySelector('input').checked).querySelector('b').textContent, 'None, HTTP only');
});

test('the name is asked again by the plan after the last key and not before, as an administrator', async (context) => {
  const world = await open(context);
  type(fieldOf(world.view.main, 'Domain'), 'shop.example.org');
  assert.equal(world.ranOf('websites.plan').length, 0);
  context.mock.timers.tick(300);
  type(fieldOf(world.view.main, 'Domain'), 'shop.example.org2');
  context.mock.timers.tick(400);
  await settle();
  assert.equal(world.ranOf('websites.plan').length, 0, 'the last key began the wait again');
  await planned(context);
  const [[args, options]] = world.ranOf('websites.plan');
  assert.deepEqual(args, { name: 'shop.example.org2', names: 'www.shop.example.org2', kind: 'static', owner: 'chase' });
  assert.equal(options.words.unlock, 'plan the website');
});

test('What happens says each thing the add does: the certificate, the folder, the site, the restart, the ports and the backups', async (context) => {
  const world = await open(context);
  await typeShop(context, world);
  const rows = rowsOf(world);
  assert.match(rows.certificate, /Gets a certificate for shop\.example\.org and www\.shop\.example\.org from Let's Encrypt; certd renews it by itself\./);
  assert.match(rows.folder, /Makes \/shared\/www\/sites\/shop\.example\.org with a starter page, owned by chase\./);
  assert.match(rows.site, /Adds nginx's site for both names, then turns HTTPS on once the certificate is got/);
  assert.match(rows.restart, /Restarts nginx once/);
  assert.match(rows.firewall, /ports 80 and 443 reach nginx/);
  assert.match(rows.backups, /\/shared is in the nightly backup to nas/);
  assert.match(rows.snapshots, /\/shared has a snapshot every day/);
});

test('the ports are asked of the firewall from outside, and a closed one has a button that opens it for a while', async (context) => {
  const world = await open(context, {
    world: { runs: { 'firewall.test': (args) => ({ ok: true, result: args.port === 443 ? closed() : { allowed: true, rule: null, text: '', passed: [] } }) } },
  });
  await typeShop(context, world);
  const asked = world.ranOf('firewall.test').map(([args]) => args);
  assert.deepEqual(asked, [{ protocol: 'tcp', port: 80, from: '203.0.113.1' }, { protocol: 'tcp', port: 443, from: '203.0.113.1' }]);
  const press = world.view.main.querySelector('[data-action="open-port-443"]');
  assert.ok(press, 'Open port 443');
  assert.equal(world.view.main.querySelector('[data-action="open-port-80"]'), null);
  await world.press(press);
  const [[args]] = world.ranOf('firewall.add');
  assert.deepEqual(args, { action: 'allow', protocol: 'tcp', ports: '443', try: 600 });
});

test('a plan that was read says what is wrong in words, and the add waits until it is put right', async (context) => {
  const failing = plan({
    checks: [{ id: 'WEB-101', result: 'failed', words: 'Another site answers to shop.example.org' }, { id: null, result: 'warning', words: 'Its folder is not empty' }],
  });
  const world = await open(context, { world: { runs: { 'websites.plan': { ok: true, result: failing } } } });
  await typeShop(context, world);
  const checks = world.view.main.querySelector('.web-checks');
  assert.match(text(checks), /Another site answers to shop\.example\.org WEB-101/);
  assert.match(text(checks), /Its folder is not empty/);
  const add = buttonOf(world.view.main, 'Add the website');
  assert.equal(add.getAttribute('aria-disabled'), 'true');
  assert.match(text(world.view.main.querySelector('#plan')), /Put right what the plan says first\./);
  await world.press(add);
  assert.equal(world.ranOf('websites.add').length, 0);
});

test('a locked console reads no plan: the card says so and Unlock asks first', async (context) => {
  const world = await open(context, { unlocked: false });
  await typeShop(context, world);
  assert.equal(world.ranOf('websites.plan').length, 0);
  const unlock = world.view.main.querySelector('.web-unlock');
  assert.equal(unlock.hidden, false);
  assert.match(text(unlock), /read as an administrator\. Unlock to see them\./);
  assert.ok(buttonOf(unlock, 'Unlock'));
  assert.ok(Object.keys(rowsOf(world)).includes('certificate'), 'the rows the page can tell without the plan are there');
});

test('Preview the files first reads the plan as an administrator and opens each file with its lines numbered', async (context) => {
  const world = await open(context);
  await typeShop(context, world);
  const before = world.ranOf('websites.plan').length;
  await world.press(buttonOf(world.view.main, 'Preview the files first'));
  assert.equal(world.ranOf('websites.plan').length, before + 1);
  const [dialog] = dialogsOf(world.document);
  assert.ok(dialog, 'the files are shown');
  assert.equal(dialog.querySelector('h2').textContent, 'The files that adding the site writes');
  const files = dialog.querySelectorAll('section.web-file');
  assert.deepEqual(files.map((one) => one.getAttribute('data-path')), [
    '/system/settings/websites/shop.example.org.conf', '/shared/www/sites/shop.example.org/index.php',
  ]);
  assert.match(text(files[0]), /new/);
  assert.equal(files[0].querySelectorAll('.web-line')[0].querySelector('.web-no').textContent, '1');
  assert.match(files[0].querySelectorAll('.web-line')[0].textContent, /# website: shop\.example\.org/);
  assert.match(text(dialog), /Only a preview: nothing is written until Add the website is pressed\./);
  assert.equal(world.ranOf('websites.add').length, 0, 'nothing was added');
});

test('Preview the files first with no domain says what is missing and reads nothing', async (context) => {
  const world = await open(context);
  await world.press(buttonOf(world.view.main, 'Preview the files first'));
  assert.equal(dialogsOf(world.document).length, 0);
  assert.equal(world.ranOf('websites.plan').length, 0);
});

test('a name that is not one is said under the domain, and so is a folder that is not under /shared/www/sites', async (context) => {
  const world = await open(context);
  const { view } = world;
  type(fieldOf(view.main, 'Domain'), 'Shop Example');
  fieldOf(view.main, 'Domain').fire('change');
  assert.ok(view.main.querySelector('.field .msg'), 'a fault is under the domain');
  type(fieldOf(view.main, 'Domain'), 'shop.example.org');
  type(fieldOf(view.main, 'Folder'), '/etc');
  await world.press(buttonOf(view.main, 'Add the website'));
  assert.equal(world.ranOf('websites.add').length, 0);
  assert.match(text(view.main.querySelectorAll('.field').find((one) => one.querySelector('label')?.textContent === 'Folder')), /under \/shared\/www\/sites/);
});

test('the terms must be agreed to get a certificate, and the fault is under the box', async (context) => {
  const world = await open(context);
  await typeShop(context, world);
  await world.press(buttonOf(world.view.main, 'Add the website'));
  assert.equal(world.ranOf('websites.add').length, 0);
  assert.match(text(world.view.main), /Agree to Let's Encrypt's terms of service to get a certificate\./);
});

test('the checks of the names are asked of certd for names with a dot, once the keys stop', async (context) => {
  const world = await open(context);
  await typeShop(context, world);
  assert.ok(world.calls.includes('/api/1/certificates/check/shop.example.org%2Cwww.shop.example.org'), world.calls.join(' | '));
  const list = world.view.main.querySelector('.web-name-checks');
  assert.match(text(list), /shop\.example\.org leads to 203\.0\.113\.10/);
  const terms = linkOf(world.view.main, 'I agree to Let\'s Encrypt\'s terms of service');
  assert.equal(terms?.getAttribute('href'), 'https://letsencrypt.org/documents/LE-SA-v1.5-February-24-2025.pdf', 'the terms are a link once certd gives it');
});

test('apps that are missing are installed first: the sheet is read and the plan says so', async (context) => {
  const world = await open(context, { world: { list: missing() } });
  await typeShop(context, world);
  assert.ok(world.calls.includes('/api/1/apps/catalog/org.nginx.nginx/sheet'));
  assert.match(rowsOf(world)['install-nginx'] ?? '', /Installs nginx first, from the catalog\./);
  assert.equal(linkOf(world.view.main, 'Cancel').getAttribute('href'), '/websites');
});

test('a sheet that cannot be read stops the add and says which app', async (context) => {
  const world = await open(context, { world: { list: missing(), reads: { '/api/1/apps/catalog/*': () => { throw new Error('no'); } } } });
  await typeShop(context, world);
  assert.match(text(world.view.main.querySelector('#plan')), /The sheet of nginx could not be read/);
  assert.equal(buttonOf(world.view.main, 'Add the website').getAttribute('aria-disabled'), 'true');
});

test('with commands shown, What happens lists the command of each step once, and again only when the steps change', async (context) => {
  const world = await open(context, { prefs: { commands: 'shown' } });
  assert.match(text(world.view.main.querySelector('.web-runs')), /The commands show when every field is right\./);
  await typeShop(context, world);
  const steps = () => world.panels.map(([action]) => action);
  assert.deepEqual(steps(), ['websites.add', 'certificate.get', 'websites.https']);
  tick(fieldOf(world.view.main, 'I agree to'), true);
  await planned(context);
  assert.equal(steps().length, 3, 'the same steps are not drawn again');
  choose(world.view.main, 'PHP pages');
  await planned(context);
  assert.equal(steps().length, 6);
  assert.equal(world.panels.at(-3)[1].kind, 'php');
});

test('the commands are hidden while commands are hidden', async (context) => {
  const world = await open(context);
  await typeShop(context, world);
  assert.equal(world.view.main.querySelector('.web-runs').hidden, true);
  assert.equal(world.panels.length, 0);
});
