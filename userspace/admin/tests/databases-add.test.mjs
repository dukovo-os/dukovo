// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Add a database (pages/databases/add.js, add-run.js and once.js): the form
// and what it says will happen, its faults, the steps that run, and the
// password that is shown once and left nowhere.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  added, finished, list, missing, PASSWORD, role,
} from './databases-fixtures.mjs';
import {
  buttonOf, choose, dialogsOf, fieldOf, holdings, openDatabases, refusal, settle, type,
} from './databases-view.mjs';

// The clock is the test's: nothing waits for real time, and a timer that was not stopped is seen.
const clocked = new WeakSet();
const open = (context, more = {}) => {
  if (!clocked.has(context)) {
    clocked.add(context);
    context.mock.timers.enable({ apis: ['setTimeout'] });
  }
  return openDatabases(context, { rest: 'add', ...more });
};

/** The Add button. */
const addButton = (world) => buttonOf(world.view.main, 'Add the database');

/** Everything that holds words on the page and in its dialogs, for what a password must not be in. */
const everything = (world) => holdings(world.document.body);

/** Presses Add the database and waits for what it began. */
async function pressAdd(world) {
  addButton(world).click();
  await settle();
}

/** The answer of `databases.add` for `name`. */
const adds = (name, more) => ({ ok: true, result: added(name, more) });

test('the form has the name, the role, the text and sorting, and the three ways to connect, the first chosen', async (context) => {
  const world = await open(context);
  const { view } = world;
  assert.equal(view.headed.title, 'Add a database');
  assert.deepEqual(view.headed.crumbs, [{ title: 'Apps', path: '/apps' }, { title: 'Databases', path: '/databases' }]);
  assert.equal(view.headed.commands, true);
  for (const label of ['Name', 'Owned by', 'Text and sorting']) assert.ok(fieldOf(view.main, label), label);
  assert.match(view.main.textContent, /A new PostgreSQL role, with rights over this database only\./);
  assert.equal(fieldOf(view.main, 'Text and sorting').value, 'en_US.UTF-8');
  assert.match(view.main.textContent, /If the system's language data changes in an update, PostgreSQL asks for its indexes to be rebuilt/);
  const titles = view.main.querySelectorAll('label.radio-card b').map((one) => one.textContent);
  assert.deepEqual(titles, ['Programs on this server, with a password', 'An account of this server, by the socket', 'Programs on other machines, with a password']);
  assert.equal(view.main.querySelector('fieldset legend').textContent, 'Who connects');
  const [local] = view.main.querySelectorAll('label.radio-card input');
  assert.equal(local.checked, true);
  assert.match(view.main.textContent, /A PostgreSQL 19 database and the role that owns it\. Snapshots and backups copy it with \/shared\./);
});

test('what it reads besides the databases is the backups, the certificates, the accounts and the machine\'s name', async (context) => {
  const world = await open(context);
  assert.deepEqual(world.calls.sort(), ['/api/1/accounts', '/api/1/backups', '/api/1/certificates', '/api/1/machine/name']);
  assert.equal(world.timers.length, 0);
});

test('Backups says what the hook does and which place copies /shared, or that none does', async (context) => {
  const { view } = await open(context);
  assert.match(view.main.textContent, /\/shared is in the nightly backup to nas\./);
  const none = await open(context, { world: { reads: { '/api/1/backups': { places: [] } } } });
  assert.match(none.view.main.textContent, /No backup place copies \/shared yet\./);
  assert.equal(none.view.main.querySelectorAll('a').find((one) => one.getAttribute('href') === '/snapshots/backups').textContent, 'Add a backup place');
});

test('what happens is drawn for the way chosen, and changes with the name', async (context) => {
  const world = await open(context);
  const { view } = world;
  type(fieldOf(view.main, 'Name'), 'shop2');
  const words = () => view.main.querySelectorAll('.db-plan-rows li').map((one) => one.textContent);
  assert.equal(words()[0], 'Makes the PostgreSQL role shop2, which signs in with a password, from this server only');
  assert.match(words()[1], /^Makes the database shop2, owned by shop2, UTF-8, sorted as English \(US\)\./);
  assert.match(words()[2], /^Firewall: nothing to change\. PostgreSQL listens on the network already, and only programs on this server may sign in to this database\.$/);
  assert.match(words()[3], /none; the database is ready at once/);
  choose(view.main, 'other machines');
  type(fieldOf(view.main, 'Addresses'), '192.168.1.0/24');
  assert.equal(words()[0], 'Makes the PostgreSQL role shop2, which signs in with a password, from 192.168.1.0/24 over TLS');
  assert.match(words()[2], /Port 5432 is open to every address while PostgreSQL runs/);
  assert.match(words()[3], /none; the database is ready at once/);
});

test('the role follows the name until it is typed, and is the account\'s for the socket', async (context) => {
  const { view } = await open(context);
  type(fieldOf(view.main, 'Name'), 'abc');
  assert.equal(fieldOf(view.main, 'Owned by').value, 'abc');
  type(fieldOf(view.main, 'Owned by'), 'owner');
  type(fieldOf(view.main, 'Name'), 'abcd');
  assert.equal(fieldOf(view.main, 'Owned by').value, 'owner');
  choose(view.main, 'An account of this server');
  assert.equal(fieldOf(view.main, 'Owned by').disabled, true);
  assert.equal(fieldOf(view.main, 'Owned by').value, 'chase');
  const account = fieldOf(view.main, 'Account');
  assert.deepEqual(account.querySelectorAll('option').map((one) => one.getAttribute('value')), ['chase', 'webdev']);
  assert.match(view.main.textContent, /1 account is not offered: the name of a role is made of lowercase letters, digits and _\./);
  account.value = 'webdev';
  account.fire('change');
  assert.equal(fieldOf(view.main, 'Owned by').value, 'webdev');
  assert.match(view.main.textContent, /A service of another bundle cannot reach the socket yet: for a website's PHP, choose a password\./);
});

test('from a site, the name is the site\'s first word, the crumbs are the site\'s and Back goes to it', async (context) => {
  const { view } = await open(context, { query: 'site=my-blog.example.org', world: { list: list() } });
  assert.deepEqual(view.headed.crumbs, [
    { title: 'Apps', path: '/apps' }, { title: 'Websites', path: '/websites' }, { title: 'my-blog.example.org', path: '/websites/my-blog.example.org' },
  ]);
  assert.equal(fieldOf(view.main, 'Name').value, 'my_blog');
  assert.equal(fieldOf(view.main, 'Owned by').value, 'my_blog');
  assert.match(view.main.textContent, /for my-blog\.example\.org\. Snapshots/);
  assert.equal(view.main.querySelectorAll('a').find((one) => one.textContent === 'Back').getAttribute('href'), '/websites/my-blog.example.org');
  assert.equal(view.main.querySelectorAll('a').find((one) => one.textContent === 'Cancel').getAttribute('href'), '/databases');
});

test('a name that is taken gets a number once the databases are read', async (context) => {
  const { view } = await open(context, { query: 'site=shop.example.org' });
  assert.equal(fieldOf(view.main, 'Name').value, 'shop_2');
  assert.equal(fieldOf(view.main, 'Owned by').value, 'shop_2');
});

test('a site that is not a name is not used', async (context) => {
  const { view } = await open(context, { query: 'site=Shop..Example' });
  assert.equal(fieldOf(view.main, 'Name').value, '');
  assert.equal(view.headed.crumbs.length, 2);
  assert.equal(view.main.querySelectorAll('a').find((one) => one.textContent === 'Back'), undefined);
});

test('a form with a fault says it under the field, moves focus there and sends nothing', async (context) => {
  const world = await open(context);
  const { view, document } = world;
  await pressAdd(world);
  assert.match(view.main.textContent, /Give the database a name\./);
  assert.equal(document.activeElement.classes.has('msg'), true);
  assert.equal(world.ran.filter(([action]) => action === 'databases.add').length, 0);
  type(fieldOf(view.main, 'Name'), 'Shop');
  await pressAdd(world);
  assert.match(view.main.textContent, /Use lowercase letters, digits and _/);
  type(fieldOf(view.main, 'Name'), 'postgres');
  await pressAdd(world);
  assert.match(view.main.textContent, /PostgreSQL keeps the names that start with pg_/);
  type(fieldOf(view.main, 'Name'), 'shop');
  await pressAdd(world);
  assert.match(view.main.textContent, /A database or a role named shop is there already\./);
  assert.equal(fieldOf(view.main, 'Name').getAttribute('aria-invalid'), 'true');
  assert.equal(world.ran.filter(([action]) => action === 'databases.add').length, 0);
});

test('the role is checked as the name is', async (context) => {
  const world = await open(context);
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  type(fieldOf(world.view.main, 'Owned by'), 'blog');
  await pressAdd(world);
  assert.match(world.view.main.textContent, /A database or a role named blog is there already\./);
  type(fieldOf(world.view.main, 'Owned by'), 'pg_x');
  await pressAdd(world);
  assert.match(world.view.main.textContent, /PostgreSQL keeps the names/);
  assert.equal(world.ranOf('databases.add').length, 0);
});

test('the addresses for other machines are checked, and every address needs the word typed', async (context) => {
  const world = await open(context);
  const { view } = world;
  type(fieldOf(view.main, 'Name'), 'shop3');
  choose(view.main, 'other machines');
  await pressAdd(world);
  assert.match(view.main.textContent, /Give at least one network/);
  type(fieldOf(view.main, 'Addresses'), '10.0.2.0');
  await pressAdd(world);
  assert.match(view.main.textContent, /10\.0\.2\.0 is not an address with its prefix length/);
  type(fieldOf(view.main, 'Addresses'), '0.0.0.0/0');
  const typedBox = view.main.querySelectorAll('.banner.warn').find((one) => one.textContent.includes('Type anywhere'));
  assert.equal(typedBox.hidden, false);
  await pressAdd(world);
  assert.match(view.main.textContent, /Type anywhere to let every address in\./);
  assert.equal(world.ranOf('databases.add').length, 0);
  type(fieldOf(view.main, 'Type anywhere'), 'anywhere');
  world.state.list = list();
  await pressAdd(world);
  assert.equal(world.ranOf('databases.add').length, 1);
});

test('a certificate that does not name the machine is said, when it is chosen', async (context) => {
  const { view } = await open(context);
  choose(view.main, 'other machines');
  const certificate = fieldOf(view.main, 'Certificate');
  assert.deepEqual(certificate.querySelectorAll('option').map((one) => one.getAttribute('value')), ['this-machine', 'wide.example.org', 'blog.example.org']);
  const note = () => view.main.querySelectorAll('.db-sub')[1].children[2];
  assert.equal(note().hidden, true);
  certificate.value = 'blog.example.org';
  certificate.fire('change');
  assert.match(note().textContent, /^This certificate does not name nas\.example\.org, so programs that check the name will refuse it\.$/);
  assert.equal(note().hidden, false);
  certificate.value = 'wide.example.org';
  certificate.fire('change');
  assert.equal(note().hidden, true);
});

test('the commands show under what happens only while commands are shown', async (context) => {
  const hidden = await open(context);
  assert.equal(hidden.view.main.querySelector('.db-runs').hidden, true);
  const shown = await open(context, { prefs: { commands: 'shown' } });
  type(fieldOf(shown.view.main, 'Name'), 'shop3');
  assert.equal(shown.panels.length, 0, 'the commands wait for the last key');
  context.mock.timers.tick(400);
  assert.equal(shown.view.main.querySelector('.db-runs').hidden, false);
  assert.deepEqual(shown.panels, [['databases.add', { name: 'shop3', locale: 'en_US.UTF-8' }]]);
});

test('a reader sees the form and the button is off, with the words', async (context) => {
  const world = await open(context, { session: { admin: false } });
  assert.equal(addButton(world).disabled, true);
  assert.match(world.view.main.textContent, /Only an administrator can add a database\./);
  assert.equal(world.ran.length, 0);
});

test('PostgreSQL that is not installed is said, and the button is off', async (context) => {
  const world = await open(context, { world: { list: missing() } });
  assert.match(world.view.main.querySelector('.db-server-slot').textContent, /PostgreSQL is not installed\./);
  assert.equal(addButton(world).disabled, true);
});

test('adding sends the name, the text and sorting and the site, as the administrator, and the steps say where it is', async (context) => {
  const world = await open(context, {
    query: 'site=blog2.example.org', world: { runs: { 'databases.add': adds('blog2') } },
  });
  const opening = world.view.leaving;
  assert.equal(opening, null);
  await pressAdd(world);
  const [[args, options]] = world.ranOf('databases.add');
  assert.deepEqual(args, { name: 'blog2', locale: 'en_US.UTF-8', site: 'blog2.example.org' });
  assert.equal(options.words.unlock, 'add the database blog2');
  assert.equal(world.view.main.querySelector('h2#db-run-h').textContent, 'Adding blog2');
  assert.deepEqual(world.view.main.querySelectorAll('.flow-step b').map((one) => one.textContent), ['Make the role blog2 and the database blog2']);
});

test('the password is shown once, in a dialog, with the settings, and is nowhere else on the page', async (context) => {
  const world = await open(context, { world: { runs: { 'databases.add': adds('shop3') } } });
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  await pressAdd(world);
  const [dialog] = dialogsOf(world.document);
  assert.ok(dialog, 'the password is shown');
  assert.equal(dialog.querySelector('h2').textContent, 'The password of shop3');
  const pre = dialog.querySelector('pre');
  assert.match(pre.textContent, new RegExp(`^Password: ${PASSWORD}$`, 'm'));
  assert.match(pre.textContent, /^Host: 127\.0\.0\.1$/m);
  assert.match(dialog.textContent, /It is shown only now: copy it or save it\. Lost, it is replaced by a new one on the database's page\./);
  assert.deepEqual(dialog.querySelectorAll('button').map((one) => one.textContent.trim()), ['Copy', 'Save as file', 'I have kept it']);
  // Outside the dialog nothing holds it.
  const outside = holdings(world.view.main).join('\n');
  assert.equal(outside.includes(PASSWORD), false);
  assert.equal(world.view.leaving().includes(PASSWORD), false);
  assert.match(world.view.leaving(), /The password of shop3 is shown once/);
  for (const [words] of world.notices) assert.equal(words.includes(PASSWORD), false);
  for (const [error] of world.problems) assert.equal(JSON.stringify(error).includes(PASSWORD), false);
  assert.equal(JSON.stringify(world.ran.map(([action, args]) => [action, args])).includes(PASSWORD), false);
});

test('Save as file saves the settings and the password as shop3-postgresql.txt, and Copy copies them', async (context) => {
  const world = await open(context, { world: { runs: { 'databases.add': adds('shop3') } } });
  const saved = [];
  context.mock.method(URL, 'createObjectURL', (blob) => { saved.push(blob); return 'blob:test/0'; });
  context.mock.method(URL, 'revokeObjectURL', () => {});
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  await pressAdd(world);
  const dialog = dialogsOf(world.document)[0];
  const clicks = [];
  const make = world.document.createElement;
  world.document.createElement = (tag) => {
    const element = make(tag);
    if (tag === 'a') element.click = () => clicks.push(element.getAttribute('download'));
    return element;
  };
  buttonOf(dialog, 'Copy').click();
  await settle();
  assert.equal(world.copied.length, 1);
  assert.match(world.copied[0], new RegExp(`^Password: ${PASSWORD}$`, 'm'));
  buttonOf(dialog, 'Save as file').click();
  assert.deepEqual(clicks, ['shop3-postgresql.txt']);
  assert.match(await saved[0].text(), new RegExp(`^Password: ${PASSWORD}$`, 'm'));
  assert.match(dialog.textContent, /Saved as shop3-postgresql\.txt\./);
});

test('once kept, the settings stay on the page without the password, and no way out of it holds the password', async (context) => {
  const world = await open(context, { world: { runs: { 'databases.add': adds('shop3') } } });
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  await pressAdd(world);
  await world.press(buttonOf(world.dialog(), 'I have kept it'));
  assert.equal(dialogsOf(world.document).length, 0);
  assert.equal(world.view.leaving, null);
  const page = world.view.main;
  assert.match(page.textContent, /shop3 is added\./);
  assert.match(page.textContent, /Its password was shown once and is kept nowhere on this server\./);
  assert.match(page.textContent, /Connection settings/);
  assert.match(page.textContent, /new PDO\('pgsql:host=127\.0\.0\.1;port=5432;dbname=shop3', 'shop3', \$password\);/);
  assert.equal(everything(world).join('\n').includes(PASSWORD), false);
  const links = world.view.main.querySelector('.db-after').querySelectorAll('a').map((one) => [one.textContent.trim(), one.getAttribute('href')]);
  assert.deepEqual(links.filter(([, href]) => href.startsWith('/')), [['Open shop3', '/databases/shop3'], ['Add another', '/databases/add']]);
  assert.equal(world.notices.length, 0);
  assert.equal(world.went.length, 0);
});

test('a dialog closed without keeping the password says so and where to make another, and lets go of the guard', async (context) => {
  const world = await open(context, { world: { runs: { 'databases.add': adds('shop3') } } });
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  await pressAdd(world);
  world.dialog().close('');
  await settle();
  assert.equal(world.view.leaving, null);
  assert.match(world.view.main.textContent, /shop3 is added, but its password was not kept\./);
  assert.match(world.view.main.textContent, /Open its page and make a new password\./);
  assert.equal(everything(world).join('\n').includes(PASSWORD), false);
});

test('Escape asks first, and Go back keeps the password on show; closing then says it was not kept', async (context) => {
  const world = await open(context, { world: { runs: { 'databases.add': adds('shop3') } } });
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  await pressAdd(world);
  const dialog = dialogsOf(world.document)[0];
  dialog.fire('cancel');
  await settle();
  const question = dialogsOf(world.document)[1];
  assert.match(question.textContent, /The password is shown only now\./);
  buttonOf(question, 'Go back').click();
  await settle();
  assert.equal(dialog.open, true);
  assert.ok(world.view.leaving, 'still guarded');
  assert.match(dialog.querySelector('pre').textContent, new RegExp(PASSWORD));
  dialog.fire('cancel');
  await settle();
  buttonOf(dialogsOf(world.document)[1], 'Close without keeping it').click();
  await settle();
  assert.equal(dialogsOf(world.document).length, 0);
  assert.equal(world.view.leaving, null);
  assert.match(world.view.main.textContent, /shop3 is added, but its password was not kept\./);
  assert.equal(everything(world).join('\n').includes(PASSWORD), false);
});

test('leaving the page while the password is shown closes the dialog and keeps nothing', async (context) => {
  const world = await open(context, { world: { runs: { 'databases.add': adds('shop3') } } });
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  await pressAdd(world);
  assert.equal(dialogsOf(world.document).length, 1);
  world.leave();
  await settle();
  assert.equal(dialogsOf(world.document).length, 0);
  assert.equal(world.view.leaving, null);
  assert.equal(everything(world).join('\n').includes(PASSWORD), false);
});

test('leaving the page before the first step answers lets go of the guard, and nothing is shown', async (context) => {
  let answer;
  const world = await open(context, { world: { runs: { 'databases.add': () => new Promise((resolve) => { answer = resolve; }) } } });
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  await pressAdd(world);
  assert.ok(world.view.leaving());
  world.leave();
  answer(adds('shop3'));
  await settle();
  assert.equal(world.view.leaving, null);
  assert.equal(dialogsOf(world.document).length, 0);
  assert.equal(everything(world).join('\n').includes(PASSWORD), false);
});

test('a name taken after the form was read is refused, the steps stop, and Back to the form brings the form back', async (context) => {
  const world = await open(context, {
    world: { runs: { 'databases.add': refusal('DB-102', 'A database or role of that name is there already') } },
  });
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  await pressAdd(world);
  assert.equal(dialogsOf(world.document).length, 0);
  assert.equal(world.view.leaving, null);
  const step = world.view.main.querySelector('.flow-step');
  assert.equal(step.getAttribute('data-state'), 'failed');
  assert.ok(step.querySelectorAll('a').some((one) => one.getAttribute('href') === '/help/errors/DB-102'));
  assert.match(step.textContent, /The database is made only when this step is done\./);
  await world.press(buttonOf(world.view.main, 'Back to the form'));
  assert.equal(world.view.main.querySelector('.db-add').hidden, false);
  assert.equal(world.view.main.querySelector('.db-run-slot').children.length, 0);
  assert.equal(fieldOf(world.view.main, 'Name').value, 'shop3');
});

test('an answer without the password fails the step, as the database may have been made', async (context) => {
  const world = await open(context, { world: { runs: { 'databases.add': { ok: true, result: { done: true, output: ['database: added shop3'] } } } } });
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  await pressAdd(world);
  assert.equal(dialogsOf(world.document).length, 0);
  assert.equal(world.view.main.querySelector('.flow-step').getAttribute('data-state'), 'failed');
  assert.match(world.view.main.textContent, /make a new password on its page/);
});

test('an account by the socket is sent as the peer, and has no password to show', async (context) => {
  const peer = added('shop3', { role: 'chase', password: null, host: null });
  const world = await open(context, { world: { runs: { 'databases.add': { ok: true, result: peer } } } });
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  choose(world.view.main, 'An account of this server');
  await pressAdd(world);
  assert.deepEqual(world.ranOf('databases.add')[0][0], { name: 'shop3', peer: 'chase', locale: 'en_US.UTF-8' });
  assert.equal(dialogsOf(world.document).length, 0);
  assert.match(world.view.main.textContent, /It has no password: chase signs in as its account, through the socket\./);
  assert.match(world.view.main.textContent, /psql -d shop3/);
  assert.equal(world.view.leaving, null);
});

test('an account that has a role already is not made again', async (context) => {
  const world = await open(context);
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  choose(world.view.main, 'An account of this server');
  const account = fieldOf(world.view.main, 'Account');
  account.value = 'webdev';
  account.fire('change');
  await pressAdd(world);
  assert.match(world.view.main.textContent, /A PostgreSQL role named webdev is there already\./);
  assert.equal(world.ranOf('databases.add').length, 0);
});

test('other machines are let in with a second step that is a task, and the password is shown when both are done', async (context) => {
  const world = await open(context, {
    world: {
      runs: { 'databases.add': adds('shop3'), 'databases.allow': { ok: true, task: 7, result: { task: 7 } } },
      reads: { '/api/1/tasks/7': finished(7) },
    },
  });
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  choose(world.view.main, 'other machines');
  type(fieldOf(world.view.main, 'Addresses'), '192.168.1.0/24\n10.0.0.0/8');
  await pressAdd(world);
  assert.deepEqual(world.ran.filter(([action]) => action !== 'databases.list').map(([action]) => action), ['databases.add', 'databases.allow']);
  assert.deepEqual(world.ranOf('databases.allow')[0][0], { name: 'shop3', from: '192.168.1.0/24\n10.0.0.0/8', certificate: 'this-machine' });
  assert.deepEqual(world.view.main.querySelectorAll('.flow-step').map((one) => one.getAttribute('data-state')), ['done', 'done']);
  const dialog = world.dialog();
  assert.match(dialog.querySelector('pre').textContent, /^Host: nas\.example\.org$/m);
  assert.match(dialog.querySelector('pre').textContent, /^Root certificate: nas\.pem$/m);
  await world.press(buttonOf(dialog, 'I have kept it'));
  const ways = world.view.main.querySelectorAll('.db-way h3').map((one) => one.textContent);
  assert.deepEqual(ways, ['Programs on this server, with a password', 'Programs on other machines, with a password']);
  assert.match(world.view.main.textContent, /sslrootcert=nas\.pem/);
  assert.equal(everything(world).join('\n').includes(PASSWORD), false);
});

test('when letting other machines in fails, the first step stands, the password is still shown and the page says where to try again', async (context) => {
  const world = await open(context, {
    world: {
      runs: { 'databases.add': adds('shop3'), 'databases.allow': { ok: true, task: 7, result: { task: 7 } } },
      reads: { '/api/1/tasks/7': { id: 7, state: 'failed', status: 1, log: ['database allow: pg_hba.conf line 91: bad (DB-104)'] } },
    },
  });
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  choose(world.view.main, 'other machines');
  type(fieldOf(world.view.main, 'Addresses'), '192.168.1.0/24');
  await pressAdd(world);
  assert.deepEqual(world.view.main.querySelectorAll('.flow-step').map((one) => one.getAttribute('data-state')), ['done', 'failed']);
  const failed = world.view.main.querySelectorAll('.flow-step')[1];
  assert.match(failed.textContent, /The database shop3 is made, and programs on this server can connect with its password\./);
  assert.ok(failed.querySelectorAll('a').some((one) => one.getAttribute('href') === '/help/errors/DB-104'));
  // The database exists, so its password is shown, with the settings that work.
  const dialog = world.dialog();
  assert.ok(dialog);
  assert.match(dialog.querySelector('pre').textContent, /^Host: 127\.0\.0\.1$/m);
  await world.press(buttonOf(dialog, 'I have kept it'));
  assert.match(world.view.main.textContent, /Other machines cannot connect yet\. Open its page to try again\./);
  assert.deepEqual(world.view.main.querySelectorAll('.db-way h3').map((one) => one.textContent), ['Programs on this server, with a password']);
  assert.equal(everything(world).join('\n').includes(PASSWORD), false);
});

test('a role that exists after the read is not offered by the name that was typed', async (context) => {
  const world = await open(context, { world: { list: list({ roles: [role('shop3')] }) } });
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  await pressAdd(world);
  assert.match(world.view.main.textContent, /A database or a role named shop3 is there already\./);
});

test('the console\'s own password for nothing: the page never reads the list on a timer or again after adding', async (context) => {
  const world = await open(context, { world: { runs: { 'databases.add': adds('shop3') } } });
  type(fieldOf(world.view.main, 'Name'), 'shop3');
  await pressAdd(world);
  await world.press(buttonOf(world.dialog(), 'I have kept it'));
  assert.equal(world.timers.length, 0);
  assert.equal(world.ranOf('databases.list').length, 1);
});
