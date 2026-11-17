// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Restore… on a database's page (pages/databases/restore.js): the copies it
// lists, the choice of a new database or over the database, the restore as a
// task with its steps, the sessions that stop an over until the person says
// they may be ended, and the words that say where it went.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  copies, database, finished, list,
} from './databases-fixtures.mjs';
import {
  buttonOf, dialogsOf, fieldOf, openDatabases, refusal, type,
} from './databases-view.mjs';

const LIVE = '/shared/services/org.postgresql.postgresql/postgres/dumps/shop.dump';
const SNAPSHOT = '/snapshots/shared/2026-11-13-0200/services/org.postgresql.postgresql/postgres/dumps/shop.dump';

// The clock is the test's: nothing waits for real time, and a timer that was not stopped is seen.
const clocked = new WeakSet();
const open = (context, name, more = {}) => {
  if (!clocked.has(context)) {
    clocked.add(context);
    context.mock.timers.enable({ apis: ['setTimeout'] });
  }
  return openDatabases(context, { rest: name, ...more });
};

/** A restore that starts as the task `id`, which `reads` then answers. */
const task = (id) => ({ ok: true, task: id, result: { task: id } });

/** The radio of the dialog that says `words` in its row. */
const rowOf = (dialog, words) => dialog.querySelectorAll('label.radio-card').find((one) => one.textContent.includes(words)).querySelector('input');

/** Chooses the radio `input`, as a press does. */
const choose = (input) => {
  for (const other of input.closest('fieldset').querySelectorAll(`input[name="${input.getAttribute('name')}"]`)) other.checked = false;
  input.checked = true;
  input.closest('fieldset').fire('change');
};

/** The dialog of Restore… of `name`, opened. */
async function opened(context, name = 'shop', more = {}) {
  const world = await open(context, name, more);
  await world.press(buttonOf(world.view.main, 'Restore…'));
  return world;
}

test('Restore… asks for the copies as an administrator, newest first, with their time, size and place', async (context) => {
  const world = await opened(context);
  const [args, options] = world.ranOf('databases.copies')[0];
  assert.deepEqual(args, { name: 'shop' });
  assert.equal(options.words.unlock, 'list the copies of shop');
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Restore shop');
  const rows = dialog.querySelector('fieldset').querySelectorAll('label.radio-card');
  assert.deepEqual(rows.map((one) => one.textContent), ['Today 2:00 AM12 MB · This machine', 'Yesterday 2:00 AM11 MB · Snapshot 2026-11-13-0200']);
  assert.deepEqual(rows.map((one) => one.querySelector('input').checked), [true, false]);
  assert.equal(world.ranOf('databases.restore').length, 0);
});

test('the choices are a new database, which is the first, and over the database, which keeps it', async (context) => {
  const world = await opened(context);
  const dialog = world.dialog();
  assert.match(dialog.textContent, /As a new database/);
  assert.match(dialog.textContent, /Over shop/);
  assert.match(dialog.textContent, /The database as it is now is kept as shop_before_… and the day and time\./);
  assert.equal(rowOf(dialog, 'As a new database').checked, true);
  assert.equal(rowOf(dialog, 'Over shop').checked, false);
  assert.equal(fieldOf(dialog, 'Name of the new database').value, 'shop_restored_20261114');
});

test('a new database is restored as a task, its steps are shown, and its page is linked when it is done', async (context) => {
  const world = await opened(context, 'shop', {
    world: { runs: { 'databases.restore': task(9) }, reads: { '/api/1/tasks/9': finished(9, ['put the copy into the new database shop_restored_20261114; shop was not changed']) } },
  });
  await world.submit();
  assert.deepEqual(world.ranOf('databases.restore')[0][0], { name: 'shop', from: LIVE, as: 'shop_restored_20261114' });
  assert.equal(dialogsOf(world.document).length, 0);
  const flow = world.view.main.querySelector('.db-flow');
  assert.equal(flow.querySelector('h2').textContent, 'Restoring shop');
  assert.equal(flow.querySelector('.flow-step').getAttribute('data-state'), 'done');
  assert.match(flow.querySelector('.flow-step').textContent, /Restore shop into shop_restored_20261114/);
  const banner = flow.querySelector('.banner.ok');
  assert.equal(banner.textContent, 'Restored into shop_restored_20261114');
  assert.equal(banner.querySelector('a').getAttribute('href'), '/databases/shop_restored_20261114');
  assert.equal(world.ranOf('databases.list').length, 2, 'the list is read again after the restore');
});

test('another copy, and another name, are what is sent', async (context) => {
  const world = await opened(context, 'shop', { world: { runs: { 'databases.restore': task(9) }, reads: { '/api/1/tasks/9': finished(9) } } });
  choose(rowOf(world.dialog(), 'Snapshot 2026-11-13-0200'));
  type(fieldOf(world.dialog(), 'Name of the new database'), 'shop_old');
  await world.submit();
  assert.deepEqual(world.ranOf('databases.restore')[0][0], { name: 'shop', from: SNAPSHOT, as: 'shop_old' });
  assert.equal(world.view.main.querySelector('.db-flow .banner.ok a').getAttribute('href'), '/databases/shop_old');
});

test('a name that is not one, or is a database already, is said under the field and nothing is sent', async (context) => {
  const world = await opened(context);
  type(fieldOf(world.dialog(), 'Name of the new database'), 'Shop Old');
  await world.submit();
  assert.match(world.dialog().textContent, /lowercase letters, digits and _/);
  type(fieldOf(world.dialog(), 'Name of the new database'), 'blog');
  await world.submit();
  assert.match(world.dialog().textContent, /A database named blog is there already\./);
  type(fieldOf(world.dialog(), 'Name of the new database'), '');
  await world.submit();
  assert.match(world.dialog().textContent, /Give the database a name\./);
  assert.equal(world.ranOf('databases.restore').length, 0);
  assert.equal(dialogsOf(world.document).length, 1);
});

test('the name a restore gets is the day after it, and the next free one when that is taken', async (context) => {
  const taken = list({ databases: [database('shop'), database('shop_restored_20261114'), database('shop_restored_20261114_2')] });
  const world = await opened(context, 'shop', { world: { list: taken } });
  assert.equal(fieldOf(world.dialog(), 'Name of the new database').value, 'shop_restored_20261114_3');
  const long = `a${'b'.repeat(54)}`;
  const big = await opened(context, long, { world: { list: list({ databases: [database(long)] }) } });
  assert.equal(fieldOf(big.dialog(), 'Name of the new database').value, '', 'a name that is too long to add to is left to the person');
  assert.doesNotMatch(big.dialog().textContent, /Over a/, 'and so is one that cannot be kept under another name');
});

test('over the database keeps it, and the page of the database it was kept as is linked', async (context) => {
  const world = await opened(context, 'shop', { world: { runs: { 'databases.restore': task(9) }, reads: { '/api/1/tasks/9': finished(9) } } });
  choose(rowOf(world.dialog(), 'Over shop'));
  assert.equal(fieldOf(world.dialog(), 'Name of the new database').closest('.db-sub').hidden, true);
  // The list read after the restore has the database that was kept.
  world.state.list = list({ databases: [...list().databases, database('shop_before_20261114_1012')] });
  await world.submit();
  assert.deepEqual(world.ranOf('databases.restore')[0][0], { name: 'shop', from: LIVE, over: true });
  const flow = world.view.main.querySelector('.db-flow');
  assert.match(flow.querySelector('.flow-step').textContent, /Restore shop over itself/);
  const banner = flow.querySelector('.banner.ok');
  assert.equal(banner.textContent, 'Restored into shop; the database before is kept as shop_before_20261114_1012');
  assert.equal(banner.querySelector('a').getAttribute('href'), '/databases/shop_before_20261114_1012');
});

test('when the database that was kept is not in the list, the words say what its name starts with', async (context) => {
  const world = await opened(context, 'shop', { world: { runs: { 'databases.restore': task(9) }, reads: { '/api/1/tasks/9': finished(9) } } });
  choose(rowOf(world.dialog(), 'Over shop'));
  await world.submit();
  assert.match(world.view.main.querySelector('.db-flow .banner.ok').textContent, /kept under a name that starts with shop_before_: the list of databases has it\./);
  assert.equal(world.view.main.querySelector('.db-flow .banner.ok a'), null);
});

test('open sessions stop an over, and the person is asked before they are ended', async (context) => {
  const stopped = { id: 9, state: 'failed', status: 1, log: ['database restore: blog has 2 open sessions: psql, blog-php (DB-103)'] };
  const world = await opened(context, 'blog', {
    world: {
      runs: { 'databases.restore': (args) => task(args.force ? 10 : 9) },
      reads: { '/api/1/tasks/9': stopped, '/api/1/tasks/10': finished(10) },
    },
  });
  assert.match(world.dialog().textContent, /It has 2 open sessions: you are asked before they are ended\./);
  choose(rowOf(world.dialog(), 'Over blog'));
  await world.submit();
  assert.deepEqual(world.ranOf('databases.restore').map(([args]) => args.force), [undefined]);
  const question = dialogsOf(world.document)[0];
  assert.equal(question.querySelector('h2').textContent, 'End the sessions of blog?');
  assert.match(question.textContent, /blog has 2 open sessions: psql, blog-php/);
  assert.doesNotMatch(question.textContent, /DB-103/);
  assert.match(question.textContent, /Ending the sessions stops those programs at once, and what they were doing is lost\./);
  await world.press(buttonOf(question, 'End the sessions and restore'));
  assert.deepEqual(world.ranOf('databases.restore').map(([args]) => args), [
    { name: 'blog', from: '/shared/services/org.postgresql.postgresql/postgres/dumps/blog.dump', over: true },
    { name: 'blog', from: '/shared/services/org.postgresql.postgresql/postgres/dumps/blog.dump', over: true, force: true },
  ]);
  assert.equal(world.view.main.querySelector('.db-flow .flow-step').getAttribute('data-state'), 'done');
});

test('keeping the sessions leaves the restore stopped, with the error\'s page', async (context) => {
  const stopped = { id: 9, state: 'failed', status: 1, log: ['database restore: blog has 2 open sessions: psql (DB-103)'] };
  const world = await opened(context, 'blog', { world: { runs: { 'databases.restore': task(9) }, reads: { '/api/1/tasks/9': stopped } } });
  choose(rowOf(world.dialog(), 'Over blog'));
  await world.submit();
  await world.press(buttonOf(dialogsOf(world.document)[0], 'Keep them'));
  assert.equal(world.ranOf('databases.restore').length, 1);
  const step = world.view.main.querySelector('.flow-step');
  assert.equal(step.getAttribute('data-state'), 'failed');
  assert.ok(step.querySelectorAll('a').some((one) => one.getAttribute('href') === '/help/errors/DB-103'));
  assert.match(step.textContent, /blog is as it was\./);
  assert.equal(world.view.main.querySelector('.db-flow .banner.ok'), null);
});

test('a restore that fails for another reason says so and asks nothing', async (context) => {
  const failed = { id: 9, state: 'failed', status: 1, log: ['pg_restore: error: could not execute query (DB-110)'] };
  const world = await opened(context, 'shop', { world: { runs: { 'databases.restore': task(9) }, reads: { '/api/1/tasks/9': failed } } });
  await world.submit();
  assert.equal(dialogsOf(world.document).length, 0);
  const step = world.view.main.querySelector('.flow-step');
  assert.match(step.textContent, /The copy was not restored/);
  assert.ok(step.querySelectorAll('a').some((one) => one.getAttribute('href') === '/help/errors/DB-110'));
  assert.equal(world.view.main.querySelector('.db-flow .banner.ok'), null);
});

test('sessions that stop a restore into a new database are not offered to be ended', async (context) => {
  const stopped = { id: 9, state: 'failed', status: 1, log: ['database restore: blog has 2 open sessions (DB-103)'] };
  const world = await opened(context, 'blog', { world: { runs: { 'databases.restore': task(9) }, reads: { '/api/1/tasks/9': stopped } } });
  await world.submit();
  assert.equal(dialogsOf(world.document).length, 0);
  assert.equal(world.ranOf('databases.restore').length, 1);
});

test('a database with no copy says so in a notice and opens no dialog', async (context) => {
  const world = await open(context, 'shop', { world: { runs: { 'databases.copies': { ok: true, result: { format: 1, database: 'shop', copies: [] } } } } });
  await world.press(buttonOf(world.view.main, 'Restore…'));
  assert.equal(dialogsOf(world.document).length, 0);
  assert.deepEqual(world.notices, [['There is no copy of shop to put back. Copies are made before each backup.', { kind: 'warn' }]]);
});

test('copies that cannot be listed are said where the page puts its problems, and no dialog opens', async (context) => {
  const world = await open(context, 'shop', { world: { runs: { 'databases.copies': refusal('DB-109', 'The copies could not be read') } } });
  await world.press(buttonOf(world.view.main, 'Restore…'));
  assert.equal(dialogsOf(world.document).length, 0);
  assert.match(world.view.main.querySelector('.db-errors').textContent, /The copies could not be read/);
});

test('Cancel changes nothing, and the command of the restore is shown while commands are shown', async (context) => {
  const world = await opened(context, 'shop', { prefs: { commands: 'shown' } });
  assert.deepEqual(world.panels.at(-1), ['databases.restore', { name: 'shop', from: LIVE, as: 'shop_restored_20261114' }]);
  choose(rowOf(world.dialog(), 'Over shop'));
  world.dialog().querySelector('form').fire('input');
  context.mock.timers.tick(400);
  assert.deepEqual(world.panels.at(-1), ['databases.restore', { name: 'shop', from: LIVE, over: true }]);
  await world.press(buttonOf(world.dialog(), 'Cancel'));
  assert.equal(world.ranOf('databases.restore').length, 0);
  assert.equal(world.view.main.querySelector('.db-flow').children.length, 0);
});

test('the button is off while the restore runs', async (context) => {
  const world = await opened(context, 'shop', { world: { runs: { 'databases.restore': task(9) }, reads: { '/api/1/tasks/9': { id: 9, state: 'running', step: 2, steps: 5, words: 'Putting the copy in', log: [] } } } });
  await world.submit();
  const button = buttonOf(world.view.main, 'Restore…');
  assert.equal(button.disabled === true || button.hasAttribute('disabled'), true);
  assert.match(world.view.main.querySelector('.flow-step').textContent, /Step 2 of 5: Putting the copy in/);
});
