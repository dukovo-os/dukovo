// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A database's page (pages/databases/database.js and dialogs.js): its facts,
// the settings of each way to connect, who connects, and the changes that
// follow: let other machines connect, stop them, a new password, remove it.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  database, empty, finished, list, missing, off, PASSWORD, role, server,
} from './databases-fixtures.mjs';
import {
  buttonOf, dialogsOf, fieldOf, holdings, openDatabases, refusal, settle, type,
} from './databases-view.mjs';

// The clock is the test's: nothing waits for real time, and a timer that was not stopped is seen.
const clocked = new WeakSet();
const open = (context, name, more = {}) => {
  if (!clocked.has(context)) {
    clocked.add(context);
    context.mock.timers.enable({ apis: ['setTimeout'] });
  }
  return openDatabases(context, { rest: name, ...more });
};

/** What a database's page holds in words, with the dialogs. */
const everything = (world) => holdings(world.document.body).join('\n');

/** Whether a button is off, as the attribute or the property says. */
const isOff = (button) => button.disabled === true || button.hasAttribute('disabled');

/** The sections of the page by their heading. */
const sectionOf = (world, heading) => world.view.main.querySelectorAll('section').find((one) => one.querySelector('h2')?.textContent === heading);

/** A new password's answer, as `databases.password` prints it. */
const newPassword = (name = 'shop') => ({
  ok: true,
  result: {
    done: true,
    changed: true,
    output: [`database: new password for ${name}`, JSON.stringify({
      format: 1, database: name, role: name, password: PASSWORD, host: '127.0.0.1', port: 5432,
    })],
  },
});

test('the page has the name, its crumbs and the facts of the database', async (context) => {
  const world = await open(context, 'shop');
  assert.equal(world.view.headed.title, 'shop');
  assert.deepEqual(world.view.headed.crumbs, [{ title: 'Apps', path: '/apps' }, { title: 'Databases', path: '/databases' }]);
  assert.equal(world.view.headed.commands, true);
  const facts = sectionOf(world, 'About this database');
  assert.deepEqual(facts.querySelectorAll('dt').map((one) => one.textContent), ['Owner', 'Text and sorting', 'Size', 'Sessions', 'For']);
  assert.deepEqual(facts.querySelectorAll('dd').map((one) => one.textContent), ['shop', 'UTF-8, sorted as English (US)', '38 MB', '0', 'shop.example.org']);
  assert.match(facts.textContent, /No one is connected now\./);
  const blog = await open(context, 'blog');
  assert.match(sectionOf(blog, 'About this database').textContent, /2 programs are connected now\./);
});

test('what it asks is the databases, as an administrator, and the machine\'s name, and nothing on a timer', async (context) => {
  const world = await open(context, 'shop');
  assert.deepEqual(world.calls, ['/api/1/machine/name']);
  assert.equal(world.ranOf('databases.list').length, 1);
  assert.deepEqual(world.ranOf('databases.list')[0][1].words.unlock, 'list the databases');
  assert.equal(world.timers.length, 0);
});

test('the settings are those of each way the database is reached, and none holds a password', async (context) => {
  const world = await open(context, 'shop');
  const settings = sectionOf(world, 'Connection settings');
  assert.deepEqual(settings.querySelectorAll('h3').map((one) => one.textContent), ['Programs on this server, with a password', 'Programs on other machines, with a password']);
  assert.match(settings.textContent, /new PDO\('pgsql:host=127\.0\.0\.1;port=5432;dbname=shop', 'shop', \$password\);/);
  assert.match(settings.textContent, /sslmode=verify-full/);
  assert.match(settings.textContent, /The one shown once/);
  assert.equal(everything(world).includes(PASSWORD), false);
  const notes = await open(context, 'notes');
  assert.deepEqual(sectionOf(notes, 'Connection settings').querySelectorAll('h3').map((one) => one.textContent), ['An account of this server, by the socket']);
  assert.match(sectionOf(notes, 'Connection settings').textContent, /psql -d notes/);
  const blog = await open(context, 'blog');
  assert.deepEqual(sectionOf(blog, 'Connection settings').querySelectorAll('h3').map((one) => one.textContent), ['Programs on this server, with a password']);
});

test('Who connects has each way, each network with its Remove, and a line written by hand is kept and said', async (context) => {
  const world = await open(context, 'shop');
  const who = sectionOf(world, 'Who connects');
  assert.deepEqual(who.querySelectorAll('li').map((one) => one.textContent), ['Programs on this server, with the password', '192.168.1.0/24, over TLS with the passwordRemove']);
  assert.equal(buttonOf(who, 'Remove').getAttribute('aria-label'), 'Remove 192.168.1.0/24');
  const blog = await open(context, 'blog');
  const hand = sectionOf(blog, 'Who connects').querySelectorAll('li');
  assert.equal(hand.length, 2);
  assert.match(hand[1].textContent, /Written by hand in pg_hba\.conf: kept as it is\./);
  assert.equal(buttonOf(sectionOf(blog, 'Who connects'), 'Stop them'), undefined);
  const notes = await open(context, 'notes');
  assert.deepEqual(sectionOf(notes, 'Who connects').querySelectorAll('li').map((one) => one.textContent), ['webdev, by its account']);
});

test('the buttons are those that fit: a password database has Let other machines connect and a new password, an account has neither', async (context) => {
  const shop = await open(context, 'shop');
  const words = (world) => world.view.main.querySelectorAll('button').map((one) => one.textContent.trim()).filter((one) => !['', 'Copy'].includes(one) && !one.startsWith('Download'));
  assert.deepEqual(words(shop), ['Remove', 'Let other machines connect', 'Stop them', 'Make a new password', 'Remove…']);
  const blog = await open(context, 'blog');
  assert.deepEqual(words(blog), ['Let other machines connect', 'Make a new password', 'Remove…']);
  const notes = await open(context, 'notes');
  assert.deepEqual(words(notes), ['Remove…']);
  assert.match(sectionOf(notes, 'Password and removal').textContent, /Its role signs in as the account webdev, so it has no password\./);
});

test('the commands that the buttons run are shown beside them while commands are shown', async (context) => {
  const world = await open(context, 'shop');
  assert.deepEqual(world.inlines.map(([action]) => action).sort(), ['databases.deny', 'databases.password', 'databases.remove']);
});

test('a database whose indexes need rebuilding says so, with the two commands', async (context) => {
  const world = await open(context, 'blog');
  assert.match(world.view.main.textContent, /Its sorting data changed in an update: rebuild its indexes, then tell PostgreSQL that it did\./);
  assert.match(world.view.main.textContent, /REINDEX DATABASE blog/);
  assert.match(world.view.main.textContent, /REFRESH COLLATION VERSION/);
  const shop = await open(context, 'shop');
  assert.doesNotMatch(shop.view.main.textContent, /REINDEX/);
});

test('a name that is not there is said, with the way back', async (context) => {
  const world = await open(context, 'nothing');
  assert.match(world.view.main.textContent, /There is no database named nothing\./);
  assert.equal(world.view.main.querySelectorAll('a').find((one) => one.textContent === 'All the databases').getAttribute('href'), '/databases');
  assert.equal(world.view.main.querySelectorAll('section').find((one) => one.querySelector('h2')?.textContent === 'Who connects'), undefined);
});

test('a database called add has its page at %61dd', async (context) => {
  const world = await open(context, '%61dd', { world: { list: list({ databases: [database('add')], roles: [role('add')], access: [] }) } });
  assert.equal(world.view.headed.title, 'add');
  assert.ok(sectionOf(world, 'About this database'));
});

test('PostgreSQL that is off or not installed is said, in place of the database', async (context) => {
  const stopped = await open(context, 'shop', { world: { list: off() } });
  assert.match(stopped.view.main.textContent, /PostgreSQL is off\./);
  assert.equal(sectionOf(stopped, 'Who connects'), undefined);
  const absent = await open(context, 'shop', { world: { list: missing() } });
  assert.match(absent.view.main.textContent, /PostgreSQL is not installed\./);
});

test('a reader is told who sees it, and an administrator whose console is locked is asked to unlock', async (context) => {
  const reader = await open(context, 'shop', { session: { admin: false } });
  assert.match(reader.view.main.textContent, /Only an administrator can list the databases/);
  assert.equal(reader.ran.length, 0);
  assert.equal(sectionOf(reader, 'Who connects'), undefined);
  const locked = await open(context, 'shop', { unlocked: false });
  assert.ok(buttonOf(locked.view.main, 'Unlock'));
  assert.equal(locked.ran.length, 0);
});

test('a read that fails is said, with Try again', async (context) => {
  const world = await open(context, 'shop', { world: { runs: { 'databases.list': refusal('DB-101', 'PostgreSQL did not answer') } } });
  assert.match(world.view.main.textContent, /PostgreSQL did not answer/);
  assert.ok(buttonOf(world.view.main, 'Try again') || buttonOf(world.view.main, 'Read again'));
});

// ---- Let other machines connect, Stop them --------------------------------

test('Let other machines connect asks for the networks, and says what it does to PostgreSQL', async (context) => {
  const world = await open(context, 'blog');
  await world.press(buttonOf(world.view.main, 'Let other machines connect'));
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Let other machines connect to blog');
  assert.match(dialog.textContent, /PostgreSQL is listening on the network already: it reads its settings again, and no session ends\./);
  assert.ok(fieldOf(dialog, 'Addresses'));
  assert.ok(fieldOf(dialog, 'Certificate'));
  assert.equal(world.ranOf('databases.allow').length, 0);
  // This machine's own certificate is the one PostgreSQL has; another is a restart.
  const certificate = fieldOf(dialog, 'Certificate');
  certificate.value = 'wide.example.org';
  certificate.fire('change');
  assert.match(dialog.textContent, /PostgreSQL restarts once to listen on the network: its open sessions end\./);
  const local = await open(context, 'blog', { world: { list: list({ server: server() }) } });
  await local.press(buttonOf(local.view.main, 'Let other machines connect'));
  assert.match(local.dialog().textContent, /PostgreSQL restarts once to listen on the network/);
});

test('a dialog with no network, or one that is not an address, says so under the field and sends nothing', async (context) => {
  const world = await open(context, 'blog');
  await world.press(buttonOf(world.view.main, 'Let other machines connect'));
  await world.submit();
  assert.match(world.dialog().textContent, /Give at least one network/);
  type(fieldOf(world.dialog(), 'Addresses'), '10.0.0.0');
  await world.submit();
  assert.match(world.dialog().textContent, /10\.0\.0\.0 is not an address with its prefix length/);
  type(fieldOf(world.dialog(), 'Addresses'), '0.0.0.0/0');
  await world.submit();
  assert.match(world.dialog().textContent, /Type anywhere to let every address in\./);
  assert.equal(world.ranOf('databases.allow').length, 0);
});

test('Cancel in the dialog changes nothing', async (context) => {
  const world = await open(context, 'blog');
  await world.press(buttonOf(world.view.main, 'Let other machines connect'));
  await world.press(buttonOf(world.dialog(), 'Cancel'));
  assert.equal(dialogsOf(world.document).length, 0);
  assert.equal(world.ran.length, 1);
  assert.equal(world.view.main.querySelector('.db-flow').children.length, 0);
});

test('the networks are let in as a step that is a task, the buttons are off while it runs, and the page is read again after', async (context) => {
  let end;
  const world = await open(context, 'blog', {
    world: {
      runs: { 'databases.allow': { ok: true, task: 7, result: { task: 7 } } },
      reads: { '/api/1/tasks/7': () => end() },
    },
  });
  let running = true;
  end = () => (running ? { id: 7, state: 'running', step: 2, steps: 5, words: 'Writing the settings', log: [] } : finished(7));
  await world.press(buttonOf(world.view.main, 'Let other machines connect'));
  type(fieldOf(world.dialog(), 'Addresses'), '10.0.0.0/8\n192.168.0.0/16');
  await world.submit();
  assert.deepEqual(world.ranOf('databases.allow')[0][0], { name: 'blog', from: '10.0.0.0/8\n192.168.0.0/16', certificate: 'this-machine' });
  assert.equal(dialogsOf(world.document).length, 0);
  const flow = world.view.main.querySelector('.db-flow');
  assert.equal(flow.querySelector('h2').textContent, 'Letting other machines connect to blog');
  assert.equal(flow.querySelector('.flow-step').getAttribute('data-state'), 'running');
  assert.equal(isOff(buttonOf(world.view.main, 'Make a new password')), true);
  assert.equal(isOff(buttonOf(world.view.main, 'Let other machines connect')), true);
  running = false;
  world.state.list = list({ access: [...list().access, { ...list().access[0], databases: ['blog'], users: ['blog'], address: '10.0.0.0/8' }] });
  await world.emit('task', { id: 7, state: 'done' });
  assert.equal(flow.querySelector('.flow-step').getAttribute('data-state'), 'done');
  assert.equal(world.ranOf('databases.list').length, 2, 'the list is read again after the change');
  assert.equal(isOff(buttonOf(world.view.main, 'Make a new password')), false);
  assert.match(sectionOf(world, 'Who connects').textContent, /10\.0\.0\.0\/8, over TLS with the password/);
});

test('when PostgreSQL would not take the settings the step says so, what stands, the error\'s page and the task', async (context) => {
  const world = await open(context, 'blog', {
    world: {
      runs: { 'databases.allow': { ok: true, task: 7, result: { task: 7 } } },
      reads: { '/api/1/tasks/7': { id: 7, state: 'failed', status: 1, log: ['database allow: PostgreSQL did not start (DB-105)'] } },
    },
  });
  await world.press(buttonOf(world.view.main, 'Let other machines connect'));
  type(fieldOf(world.dialog(), 'Addresses'), '10.0.0.0/8');
  await world.submit();
  const step = world.view.main.querySelector('.flow-step');
  assert.equal(step.getAttribute('data-state'), 'failed');
  assert.match(step.textContent, /Other machines cannot connect yet/);
  assert.match(step.textContent, /PostgreSQL's settings are as they were\./);
  assert.ok(step.querySelectorAll('a').some((one) => one.getAttribute('href') === '/help/errors/DB-105'));
  await world.press(buttonOf(step, 'Open the task'));
  assert.deepEqual(world.opened, [7]);
  assert.equal(isOff(buttonOf(world.view.main, 'Let other machines connect')), false);
});

test('Stop them asks, says whether PostgreSQL restarts, and does nothing when it is kept', async (context) => {
  const world = await open(context, 'shop');
  await world.press(buttonOf(world.view.main, 'Stop them'));
  let question = dialogsOf(world.document)[0];
  assert.equal(question.querySelector('h2').textContent, 'Stop other machines connecting to shop?');
  assert.match(question.textContent, /No other database lets other machines in, so PostgreSQL goes back to listening on this machine only, and restarts once: its open sessions end\./);
  await world.press(buttonOf(question, 'Keep it'));
  assert.equal(world.ranOf('databases.deny').length, 0);
  // Another database that lets machines in: PostgreSQL goes on listening.
  const more = list({ access: [...list().access, { ...list().access[0], databases: ['blog'], users: ['blog'], line: 92 }] });
  const other = await open(context, 'shop', { world: { list: more } });
  await other.press(buttonOf(other.view.main, 'Stop them'));
  assert.match(dialogsOf(other.document)[0].textContent, /PostgreSQL reads its settings again; no session ends\./);
});

test('Stop them stops every network, and Remove stops one', async (context) => {
  const world = await open(context, 'shop', { world: { runs: { 'databases.deny': { ok: true, task: 8, result: { task: 8 } } }, reads: { '/api/1/tasks/8': finished(8) } } });
  await world.press(buttonOf(world.view.main, 'Stop them'));
  await world.press(buttonOf(dialogsOf(world.document)[0], 'Stop them'));
  assert.deepEqual(world.ranOf('databases.deny')[0][0], { name: 'shop' });
  assert.equal(world.view.main.querySelector('.db-flow h2').textContent, 'Stopping other machines connecting to shop');
  assert.equal(world.view.main.querySelector('.flow-step').getAttribute('data-state'), 'done');
  const one = await open(context, 'shop', { world: { runs: { 'databases.deny': { ok: true, task: 8, result: { task: 8 } } }, reads: { '/api/1/tasks/8': finished(8) } } });
  await one.press(buttonOf(one.view.main, 'Remove'));
  const question = dialogsOf(one.document)[0];
  assert.equal(question.querySelector('h2').textContent, 'Stop 192.168.1.0/24 connecting to shop?');
  await one.press(buttonOf(question, 'Stop 192.168.1.0/24'));
  assert.deepEqual(one.ranOf('databases.deny')[0][0], { name: 'shop', from: '192.168.1.0/24' });
});

// ---- Make a new password ---------------------------------------------------

test('Make a new password asks, makes it as an administrator, and shows it once with the settings', async (context) => {
  const world = await open(context, 'shop', { world: { runs: { 'databases.password': newPassword() } } });
  await world.press(buttonOf(world.view.main, 'Make a new password'));
  const question = world.dialog();
  assert.equal(question.querySelector('h2').textContent, 'Make a new password for shop?');
  assert.match(question.textContent, /Programs that use the old one stop signing in until they have the new one\./);
  assert.equal(world.ranOf('databases.password').length, 0);
  await world.submit(question);
  const [args, options] = world.ranOf('databases.password')[0];
  assert.deepEqual(args, { name: 'shop' });
  assert.equal(options.words.unlock, 'make a new password for shop');
  const shown = dialogsOf(world.document);
  assert.equal(shown.length, 1);
  assert.equal(shown[0].querySelector('h2').textContent, 'The password of shop');
  assert.match(shown[0].querySelector('pre').textContent, new RegExp(`^Password: ${PASSWORD}$`, 'm'));
  // Both ways it is reached by are in the settings: the database lets other machines in.
  assert.match(shown[0].querySelector('pre').textContent, /^Host: nas\.example\.org$/m);
  assert.match(world.view.leaving(), /The new password of shop is shown only once/);
  assert.equal(holdings(world.view.main).join('\n').includes(PASSWORD), false);
  await world.press(buttonOf(shown[0], 'I have kept it'));
  assert.equal(world.view.leaving, null);
  assert.equal(world.notices.length, 0);
  assert.equal(everything(world).includes(PASSWORD), false);
  assert.equal(JSON.stringify(world.ran.map(([action, call]) => [action, call])).includes(PASSWORD), false);
  for (const [, where] of world.problems) assert.equal(JSON.stringify(Object.keys(where ?? {})).includes(PASSWORD), false);
});

test('a password that was not kept is said in a notice, and is nowhere afterwards', async (context) => {
  const world = await open(context, 'blog', { world: { runs: { 'databases.password': newPassword('blog') } } });
  await world.press(buttonOf(world.view.main, 'Make a new password'));
  await world.submit();
  const shown = dialogsOf(world.document)[0];
  assert.match(shown.querySelector('pre').textContent, /^Host: 127\.0\.0\.1$/m, 'a database that lets no other machine in is reached on this server');
  shown.close('');
  await settle();
  assert.equal(world.view.leaving, null);
  assert.deepEqual(world.notices, [['The new password of blog was not kept. Make another when you are ready.', { kind: 'warn' }]]);
  assert.equal(everything(world).includes(PASSWORD), false);
});

test('leaving the page while the new password is shown drops it', async (context) => {
  const world = await open(context, 'shop', { world: { runs: { 'databases.password': newPassword() } } });
  await world.press(buttonOf(world.view.main, 'Make a new password'));
  await world.submit();
  assert.equal(dialogsOf(world.document).length, 1);
  world.leave();
  await settle();
  assert.equal(dialogsOf(world.document).length, 0);
  assert.equal(world.view.leaving, null);
  assert.equal(everything(world).includes(PASSWORD), false);
  assert.equal(world.notices.length, 0);
});

test('an answer that holds no password is said, and the question is closed', async (context) => {
  const world = await open(context, 'shop', { world: { runs: { 'databases.password': { ok: true, result: { done: true, changed: true, output: [] } } } } });
  await world.press(buttonOf(world.view.main, 'Make a new password'));
  await world.submit();
  assert.equal(dialogsOf(world.document).length, 0);
  assert.match(world.notices[0][0], /The password of shop was changed, but the answer held no password\. Make another\./);
});

test('a refusal keeps the question open with the reason and shows nothing', async (context) => {
  const world = await open(context, 'shop', { world: { runs: { 'databases.password': refusal('DB-106', 'PostgreSQL would not change it') } } });
  await world.press(buttonOf(world.view.main, 'Make a new password'));
  await world.submit();
  assert.equal(dialogsOf(world.document).length, 1);
  assert.match(world.dialog().textContent, /PostgreSQL would not change it/);
  assert.equal(world.view.leaving, null);
});

test('the palette\'s way in, ?password, asks the question once, and not for an account', async (context) => {
  const world = await open(context, 'shop', { query: 'password' });
  assert.equal(world.dialog().querySelector('h2').textContent, 'Make a new password for shop?');
  assert.equal(world.ranOf('databases.password').length, 0);
  await world.press(buttonOf(world.dialog(), 'Cancel'));
  await world.emit('unlock');
  assert.equal(dialogsOf(world.document).length, 0, 'it is not asked again by a later read');
  const notes = await open(context, 'notes', { query: 'password' });
  assert.equal(dialogsOf(notes.document).length, 0);
});

// ---- Remove… ---------------------------------------------------------------

test('Remove… asks for the name, says what goes and what stays, and offers the role it made', async (context) => {
  const world = await open(context, 'shop', { world: { runs: { 'databases.remove': { ok: true, result: { done: true, changed: true, output: ['database: removed shop'] } } } } });
  await world.press(buttonOf(world.view.main, 'Remove…'));
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Remove shop?');
  assert.match(dialog.textContent, /Removes the database shop and everything in it\. Its data stays in the snapshots of \/shared until they are pruned\./);
  const confirm = buttonOf(dialog, 'Remove the database');
  assert.equal(confirm.disabled, true);
  type(fieldOf(dialog, 'Type shop to go on'), 'sho');
  assert.equal(confirm.disabled, true);
  const tick = dialog.querySelector('input[type="checkbox"]');
  assert.equal(tick.checked, true, 'the role it owns alone goes with it');
  type(fieldOf(dialog, 'Type shop to go on'), 'shop');
  assert.equal(confirm.disabled, false);
  await world.submit(dialog);
  const [args, options] = world.ranOf('databases.remove')[0];
  assert.deepEqual(args, { name: 'shop', role: true });
  assert.equal(options.words.unlock, 'remove shop');
  assert.equal(dialogsOf(world.document).length, 0);
  assert.deepEqual(world.went, ['/databases']);
});

test('the role is left when it is unticked, and is not offered when this page did not make it', async (context) => {
  const world = await open(context, 'shop');
  await world.press(buttonOf(world.view.main, 'Remove…'));
  const tick = world.dialog().querySelector('input[type="checkbox"]');
  tick.checked = false;
  type(fieldOf(world.dialog(), 'Type shop to go on'), 'shop');
  await world.submit();
  assert.deepEqual(world.ranOf('databases.remove')[0][0], { name: 'shop' });
  const own = await open(context, 'shop', { world: { list: list({ roles: [role('shop', { made_by_database: false })] }) } });
  await own.press(buttonOf(own.view.main, 'Remove…'));
  assert.equal(own.dialog().querySelector('input[type="checkbox"]'), null);
  assert.match(own.dialog().textContent, /The role shop was not made by this page, so it stays\./);
});

test('a role that owns another database too is not ticked, and the other is named', async (context) => {
  const shared = list({ databases: [database('shop'), database('shop_old', { owner: 'shop' })] });
  const world = await open(context, 'shop', { world: { list: shared } });
  await world.press(buttonOf(world.view.main, 'Remove…'));
  assert.equal(world.dialog().querySelector('input[type="checkbox"]').checked, false);
  assert.match(world.dialog().textContent, /The role shop also owns 1 other database\./);
});

test('open sessions are said, and removing ends them', async (context) => {
  const world = await open(context, 'blog');
  await world.press(buttonOf(world.view.main, 'Remove…'));
  assert.match(world.dialog().textContent, /Ends its 2 open sessions\./);
  type(fieldOf(world.dialog(), 'Type blog to go on'), 'blog');
  await world.submit();
  assert.deepEqual(world.ranOf('databases.remove')[0][0], { name: 'blog', role: true, force: true });
});

test('a database that was removed under it is refused, the dialog stays and the page does not go', async (context) => {
  const world = await open(context, 'shop', { world: { runs: { 'databases.remove': refusal('DB-107', 'There is no database of that name') } } });
  await world.press(buttonOf(world.view.main, 'Remove…'));
  type(fieldOf(world.dialog(), 'Type shop to go on'), 'shop');
  await world.submit();
  assert.equal(dialogsOf(world.document).length, 1);
  assert.match(world.dialog().textContent, /There is no database of that name/);
  assert.deepEqual(world.went, []);
});

test('the command of the dialogs is shown while commands are shown', async (context) => {
  const world = await open(context, 'shop', { prefs: { commands: 'shown' } });
  await world.press(buttonOf(world.view.main, 'Remove…'));
  assert.deepEqual(world.panels.at(-1), ['databases.remove', { name: 'shop', role: true }]);
  await world.press(buttonOf(world.dialog(), 'Cancel'));
  await world.press(buttonOf(world.view.main, 'Make a new password'));
  assert.deepEqual(world.panels.at(-1), ['databases.password', { name: 'shop' }]);
});

// ---- the settings --------------------------------------------------------

test('Copy copies an example, and the examples hold no password', async (context) => {
  const world = await open(context, 'shop');
  const settings = sectionOf(world, 'Connection settings');
  const copy = settings.querySelectorAll('button').filter((one) => one.textContent.trim() === 'Copy')[0];
  await world.press(copy);
  assert.equal(world.copied.length, 1);
  assert.match(world.copied[0], /new PDO\('pgsql:host=127\.0\.0\.1;port=5432;dbname=shop', 'shop', \$password\);/);
  for (const box of settings.querySelectorAll('pre')) assert.equal(box.textContent.includes(PASSWORD), false);
});

test('Download the certificate saves the chain of this server\'s certificate as nas.pem', async (context) => {
  const saved = [];
  context.mock.method(URL, 'createObjectURL', (blob) => { saved.push(blob); return 'blob:test/0'; });
  context.mock.method(URL, 'revokeObjectURL', () => {});
  const world = await open(context, 'shop');
  const clicks = [];
  const make = world.document.createElement;
  world.document.createElement = (tag) => {
    const element = make(tag);
    if (tag === 'a') element.click = () => clicks.push(element.getAttribute('download'));
    return element;
  };
  await world.press(buttonOf(world.view.main, 'Download the certificate (nas.pem)'));
  assert.deepEqual(clicks, ['nas.pem']);
  assert.match(await saved[0].text(), /^-----BEGIN CERTIFICATE-----/);
  assert.match(sectionOf(world, 'Connection settings').textContent, /Saved as nas\.pem\./);
  assert.ok(world.calls.includes('/api/1/certificates/this-machine'));
});

test('a certificate that cannot be read is said, with where to get it', async (context) => {
  const world = await open(context, 'shop', { world: { reads: { '/api/1/certificates/this-machine': { name: 'this-machine' } } } });
  await world.press(buttonOf(world.view.main, 'Download the certificate (nas.pem)'));
  assert.match(sectionOf(world, 'Connection settings').textContent, /The certificate could not be read\. Download it on the Certificates page\./);
});

test('a database whose role cannot sign in has no way known, and says so', async (context) => {
  const lost = list({ roles: [role('shop', { login: false, password: false })], access: [] });
  const world = await open(context, 'shop', { world: { list: lost } });
  assert.match(sectionOf(world, 'Connection settings').textContent, /How programs connect to this database is not known/);
  assert.equal(buttonOf(world.view.main, 'Make a new password'), undefined);
});
