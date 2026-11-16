// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The list of databases (pages/databases/list.js and read.js): the three
// modes of an administrator's read, the line about the server, the table, the
// states of PostgreSQL, and that nothing is read on a timer.

import assert from 'node:assert/strict';
import test from 'node:test';

import { setUnlockSeconds } from '../unlock.js';
import {
  broken, empty, list, missing, off, server,
} from './databases-fixtures.mjs';
import {
  buttonOf, openDatabases, refusal, settle,
} from './databases-view.mjs';

/** The words of the cells of each row of `root`'s body. */
const rowsOf = (root) => root.querySelectorAll('tbody tr').map((row) => row.querySelectorAll('td').map((cell) => cell.textContent));

test('an administrator whose console is unlocked sees each database, with who connects, its size and its sessions', async (context) => {
  const { view } = await openDatabases(context);
  const rows = rowsOf(view.main);
  assert.deepEqual(rows.map((row) => row[0]), ['blogIts indexes need to be rebuilt', 'notes', 'shopfor shop.example.org']);
  assert.deepEqual(rows[2], ['shopfor shop.example.org', 'shop', 'Programs on this server, with the password192.168.1.0/24, over TLS with the password', '38 MB', '0']);
  assert.equal(rows[0][2], 'Programs on this server, with the passwordand 1 line written by hand');
  assert.equal(rows[0][4], '2');
  assert.equal(rows[1][2], 'webdev, by its account');
  assert.equal(rows[1][3], '8 KB');
});

test('each name is a link to the page of the database', async (context) => {
  const { view } = await openDatabases(context);
  const links = view.main.querySelectorAll('tbody a').map((link) => link.getAttribute('href'));
  assert.deepEqual(links, ['/databases/blog', '/databases/notes', '/databases/shop']);
  for (const cell of view.main.querySelectorAll('tbody td')) assert.ok(cell.getAttribute('data-label'));
  assert.equal(view.main.querySelector('table').classes.has('stack'), true);
});

test('the head has the crumb, the switch of commands and a link to Add a database', async (context) => {
  const { view } = await openDatabases(context);
  assert.equal(view.headed.title, 'Databases');
  assert.deepEqual(view.headed.crumbs, [{ title: 'Apps', path: '/apps' }]);
  assert.equal(view.headed.commands, true);
  const [add] = view.headed.buttons;
  assert.equal(add.getAttribute('href'), '/databases/add');
  assert.match(add.textContent, /Add a database/);
});

test('the line about the server says its version, that it runs and where it listens', async (context) => {
  const { view } = await openDatabases(context);
  const line = view.main.querySelector('.db-server');
  assert.equal(line.querySelector('.pill').textContent, 'Running');
  assert.match(line.textContent, /PostgreSQL 19\.0 · running · listens on the network, TLS with this-machine's certificate/);
  const local = await openDatabases(context, { world: { list: list({ server: server() }) } });
  assert.match(local.view.main.querySelector('.db-server').textContent, /listens on this machine only/);
  assert.ok(local.view.main.querySelector('.db-server a').getAttribute('href') === '/services/postgresql');
});

test('PostgreSQL\'s own databases are said not to be listed', async (context) => {
  const { view } = await openDatabases(context);
  assert.match(view.main.textContent, /PostgreSQL's own databases are not listed\./);
});

test('the list is read once, as an administrator, with the unlock words, and never on a timer', async (context) => {
  const world = await openDatabases(context);
  const [[args, options]] = world.ranOf('databases.list');
  assert.deepEqual(args, {});
  assert.equal(options.words.unlock, 'list the databases');
  assert.ok(options.where.slot);
  assert.equal(world.ran.length, 1);
  assert.equal(world.timers.length, 0);
  assert.equal(world.listeners.filter(([name]) => name === 'task').length, 0);
});

test('Read again reads again', async (context) => {
  const world = await openDatabases(context);
  world.state.list = list({ databases: [] });
  await world.press(buttonOf(world.view.main, 'Read again'));
  assert.equal(world.ranOf('databases.list').length, 2);
  assert.match(world.view.main.textContent, /No databases yet\./);
});

test('a reader is told who sees the list, and nothing is asked of the server', async (context) => {
  const world = await openDatabases(context, { session: { admin: false } });
  assert.match(world.view.main.textContent, /Only an administrator can list the databases: listing them signs in to PostgreSQL as its superuser\./);
  assert.equal(world.ran.length, 0);
  assert.equal(world.view.main.querySelectorAll('table').length, 0);
  assert.equal(buttonOf(world.view.main, 'Unlock'), undefined);
  assert.deepEqual(world.inlines, [['databases.list', {}]]);
});

test('an administrator whose console is locked is asked to unlock, and the list is read when it is', async (context) => {
  const world = await openDatabases(context, { unlocked: false });
  assert.match(world.view.main.textContent, /the console has to be unlocked first/);
  assert.equal(world.ran.length, 0);
  assert.ok(buttonOf(world.view.main, 'Unlock'));
  setUnlockSeconds(900);
  await world.emit('unlock');
  assert.equal(world.ranOf('databases.list').length, 1);
  assert.equal(rowsOf(world.view.main).length, 3);
  assert.equal(buttonOf(world.view.main, 'Unlock'), undefined);
  // It locks again: the list is taken away with the words, and nothing is read.
  setUnlockSeconds(null);
  await world.emit('unlock');
  assert.equal(rowsOf(world.view.main).length, 0);
  assert.match(world.view.main.textContent, /unlocked first/);
  assert.equal(world.ranOf('databases.list').length, 1);
});

test('PostgreSQL that is not installed is said, with no table', async (context) => {
  const { view } = await openDatabases(context, { world: { list: missing() } });
  assert.match(view.main.querySelector('.banner').textContent, /PostgreSQL is not installed\./);
  assert.equal(view.main.querySelectorAll('table').length, 0);
  assert.doesNotMatch(view.main.textContent, /No databases yet/);
});

test('PostgreSQL that is off has Turn it on, which turns the service on and reads again', async (context) => {
  const world = await openDatabases(context, { world: { list: off() } });
  assert.match(world.view.main.querySelector('.banner').textContent, /PostgreSQL is off\./);
  assert.match(world.view.main.textContent, /Its first start makes the cluster\./);
  world.state.list = list();
  await world.press(buttonOf(world.view.main, 'Turn it on'));
  const [[args, options]] = world.ranOf('service.enable');
  assert.deepEqual(args, { name: 'postgresql' });
  assert.equal(options.words.unlock, 'turn PostgreSQL on');
  assert.equal(world.ranOf('databases.list').length, 2);
  assert.equal(rowsOf(world.view.main).length, 3);
  assert.deepEqual(world.inlines.at(-1)?.[0] ?? '', 'databases.list');
});

test('PostgreSQL that is enabled and does not run says so, with its service', async (context) => {
  const { view } = await openDatabases(context, { world: { list: list({ server: server({ running: false }), databases: [] }) } });
  assert.match(view.main.querySelector('.banner').textContent, /PostgreSQL is not running\./);
  assert.equal(view.main.querySelector('.banner a').getAttribute('href'), '/services/postgresql');
});

test('a server with no database says so, with a link to Add a database', async (context) => {
  const { view } = await openDatabases(context, { world: { list: empty() } });
  assert.match(view.main.querySelector('.db-empty').textContent, /No databases yet\./);
  assert.equal(view.main.querySelector('.db-empty a').getAttribute('href'), '/databases/add');
  assert.equal(view.main.querySelectorAll('table').length, 0);
});

test('what PostgreSQL found wrong in its own files is shown, with the page of its error', async (context) => {
  const { view } = await openDatabases(context, { world: { list: broken() } });
  const shown = view.main.querySelector('.db-problems');
  assert.match(shown.textContent, /invalid authentication method "scram" \(line 41 of pg_hba\.conf\)/);
  assert.equal(shown.querySelector('a').getAttribute('href'), '/help/errors/DB-104');
});

test('a read that fails is drawn where it belongs, with Try again; what was drawn stays when a later read fails', async (context) => {
  const world = await openDatabases(context, { world: { runs: { 'databases.list': refusal('DB-101', 'PostgreSQL does not let root in') } } });
  assert.equal(world.problems.length, 1);
  assert.match(world.view.main.querySelector('.db-errors').textContent, /PostgreSQL does not let root in/);
  assert.ok(buttonOf(world.view.main, 'Try again'));
  assert.equal(world.view.main.querySelectorAll('table').length, 0);

  const kept = await openDatabases(context);
  kept.state.list = null;
  await kept.press(buttonOf(kept.view.main, 'Read again'));
  assert.equal(rowsOf(kept.view.main).length, 3);
});

test('a page that is left stops what it was doing: a read that answers after is not drawn', async (context) => {
  const late = [];
  const world = await openDatabases(context, {
    world: { runs: { 'databases.list': () => new Promise((resolve) => late.push(resolve)) } },
  });
  world.leave();
  late[0]({ ok: true, result: list() });
  await settle();
  assert.equal(world.view.main.querySelectorAll('table').length, 0);
});

test('the routes: the list for nothing, Add a database for add and a database for a name', async (context) => {
  const add = await openDatabases(context, { rest: 'add' });
  assert.equal(add.view.headed.title, 'Add a database');
  const one = await openDatabases(context, { rest: 'shop' });
  assert.equal(one.view.headed.title, 'shop');
  const odd = await openDatabases(context, { rest: '%61dd' });
  assert.equal(odd.view.headed.title, 'add');
  const plain = await openDatabases(context, { rest: '' });
  assert.equal(plain.view.headed.title, 'Databases');
});
