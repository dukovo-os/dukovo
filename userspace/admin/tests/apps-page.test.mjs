// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Apps pages, drawn on a page of the test's own with a view that answers
// as admind does: the list and what it says of each app, an app's page for an
// administrator and for a reader, the buttons and the calls they make, the
// tasks that run on an app, and the dialogs that go back and remove. What the
// pages look like is the lab's web-console/apps test.

import assert from 'node:assert/strict';
import test from 'node:test';

import { list, plain, task, valkey } from './apps-fixtures.mjs';
import {
  control, openApps, refusal, rowsOf, settle,
} from './apps-view.mjs';

const VALKEY = 'io%2Evalkey%2Evalkey';

/** Types `text` in `input` as the browser would. */
function type(input, text) {
  input.value = text;
  input.fire('input');
}

/** The task a press began, as the runner answers it. */
const began = (id) => ({ ok: true, task: id, result: { task: id } });

// ---- the list ----

test('the list has a row for each app installed for everyone, the flawed first, with its version and what is to be done', async (context) => {
  const app = await openApps(context);
  assert.equal(app.view.headed.title, 'Apps');
  assert.deepEqual(app.view.headed.crumbs, []);
  const rows = rowsOf(app.view.main.querySelectorAll('table')[0]);
  assert.deepEqual(rows.map((row) => row[0].replace(/\s+/g, ' ')), [
    'VValkeyio.valkey.valkey', 'PPHP-FPMnet.php.php-fpm', 'PPostgreSQLorg.postgresql.postgresql', 'Nnginxorg.nginx.nginx', 'PPythonorg.python.python', 'SSQLiteorg.sqlite.sqlite',
  ]);
  assert.deepEqual(rows.map((row) => row[1]), ['8.0.3', '8.4.2', '17.2', '1.27.3', '3.15.2', '3.47.2']);
  assert.deepEqual(rows.map((row) => row[2]), [
    '8.0.4 · fixes only2 known flaws, worst high',
    'Reaches this machine on day 3 of its waves',
    '17.3 · new version',
    'Up to date',
    'Up to date',
    'Held at 3.47.2',
  ]);
  assert.deepEqual(app.view.main.querySelectorAll('th').slice(0, 3).map((one) => one.textContent), ['App', 'Version', 'Update']);
});

test('each row is a link to the app\'s page, with the dots of its ID written out, and a flaw rated high is in the warning colour', async (context) => {
  const app = await openApps(context);
  const links = app.view.main.querySelectorAll('table')[0].querySelectorAll('a').map((one) => one.getAttribute('href'));
  assert.equal(links[0], `/apps/${VALKEY}`);
  assert.equal(links.length, 6);
  assert.ok(links.every((href) => !href.slice(1).includes('.')), 'no dot in an address');
  assert.equal(app.view.main.querySelector('small.warn').textContent, '2 known flaws, worst high');
});

test('the line over the table says when the apps were checked, and Update all names how many apps it updates, with its command under it', async (context) => {
  const app = await openApps(context);
  assert.equal(app.view.main.querySelector('.apps-tools p').textContent, 'Checked 4 minutes ago.');
  assert.equal(control(app.view.main, 'update-all').textContent, 'Update all (2)');
  assert.deepEqual(app.inlines.at(-1), ['apps.update-all', {}]);
  await app.press(control(app.view.main, 'update-all'));
  assert.deepEqual(app.ran('apps.update-all'), [{}]);
});

test('the apps a person installed for themselves are apart, with their commands and no link', async (context) => {
  const app = await openApps(context);
  const yours = app.view.main.querySelector('.apps-yours');
  assert.equal(yours.querySelector('h2').textContent, 'Yours');
  assert.deepEqual(yours.querySelectorAll('th').map((one) => one.textContent), ['App', 'Version', 'Commands']);
  assert.equal(yours.querySelectorAll('a').length, 0);
  assert.deepEqual(yours.querySelectorAll('pre').map((one) => one.textContent), ['bundle update org.example.tool', 'bundle remove org.example.tool']);
  assert.equal(app.ran('apps.update').length, 0, 'no action runs them');
});

test('a reader sees the list and no button, and the head has no Add an app', async (context) => {
  const app = await openApps(context, { session: { admin: false } });
  assert.equal(control(app.view.main, 'update-all'), null);
  assert.deepEqual(app.view.headed.buttons.filter(Boolean), []);
  assert.equal(rowsOf(app.view.main.querySelectorAll('table')[0]).length, 6);
});

test('with a page to add an app, an administrator\'s head links to it and has no line of commands', async (context) => {
  const app = await openApps(context);
  const [add] = app.view.headed.buttons.filter(Boolean);
  assert.deepEqual([add.getAttribute('href'), add.textContent], ['/apps/add', 'Add an app']);
  assert.equal(app.view.headed.lead, null);
  assert.equal(app.view.headed.commands, true);
});

test('with no app installed the list says so', async (context) => {
  const app = await openApps(context, { world: { reads: { '/api/1/apps': { bundles: [] } } } });
  assert.equal(app.view.main.querySelector('.apps-empty').textContent, 'No apps yet.');
  assert.equal(app.view.main.querySelectorAll('table').length, 0);
});

test('a check that cannot be read does not stop the list, and says so instead of "up to date"', async (context) => {
  const app = await openApps(context, { world: { reads: { '/api/1/updates': refusal('ADM-123', 'No', 422) } } });
  assert.equal(app.view.main.querySelector('.apps-tools p').textContent, 'The last check could not be read.');
  assert.ok(rowsOf(app.view.main.querySelectorAll('table')[0]).some((row) => row[2] === 'Not known'));
  assert.ok(!app.view.main.textContent.includes('Up to date'));
});

test('a list that cannot be read is a problem with a way to try again, and nothing is drawn', async (context) => {
  const app = await openApps(context, { world: { reads: { '/api/1/apps': refusal('ADM-103', 'The server is busy', 503) } } });
  assert.equal(app.problems.length, 1);
  assert.equal(app.problems[0][1].buttons.length, 1);
  assert.equal(app.view.headed.title, 'Apps');
  assert.equal(app.view.main.querySelector('.apps').hidden, true);
});

test('the list is read again each 30 seconds and when a change says that something moved', async (context) => {
  const app = await openApps(context);
  assert.deepEqual(app.timers.map(([ms]) => ms), [30_000]);
  assert.equal(app.listeners.change.length, 1);
  const before = app.calls.filter((path) => path === '/api/1/apps').length;
  app.answers({ '/api/1/apps': list() });
  await app.timers[0][1]();
  await settle();
  assert.equal(app.calls.filter((path) => path === '/api/1/apps').length, before + 1);
});

// ---- an app's page ----

test('an app\'s page has its heading under the crumb to the list, the list beside it with its row marked, and a banner with the update and the flaws', async (context) => {
  const app = await openApps(context, { rest: VALKEY });
  assert.equal(app.view.headed.title, 'Valkey');
  assert.deepEqual(app.view.headed.crumbs, [{ title: 'Apps', path: '/apps' }]);
  assert.equal(app.view.headed.lead, null);
  assert.equal(app.view.main.querySelector('.apps').getAttribute('class'), 'apps has-app');
  assert.equal(app.view.main.querySelector('tr.sel a').getAttribute('aria-current'), 'page');
  const panel = app.view.main.querySelector('.apps-panel');
  assert.equal(panel.querySelector('h2').textContent, 'Valkey 8.0.3');
  assert.equal(panel.getAttribute('aria-labelledby'), 'apps-h');
  assert.ok(panel.textContent.includes('Dukovo catalog · signed · BSD-3-Clause'));
  const banner = panel.querySelector('.banner');
  assert.equal(banner.getAttribute('class'), 'banner bad');
  assert.ok(banner.textContent.includes('8.0.4 is available (fixes only). 8.0.3 has 2 known flaws, worst high; 8.0.4 fixes 1 of them.'));
  assert.ok(banner.textContent.includes('DKSA-2026-0007 · high · valkey: a crafted command stops the server Fixed in 8.0.4, which the update brings.'));
  assert.ok(banner.textContent.includes('Whether a flaw matters here depends on what the app may use'));
});

test('the facts of an app are its service with its state, its account, what it may use, what it provides, what uses it, its data, its size and the rule for updates', async (context) => {
  const app = await openApps(context, { rest: VALKEY });
  const facts = new Map(app.view.main.querySelectorAll('.apps-panel dl dt').map((term, at) => [term.textContent, app.view.main.querySelectorAll('.apps-panel dl dd')[at]]));
  assert.deepEqual([...facts.keys()], ['Service', 'Runs as', 'May use', 'Provides', 'Used by', 'Data', 'Size', 'Updates']);
  assert.equal(facts.get('Service').textContent, 'valkey · Stopped, off at boot');
  assert.equal(facts.get('Service').querySelector('a').getAttribute('href'), '/services/valkey');
  assert.equal(facts.get('Service').querySelector('.st').getAttribute('class'), 'st off');
  assert.equal(facts.get('Runs as').textContent, 'its own account, valkey');
  assert.deepEqual(facts.get('May use').querySelectorAll('li').map((one) => one.textContent), [
    'keep its data in its own folder · folders /shared/services/io.valkey.valkey (read and write)', 'listen for connections · port 6379', 'nothing else',
  ]);
  assert.equal(facts.get('Provides').textContent, 'redis at /run/valkey/valkey.sock');
  assert.equal(facts.get('Used by').querySelector('a').getAttribute('href'), '/apps/net%2Ephp%2Ephp-fpm');
  assert.equal(facts.get('Used by').textContent, 'PHP-FPM (sessions)');
  assert.ok(facts.get('Data').textContent.startsWith('/shared/services/io.valkey.valkey/valkey'));
  assert.ok(facts.get('Data').textContent.includes('edits made by hand stay through updates'));
  assert.equal(facts.get('Size').textContent, '9.44 MB');
  assert.equal(facts.get('Updates').textContent, 'Tell me · Change on the Updates page');
  assert.equal(facts.get('Updates').querySelector('a').getAttribute('href'), '/updates');
});

test('nginx\'s page says how many websites there are and PostgreSQL\'s how many databases, each with a link to manage them, and no other app\'s page has such a line', async (context) => {
  const web = await openApps(context, { rest: 'org%2Enginx%2Enginx' });
  const row = (app, name) => app.view.main.querySelectorAll('.apps-panel dl dt').findIndex((term) => term.textContent === name);
  const at = row(web, 'Websites');
  assert.notEqual(at, -1);
  const line = web.view.main.querySelectorAll('.apps-panel dl dd')[at];
  assert.equal(line.textContent, '2 · Manage');
  assert.equal(line.querySelector('a').getAttribute('href'), '/websites');
  assert.ok(web.calls.includes('/api/1/websites'));
  assert.ok(!web.calls.includes('/api/1/databases'));
  const data = await openApps(context, { rest: 'org%2Epostgresql%2Epostgresql' });
  const where = row(data, 'Databases');
  assert.notEqual(where, -1);
  assert.equal(data.view.main.querySelectorAll('.apps-panel dl dd')[where].textContent, '3 · Manage');
  assert.equal(data.view.main.querySelectorAll('.apps-panel dl dd')[where].querySelector('a').getAttribute('href'), '/databases');
  const other = await openApps(context, { rest: VALKEY });
  assert.equal(row(other, 'Websites'), -1);
  assert.equal(row(other, 'Databases'), -1);
  assert.ok(!other.calls.includes('/api/1/websites'));
});

test('a reader is not asked for the databases, which only an administrator may list, and is given the link without a count', async (context) => {
  const app = await openApps(context, { rest: 'org%2Epostgresql%2Epostgresql', session: { admin: false } });
  assert.ok(!app.calls.includes('/api/1/databases'));
  const terms = app.view.main.querySelectorAll('.apps-panel dl dt');
  const at = terms.findIndex((term) => term.textContent === 'Databases');
  assert.equal(app.view.main.querySelectorAll('.apps-panel dl dd')[at].textContent, 'Manage');
  const web = await openApps(context, { rest: 'org%2Enginx%2Enginx', session: { admin: false } });
  assert.ok(web.calls.includes('/api/1/websites'), 'anyone may list the websites');
});

test('a list of websites that cannot be read leaves the line without a count, and the page stays', async (context) => {
  const app = await openApps(context, { rest: 'org%2Enginx%2Enginx', world: { reads: { '/api/1/websites': refusal('ADM-103', 'busy', 503) } } });
  const at = app.view.main.querySelectorAll('.apps-panel dl dt').findIndex((term) => term.textContent === 'Websites');
  assert.equal(app.view.main.querySelectorAll('.apps-panel dl dd')[at].textContent, 'Manage');
  assert.equal(app.problems.length, 0);
});

test('an app that nothing uses, and an older bundle info that does not say, are told apart', async (context) => {
  const none = await openApps(context, { rest: 'org%2Enginx%2Enginx' });
  assert.ok(none.view.main.querySelector('.apps-panel').textContent.includes('Nothing installed uses it.'));
  const older = await openApps(context, { rest: VALKEY, world: { reads: { '/api/1/apps/io.valkey.valkey': valkey({ used_by: undefined, provides: undefined, service_list: undefined }) } } });
  const text = older.view.main.querySelector('.apps-panel').textContent;
  assert.ok(!text.includes('Used by'));
  assert.ok(!text.includes('Nothing installed uses it.'));
  assert.ok(text.includes('valkey: the server'), 'the older lines for the services');
});

test('the versions are the one offered, the one installed and the one kept for going back, with Go back to this on the last', async (context) => {
  const app = await openApps(context, { rest: VALKEY });
  const table = app.view.main.querySelector('.apps-versions');
  assert.deepEqual(rowsOf(table).map((row) => row.map((cell) => cell.replace(/Go back to this$/, ''))), [
    ['8.0.4Fixes only', 'available'], ['8.0.3Installed 4 days ago', 'installed'], ['8.0.2Kept for going back', ''],
  ]);
  assert.equal(control(table, 'back').textContent, 'Go back to this');
  assert.deepEqual(app.inlines.find(([action]) => action === 'apps.rollback'), ['apps.rollback', { id: 'io.valkey.valkey' }]);
});

test('an app with no version kept says that an update keeps the one it replaces, and has no Go back', async (context) => {
  const app = await openApps(context, { rest: 'org%2Enginx%2Enginx' });
  assert.ok(app.view.main.querySelector('.apps-versions').textContent.includes('No earlier version is kept yet'));
  assert.equal(control(app.view.main, 'back'), null);
});

test('an administrator has Update, Hold, Settings and Remove, each with its command under it', async (context) => {
  const app = await openApps(context, { rest: VALKEY });
  const buttons = app.view.main.querySelector('.apps-buttons');
  assert.deepEqual(buttons.querySelectorAll('.btn').map((one) => one.textContent), ['Update to 8.0.4', 'Hold at 8.0.3', 'Settings…', 'Remove…']);
  assert.equal(control(buttons, 'settings').getAttribute('href'), '/services/valkey');
  assert.deepEqual(app.inlines.filter(([action]) => action !== 'apps.rollback').map(([action]) => action), ['apps.update-all', 'apps.update', 'apps.hold', 'apps.remove']);
});

test('a reader sees every fact and no button, and is told who can', async (context) => {
  const app = await openApps(context, { rest: VALKEY, session: { admin: false } });
  const panel = app.view.main.querySelector('.apps-panel');
  assert.ok(panel.textContent.includes('Dukovo catalog · signed'));
  assert.equal(panel.querySelectorAll('button').length, 0);
  assert.equal(panel.querySelectorAll('.btn').length, 0);
  assert.ok(panel.textContent.includes('An administrator can update, hold, go back and remove apps.'));
  assert.equal(control(panel, 'back'), null);
});

test('an app held at its version says so, and has Release the hold in place of Update', async (context) => {
  const held = valkey({ held: true });
  const bundles = list();
  bundles.bundles[1].held = true;
  const app = await openApps(context, { rest: VALKEY, world: { reads: { '/api/1/apps/io.valkey.valkey': held, '/api/1/apps': bundles } } });
  const panel = app.view.main.querySelector('.apps-panel');
  assert.deepEqual(panel.querySelectorAll('.apps-buttons .btn').map((one) => one.textContent), ['Release the hold', 'Settings…', 'Remove…']);
  assert.ok(panel.textContent.includes('Held at 8.0.3: no update is taken, security fixes included. Release the hold to take updates again.'));
  assert.ok(panel.querySelector('.banner').textContent.includes('8.0.4 is available, and 8.0.3 is held.'));
});

test('an app that is a person\'s own has its commands and no button', async (context) => {
  const own = plain('org.example.tool', 'Tool', '1.0.0', { for_everyone: false });
  const app = await openApps(context, { rest: 'org%2Eexample%2Etool', world: { reads: { '/api/1/apps/org.example.tool': own } } });
  const panel = app.view.main.querySelector('.apps-panel');
  assert.deepEqual(panel.querySelectorAll('button').map((one) => one.textContent), ['Copy', 'Copy'], 'only the buttons that copy a command');
  assert.deepEqual(panel.querySelectorAll('pre').map((one) => one.textContent), ['bundle update org.example.tool', 'bundle remove org.example.tool']);
  assert.ok(panel.textContent.includes('This app is yours, not everyone\'s.'));
});

test('an app that is not installed says so, with a link to the list, and one that cannot be read while the list has it is a problem', async (context) => {
  const missing = await openApps(context, { rest: 'io%2Eexample%2Egone' });
  assert.equal(missing.view.main.querySelector('.apps-missing').textContent, 'No app called io.example.gone is installed here.All the apps');
  assert.equal(missing.view.main.querySelector('.apps-missing a').getAttribute('href'), '/apps');
  assert.equal(missing.view.main.querySelector('.apps-missing').hidden, false);
  assert.equal(missing.view.main.querySelector('.apps-panel').hidden, true);
  assert.equal(missing.view.headed.title, 'Apps');
  assert.equal(missing.problems.length, 0);
  const broken = await openApps(context, { rest: VALKEY, world: { reads: { '/api/1/apps/io.valkey.valkey': refusal('ADM-103', 'busy', 503) } } });
  assert.equal(broken.problems.length, 1);
});

test('an address that cannot be an app says so in its own words', async (context) => {
  const app = await openApps(context, { rest: '..%2Fx' });
  assert.equal(app.view.main.querySelector('.apps-missing').textContent, 'No app called ..%2Fx is installed here.All the apps');
  assert.ok(!app.calls.some((path) => path.startsWith('/api/1/apps/')));
});

// ---- what the buttons do ----

test('Hold runs apps.hold, then reads again, and focus goes to the button that is there now', async (context) => {
  const app = await openApps(context, { rest: VALKEY });
  const before = app.calls.filter((path) => path === '/api/1/apps/io.valkey.valkey').length;
  app.answers({ '/api/1/apps/io.valkey.valkey': valkey({ held: true }) });
  await app.press(control(app.view.main, 'hold'));
  assert.deepEqual(app.ran('apps.hold'), [{ id: 'io.valkey.valkey' }]);
  assert.equal(app.calls.filter((path) => path === '/api/1/apps/io.valkey.valkey').length, before + 1);
  const now = control(app.view.main, 'hold');
  assert.equal(now.textContent, 'Release the hold');
  assert.equal(app.document.activeElement, now);
  app.answers({ '/api/1/apps/io.valkey.valkey': valkey() });
  await app.press(now);
  assert.deepEqual(app.ran('apps.unhold'), [{ id: 'io.valkey.valkey' }]);
});

test('Update runs apps.update, keeps the task, and the row and the page say how far it is while the buttons are off', async (context) => {
  const app = await openApps(context, { rest: VALKEY, world: { answers: { 'apps.update': began(7) } } });
  await app.press(control(app.view.main, 'update'));
  assert.deepEqual(app.ran('apps.update'), [{ id: 'io.valkey.valkey' }]);
  assert.equal(app.view.main.querySelector('.apps-job').textContent, 'Updating…');
  assert.equal(app.view.main.querySelector('.apps-job').hidden, false);
  assert.equal(control(app.view.main, 'hold').getAttribute('aria-disabled'), 'true');
  assert.equal(control(app.view.main, 'remove').getAttribute('aria-disabled'), 'true');
  const cell = app.view.main.querySelector('td[data-app="io.valkey.valkey"]');
  assert.equal(cell.querySelector('.apps-now').textContent, 'Updating…');
  assert.equal(cell.querySelector('.apps-next').hidden, true);
  await app.emit('task', { id: 7, action: 'apps.update', state: 'running', percent: 41 });
  assert.equal(app.view.main.querySelector('.apps-job').textContent, 'Updating… 41%');
  assert.equal(cell.querySelector('.apps-now').textContent, 'Updating… 41%');
  await app.press(control(app.view.main, 'hold'));
  assert.equal(app.ran('apps.hold').length, 0, 'a button that is off does nothing');
  await app.emit('task', { id: 7, action: 'apps.update', state: 'done' });
  assert.equal(app.view.main.querySelector('.apps-job').hidden, true);
  assert.equal(control(app.view.main, 'hold').getAttribute('aria-disabled'), null);
  assert.equal(cell.querySelector('.apps-next').hidden, false);
});

test('a task that was running when the page opened is shown at once, and one that began elsewhere is read when its event comes', async (context) => {
  const app = await openApps(context, { rest: VALKEY, world: { reads: { '/api/1/tasks': { tasks: [task(3, 'apps.update', { id: 'io.valkey.valkey' }, { percent: 10 })] } } } });
  assert.equal(app.view.main.querySelector('.apps-job').textContent, 'Updating… 10%');
  const other = await openApps(context, { rest: VALKEY });
  other.answers({ '/api/1/tasks/9': task(9, 'apps.rollback', { id: 'io.valkey.valkey' }) });
  await other.emit('task', { id: 9, action: 'apps.rollback', state: 'running' });
  assert.equal(other.view.main.querySelector('.apps-job').textContent, 'Going back…');
});

test('the task that updates every app shows on the rows that have an update, and on no other', async (context) => {
  const app = await openApps(context, { world: { answers: { 'apps.update-all': began(12) } } });
  await app.press(control(app.view.main, 'update-all'));
  const words = app.view.main.querySelectorAll('td[data-app]').map((cell) => [cell.getAttribute('data-app'), cell.querySelector('.apps-now').hidden]);
  assert.deepEqual(words.filter(([, hidden]) => !hidden).map(([id]) => id).sort(), ['io.valkey.valkey', 'org.postgresql.postgresql']);
  assert.equal(control(app.view.main, 'update-all').getAttribute('aria-disabled'), 'true');
});

test('an update that stopped to ask shows its sheet, and Update anyway runs the update with a yes', async (context) => {
  const app = await openApps(context, { rest: VALKEY, world: { answers: { 'apps.update': began(7) } } });
  await app.press(control(app.view.main, 'update'));
  app.answers({
    '/api/1/tasks/7': task(7, 'apps.update', { id: 'io.valkey.valkey' }, {
      state: 'failed', log: ['valkey 8.0.4 asks for more than 8.0.3 does:', '  - it may listen on all addresses', 'Install it?'],
    }),
  });
  await app.emit('task', { id: 7, action: 'apps.update', state: 'failed' });
  const panel = app.view.main.querySelector('.apps-panel');
  assert.ok(panel.textContent.includes('This update asks for more than the version installed does.'));
  assert.equal(panel.querySelector('.apps-sheet').textContent, 'valkey 8.0.4 asks for more than 8.0.3 does:\n  - it may listen on all addresses');
  assert.ok(app.inlines.some(([action, args]) => action === 'apps.update' && args.yes === true), 'its command is under it');
  await app.press(control(panel, 'update-yes'));
  assert.deepEqual(app.ran('apps.update'), [{ id: 'io.valkey.valkey' }, { id: 'io.valkey.valkey', yes: true }]);
  assert.equal(app.view.main.querySelector('.apps-sheet'), null, 'the sheet goes when the update is asked again');
});

test('an update that failed for another reason shows no sheet', async (context) => {
  const app = await openApps(context, { rest: VALKEY, world: { answers: { 'apps.update': began(7) } } });
  await app.press(control(app.view.main, 'update'));
  app.answers({ '/api/1/tasks/7': task(7, 'apps.update', { id: 'io.valkey.valkey' }, { state: 'failed', log: ['bundle: no route to the catalog'] }) });
  await app.emit('task', { id: 7, action: 'apps.update', state: 'failed' });
  assert.equal(app.view.main.querySelector('.apps-sheet'), null);
});

// ---- the dialogs ----

test('Go back to this asks first, in the words of what it does to the data, and then runs apps.rollback', async (context) => {
  const app = await openApps(context, { rest: VALKEY, world: { answers: { 'apps.rollback': began(8) } } });
  await app.press(control(app.view.main, 'back'));
  const dialog = app.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Go back to 8.0.2?');
  assert.ok(dialog.textContent.includes('Goes back to 8.0.2 and holds it there. Its data comes back as 8.0.2 left it'));
  assert.equal(app.ran('apps.rollback').length, 0);
  await app.submit();
  assert.deepEqual(app.ran('apps.rollback'), [{ id: 'io.valkey.valkey' }]);
  assert.equal(app.dialog(), null);
  assert.equal(app.view.main.querySelector('.apps-job').textContent, 'Going back…');
});

test('Cancel in a dialog runs nothing, and focus comes back to the button that opened it', async (context) => {
  const app = await openApps(context, { rest: VALKEY });
  const opener = control(app.view.main, 'back');
  opener.focus();
  await app.press(opener);
  const cancel = app.dialog().querySelectorAll('button').find((one) => one.textContent === 'Cancel');
  cancel.click();
  await settle();
  assert.equal(app.dialog(), null);
  assert.equal(app.ran('apps.rollback').length, 0);
  assert.equal(app.document.activeElement, control(app.view.main, 'back'));
});

test('the Show command panel is in a dialog only while commands are shown, and follows what the dialog asks', async (context) => {
  const off = await openApps(context, { rest: VALKEY });
  await off.press(control(off.view.main, 'remove'));
  assert.equal(off.dialog().querySelector('.apps-command'), null);
  const on = await openApps(context, { rest: VALKEY, prefs: { commands: 'shown' } });
  await on.press(control(on.view.main, 'remove'));
  assert.deepEqual(on.panels.at(-1), ['apps.remove', { id: 'io.valkey.valkey', data: 'keep' }]);
});

test('Remove asks what becomes of the data, with Keep its data chosen, and names the apps that use the app', async (context) => {
  const app = await openApps(context, { rest: VALKEY });
  await app.press(control(app.view.main, 'remove'));
  const dialog = app.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Remove Valkey?');
  assert.ok(dialog.textContent.includes('Valkey 8.0.3 is removed for everyone, and its services stop and go.'));
  assert.ok(dialog.textContent.includes('PHP-FPM uses it: removing it stops PHP-FPM\'s sessions.'));
  const radios = dialog.querySelectorAll('input').filter((one) => one.getAttribute('type') === 'radio');
  assert.deepEqual(radios.map((one) => [one.getAttribute('value'), one.hasAttribute('checked')]), [['keep', true], ['purge', false]]);
  assert.deepEqual(dialog.querySelectorAll('.radio-card b').map((one) => one.textContent), ['Keep its data', 'Remove its data too']);
  assert.equal(dialog.querySelector('.field').hidden, true, 'the name is asked only for the data to go');
});

test('Remove with Keep its data runs apps.remove with keep, and the app\'s page goes when the task is done', async (context) => {
  const app = await openApps(context, { rest: VALKEY, world: { answers: { 'apps.remove': began(11) } } });
  await app.press(control(app.view.main, 'remove'));
  await app.submit();
  assert.deepEqual(app.ran('apps.remove'), [{ id: 'io.valkey.valkey', data: 'keep' }]);
  assert.equal(app.view.main.querySelector('.apps-job').textContent, 'Removing…');
  assert.deepEqual(app.went, []);
  await app.emit('task', { id: 11, action: 'apps.remove', state: 'done' });
  assert.deepEqual(app.went, ['/apps']);
});

test('Remove its data too asks for the app\'s name, and runs nothing until it is typed as written', async (context) => {
  const app = await openApps(context, { rest: VALKEY, world: { answers: { 'apps.remove': began(11) } } });
  await app.press(control(app.view.main, 'remove'));
  const dialog = app.dialog();
  const purge = dialog.querySelectorAll('input').find((one) => one.getAttribute('value') === 'purge');
  purge.fire('change');
  assert.equal(dialog.querySelector('.field').hidden, false);
  assert.equal(dialog.querySelector('.field label').textContent, 'Type Valkey to go on');
  assert.ok(dialog.querySelectorAll('.radio-card').find((one) => one.classes.has('on')).textContent.includes('Remove its data too'));
  const typed = dialog.querySelector('.field input');
  type(typed, 'valkey');
  await app.submit();
  assert.equal(app.ran('apps.remove').length, 0);
  assert.ok(dialog.textContent.includes('Type Valkey exactly as it is written'));
  assert.equal(typed.getAttribute('aria-invalid'), 'true');
  type(typed, 'Valkey');
  await app.submit();
  assert.deepEqual(app.ran('apps.remove'), [{ id: 'io.valkey.valkey', data: 'purge' }]);
});

test('Remove on an app that nothing uses says nothing of users, and a refusal stays in the dialog', async (context) => {
  const refused = refusal('ADM-105', 'The app is in use', 409);
  const app = await openApps(context, { rest: 'org%2Enginx%2Enginx', world: { answers: { 'apps.remove': refused } } });
  await app.press(control(app.view.main, 'remove'));
  assert.ok(!app.dialog().textContent.includes('uses it'));
  await app.submit();
  assert.ok(app.dialog(), 'the dialog stays open for the person to put right');
  assert.equal(app.problems.at(-1)[1].slot.getAttribute('class'), 'apps-errors');
});

test('a locked administrator is asked to unlock, in words that name the change, before the dialog and not after a name is typed', async (context) => {
  const app = await openApps(context, { rest: VALKEY, unlocked: false, session: { kinds: ['password'], host: 'duke-nas' } });
  await app.press(control(app.view.main, 'remove'));
  assert.equal(app.dialog().querySelector('h2').textContent, 'Unlock to make changes');
  assert.ok(app.dialog().textContent.includes('You are about to remove Valkey.'));
  assert.equal(app.ran('apps.remove').length, 0);
});
