// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The page that adds an app, drawn on a page of the test's own with a view that
// answers as admind does: the search and the rows it finds, the setups, the
// catalogs that cannot be read, the sheet of an app for an administrator and
// for a reader, the install as a task pinned by the sheet's hash, the catalog
// that moved between the sheet and the install, and what comes after. What the
// page looks like is the lab's web-console/add-app test.

import assert from 'node:assert/strict';
import test from 'node:test';

import { installedList, nginx, notes, OTHER_SHA, php, search, SHA } from './catalog-fixtures.mjs';
import { plain } from './apps-fixtures.mjs';
import {
  cellsOf, control, failedWith, openAdd, PATHS, refusal,
} from './add-view.mjs';

const NGINX = 'org%2Enginx%2Enginx';
const SHEET = '/api/1/apps/catalog/org.nginx.nginx/sheet';

/** The words of a node, as they read: runs of spaces are one. */
const words = (node) => node.textContent.replace(/\s+/g, ' ').trim();

// ---- the list ----

test('the head has its crumb to Apps and the two views of the apps, with how many are installed', async (context) => {
  const app = await openAdd(context);
  assert.equal(app.view.headed.title, 'Add an app');
  assert.deepEqual(app.view.headed.crumbs, [{ title: 'Apps', path: '/apps' }]);
  assert.equal(app.view.headed.commands, true);
  const seg = app.view.headed.buttons[0];
  assert.deepEqual(seg.querySelectorAll('a').map((one) => [one.textContent, one.getAttribute('href'), one.getAttribute('aria-current')]), [
    ['Installed (1)', '/apps', null], ['Add an app', '/apps/add', 'page'],
  ]);
});

test('the page reads the installed apps and every bundle of the catalogs when it opens, and no sheet', async (context) => {
  const app = await openAdd(context);
  assert.ok(app.calls.includes(PATHS.apps));
  assert.ok(app.calls.includes(PATHS.search));
  assert.ok(!app.calls.some((path) => path.includes('/sheet')));
  assert.ok(!app.calls.includes(PATHS.backups));
});

test('the catalogs\' table has a row for each app, by name, with its version, catalog, flaws and whether it is installed', async (context) => {
  const app = await openAdd(context);
  assert.deepEqual(cellsOf(app.view.main.querySelector('table')), [
    ['Nnginxorg.nginx.nginx', '1.30.6', 'dukovo', 'No known flaws', 'Not installed'],
    ['NNotesorg.example.notes', '2.0.1', 'dukovo', 'Flaws not listed', 'Not installed'],
    ['PPHPnet.php.php', '8.4.3', 'dukovo', '1 known flaw, worst medium', 'Not installed'],
    ['PPostgreSQLorg.postgresql.postgresql', '17.3', 'dukovo', '2 known flaws, worst high', 'InstalledVersion 17.2'],
    ['Ppythonorg.python.python', '3.15.2', 'dukovo', 'No known flaws', 'Not installed'],
  ]);
  assert.deepEqual(app.view.main.querySelectorAll('th').map((one) => one.textContent), ['App', 'Version', 'Catalog', 'Flaws', 'Installed']);
  assert.equal(app.view.main.querySelector('caption').textContent, '5 apps in the catalogs');
  assert.equal(app.view.main.querySelector('tbody .warn').textContent, '2 known flaws, worst high');
});

test('a row is a link to the sheet of the app, with the dots of its ID written out', async (context) => {
  const app = await openAdd(context);
  const links = app.view.main.querySelector('table').querySelectorAll('a').map((one) => one.getAttribute('href'));
  assert.equal(links[0], '/apps/add/org%2Enginx%2Enginx');
  assert.equal(links.length, 5);
  assert.ok(links.every((href) => !href.slice(1).includes('.')), 'no dot in an address');
});

test('an app is installed by the list of installed apps when the search does not say', async (context) => {
  const found = search();
  found.bundles[0].installed = null;
  const app = await openAdd(context, { world: { reads: { [PATHS.search]: found, [PATHS.apps]: installedList(['org.nginx.nginx', 'nginx', '1.30.5']) } } });
  assert.equal(cellsOf(app.view.main.querySelector('table'))[0][4], 'InstalledVersion 1.30.5');
});

test('the setups are the four cards, each a link to its page with what it uses, and the ones whose apps are not installed say what they install first', async (context) => {
  const app = await openAdd(context);
  const cards = app.view.main.querySelectorAll('ul.setups a');
  assert.deepEqual(cards.map((one) => one.getAttribute('href')), ['/websites/add?kind=static', '/websites/add?kind=php&database=1', '/databases/add', '/security#private']);
  assert.deepEqual(cards.map((one) => one.querySelector('b').textContent), ['Static website', 'PHP website with a database', 'A database', 'Private access for your devices']);
  assert.deepEqual(cards.map((one) => one.querySelectorAll('.chip').map((chip) => chip.textContent)), [['nginx'], ['nginx', 'PHP-FPM', 'PostgreSQL'], ['PostgreSQL'], ['WireGuard']]);
  assert.deepEqual(cards.map((one) => one.querySelector('.warn')?.textContent ?? null), ['Installs nginx first', 'Installs nginx and PHP-FPM first', null, null]);
  assert.ok(words(app.view.main).includes('Each setup is a page of this console; it shows every command it runs before it runs it.'));
});

test('a reader sees the setups and the catalogs too', async (context) => {
  const app = await openAdd(context, { session: { admin: false } });
  assert.equal(app.view.main.querySelectorAll('ul.setups a').length, 4);
  assert.equal(app.view.main.querySelectorAll('tbody tr').length, 5);
});

// ---- the search ----

test('what is typed is searched 300 ms after the typing stops, once, in lowercase words', async (context) => {
  const app = await openAdd(context);
  const before = app.calls.filter((path) => path.startsWith(PATHS.search)).length;
  await app.type('W');
  await app.tick(200);
  await app.type('Web  Ser');
  await app.tick(299);
  assert.equal(app.calls.filter((path) => path.startsWith(PATHS.search)).length, before, 'nothing is sent while the typing goes on');
  await app.tick(1);
  assert.equal(app.calls.at(-1), '/api/1/apps/catalog/search?words=web%20ser');
  assert.equal(app.calls.filter((path) => path.startsWith(PATHS.search)).length, before + 1);
});

test('the address keeps what was typed, and the rows and the way back keep it', async (context) => {
  const app = await openAdd(context);
  await app.type('pg');
  assert.equal(app.kept.at(-1), '/apps/add?q=pg');
  await app.tick(300);
  assert.ok(app.view.main.querySelector('table').querySelectorAll('a')[0].getAttribute('href').endsWith('?q=pg'));
  await app.type('');
  assert.equal(app.kept.at(-1), '/apps/add');
});

test('a search in the address is typed in the field, searched, and filters the setups', async (context) => {
  const app = await openAdd(context, { query: 'q=Nginx' });
  assert.equal(app.view.main.querySelector('#add-q').value, 'Nginx');
  assert.ok(app.calls.includes('/api/1/apps/catalog/search?words=nginx'));
  assert.deepEqual(app.view.main.querySelectorAll('ul.setups a b').map((one) => one.textContent), ['Static website', 'PHP website with a database']);
});

test('the setups follow what is typed at once, by their title and by what they use, and none is no card', async (context) => {
  const app = await openAdd(context);
  await app.type('postgresql');
  assert.deepEqual(app.view.main.querySelectorAll('ul.setups a b').map((one) => one.textContent), ['PHP website with a database', 'A database']);
  await app.type('wireguard');
  assert.deepEqual(app.view.main.querySelectorAll('ul.setups a b').map((one) => one.textContent), ['Private access for your devices']);
  await app.type('zzz');
  assert.equal(app.view.main.querySelector('ul.setups'), null);
  assert.equal(app.view.main.querySelector('#add-setups-h'), null);
});

test('a search that the server would refuse is not sent, and the field says why', async (context) => {
  const app = await openAdd(context);
  const before = app.calls.length;
  await app.type('a b c d e f');
  await app.tick(500);
  assert.equal(app.calls.length, before);
  assert.equal(app.view.main.querySelector('#add-q-note').textContent, 'Search for at most 5 words at a time.');
  assert.equal(app.view.main.querySelector('#add-q').getAttribute('aria-invalid'), 'true');
  await app.type('../x');
  assert.equal(app.view.main.querySelector('#add-q-note').textContent, 'Words of a search are letters, digits, dots, @ and dashes.');
  await app.type('nginx');
  assert.equal(app.view.main.querySelector('#add-q-note').textContent, '');
  assert.equal(app.view.main.querySelector('#add-q').getAttribute('aria-invalid'), null);
});

test('a search that finds nothing says so, with the words, and Clear the search lists everything again', async (context) => {
  const app = await openAdd(context, { world: { reads: { [`${PATHS.search}?words=zzz`]: { format: 1, bundles: [], unread: [] } } } });
  await app.type('zzz');
  await app.tick(300);
  assert.equal(app.view.main.querySelector('.add-empty').textContent, 'No app in the catalogs matches “zzz”.Clear the search');
  assert.equal(app.view.main.querySelector('table'), null);
  await app.press(app.view.main.querySelector('.add-empty button'));
  assert.equal(app.view.main.querySelector('#add-q').value, '');
  assert.equal(app.view.main.querySelectorAll('tbody tr').length, 0, 'the search is sent when the typing stops');
  await app.tick(300);
  assert.equal(app.view.main.querySelectorAll('tbody tr').length, 5);
  assert.equal(app.kept.at(-1), '/apps/add');
});

test('the line a screen reader hears says how many apps the search found', async (context) => {
  const app = await openAdd(context);
  assert.equal(app.view.main.querySelector('p.sr-only').textContent, '5 apps in the catalogs');
});

test('no catalog turned on is said with the command that lists them, and the setups stay', async (context) => {
  const app = await openAdd(context, { world: { reads: { [PATHS.search]: { format: 1, bundles: [], unread: [] } } } });
  const empty = app.view.main.querySelector('.add-empty');
  assert.ok(empty.textContent.startsWith('No catalog is turned on.'));
  assert.equal(empty.querySelector('pre').textContent, 'bundle catalog');
  assert.equal(app.view.main.querySelectorAll('ul.setups a').length, 4);
});

test('a catalog that cannot be read has a banner with its reason, and the others are listed', async (context) => {
  const unread = [{ catalog: 'lab', why: 'the index is damaged' }];
  const app = await openAdd(context, { world: { reads: { [PATHS.search]: search({ unread }) } } });
  const banner = app.view.main.querySelector('.banner.warn');
  assert.equal(words(banner), 'The catalog lab could not be read: the index is damaged. The list shows the others.');
  assert.equal(banner.querySelector('a'), null);
  assert.equal(app.view.main.querySelectorAll('tbody tr').length, 5);
});

test('a catalog that cannot be read because of the network links to the Updates page', async (context) => {
  const unread = [{ catalog: 'dukovo', why: 'the network is unreachable' }];
  const app = await openAdd(context, { world: { reads: { [PATHS.search]: search({ unread }) } } });
  assert.equal(app.view.main.querySelector('.banner.warn a').getAttribute('href'), '/updates');
});

test('when no catalog can be read the banners say so and the table says that nothing is listed', async (context) => {
  const answer = { format: 1, bundles: [], unread: [{ catalog: 'dukovo', why: 'the server did not answer' }] };
  const app = await openAdd(context, { world: { reads: { [PATHS.search]: answer } } });
  assert.equal(words(app.view.main.querySelector('.banner.warn')), 'The catalog dukovo could not be read: the server did not answer.');
  assert.equal(app.view.main.querySelector('.add-empty').textContent, 'No app is listed: no catalog could be read.');
  assert.equal(app.view.main.querySelectorAll('ul.setups a').length, 4);
});

test('a search that fails is a problem with a way to try again, and the page does not say that nothing matches', async (context) => {
  const app = await openAdd(context, { world: { reads: { [PATHS.search]: refusal('APP-103', 'No catalog could be read', 502) } } });
  assert.equal(app.problems.length, 1);
  assert.equal(app.problems[0][0].id, 'APP-103');
  assert.equal(app.problems[0][1].buttons[0].textContent, 'Try again');
  assert.equal(app.view.main.querySelector('.add-empty').textContent, 'The catalogs could not be listed.');
  app.answers[PATHS.search] = search();
  await app.press(app.problems[0][1].buttons[0]);
  assert.equal(app.view.main.querySelectorAll('tbody tr').length, 5);
});

test('installed apps that cannot be read leave the table without the marks and the setups without what they install', async (context) => {
  const app = await openAdd(context, { world: { reads: { [PATHS.apps]: refusal('ADM-103', 'busy', 503) } } });
  assert.equal(app.problems.length, 0);
  assert.equal(app.view.headed.buttons[0].querySelector('a').textContent, 'Installed');
  assert.ok(cellsOf(app.view.main.querySelector('table')).every((row) => row[4] === 'Not installed' || row[4].startsWith('Installed')));
  assert.ok(app.view.main.querySelectorAll('ul.setups a .warn').length === 0);
});

test('a change that says something moved reads the installed apps again and not the catalogs', async (context) => {
  const app = await openAdd(context);
  const searched = app.calls.filter((path) => path.startsWith(PATHS.search)).length;
  const listed = app.calls.filter((path) => path === PATHS.apps).length;
  app.answers[PATHS.apps] = installedList(['org.postgresql.postgresql', 'PostgreSQL', '17.2'], ['org.nginx.nginx', 'nginx', '1.30.6']);
  await app.emit('change', {});
  assert.equal(app.calls.filter((path) => path === PATHS.apps).length, listed + 1);
  assert.equal(app.calls.filter((path) => path.startsWith(PATHS.search)).length, searched);
  assert.equal(app.view.headed.buttons[0].querySelector('a').textContent, 'Installed (2)');
  assert.equal(cellsOf(app.view.main.querySelector('table'))[0][4], 'InstalledVersion 1.30.6');
  assert.equal(app.view.main.querySelector('ul.setups .warn')?.textContent, 'Installs PHP-FPM first');
});

test('leaving the page stops a search that waited for the typing to end', async (context) => {
  const app = await openAdd(context);
  const before = app.calls.length;
  await app.type('abc');
  app.leave();
  await app.tick(1000);
  assert.equal(app.calls.length, before);
});

// ---- the sheet ----

test('a sheet is drawn beside the list with its row marked, and reads the sheet and the backups, once', async (context) => {
  const app = await openAdd(context, { rest: NGINX });
  assert.ok(app.view.main.querySelector('.add').classes.has('has-sheet'));
  assert.ok(app.view.main.querySelector('table'), 'the list is beside it');
  assert.equal(app.view.main.querySelector('tbody tr.sel').querySelector('a').getAttribute('aria-current'), 'page');
  assert.equal(app.calls.filter((path) => path === SHEET).length, 1);
  assert.ok(app.calls.includes(PATHS.backups));
  assert.equal(app.view.main.querySelector('.add-panel h2').textContent, 'nginx 1.30.6');
});

test('the title line says the catalog, that it is signed, and the licence, and the summary follows', async (context) => {
  const app = await openAdd(context, { rest: NGINX });
  const panel = app.view.main.querySelector('.add-panel');
  assert.equal(words(panel.querySelector('.add-title')), 'Nnginx 1.30.6Dukovo catalog · signed · BSD-2-Clause');
  assert.ok(words(panel).includes('Web server and reverse proxy'));
});

test('a key this computer does not know yet is said so, in the warning colour', async (context) => {
  const app = await openAdd(context, { rest: NGINX, world: { reads: { [SHEET]: nginx({ trust: { kind: 'unknown' } }) } } });
  const line = app.view.main.querySelector('.add-title span.warn');
  assert.equal(line.textContent, 'Dukovo catalog · signed by a key this computer does not know yet · BSD-2-Clause');
});

test('the sheet\'s sections are headings, in the order of the plain sheet, and a section with nothing to say is left out', async (context) => {
  const app = await openAdd(context, { rest: NGINX });
  assert.deepEqual(app.view.main.querySelectorAll('.add-panel h3').map((one) => one.textContent), [
    'Who made it', 'Its size', 'Flaws', 'Runs as services', 'May use', 'Commands that run with your access', 'Its data',
  ]);
});

test('who made it, its size, its flaws, its services and its data are said in the words of the Apps pages', async (context) => {
  const app = await openAdd(context, { rest: NGINX });
  const sections = new Map(app.view.main.querySelectorAll('.add-sec').map((one) => [one.querySelector('h3').textContent, words(one).slice(one.querySelector('h3').textContent.length).trim()]));
  assert.equal(sections.get('Who made it'), 'The nginx projectSupport');
  assert.equal(sections.get('Its size'), '3.1 MB to download, 9 MB installed');
  assert.equal(sections.get('Flaws'), 'None known.');
  assert.equal(sections.get('Runs as services'), 'nginx · as its own account, nginx · off until you turn it onA bundle that runs services is installed for everyone.');
  assert.equal(sections.get('Its data'), '/shared/services/org.nginx.nginx/nginxMade at its first start; only its account may read it.Snapshots and backups copy /shared.');
  assert.equal(sections.get('Commands that run with your access'), 'Its commands run in its sandbox.');
  assert.equal(app.view.main.querySelector('.add-sec a').getAttribute('href'), 'https://nginx.org/support');
});

test('what it may use is each permission, a warning one with the word Warning and why, and nothing else after the last', async (context) => {
  const app = await openAdd(context, { rest: NGINX });
  const may = app.view.main.querySelectorAll('.add-sec').find((one) => one.querySelector('h3').textContent === 'May use');
  assert.deepEqual(may.querySelectorAll('li').map((one) => words(one)), [
    'listen for connections · ports 80, 443',
    'Warning: read the folders of /shared · folders /shared/www (read)In its words: “It serves your websites from /shared/www.”',
    'and nothing else',
  ]);
  assert.equal(may.querySelector('b.warn').textContent, 'Warning: ');
});

test('an app with no backup place that copies /shared says so, with a link to add one', async (context) => {
  const app = await openAdd(context, { rest: NGINX, world: { reads: { [PATHS.backups]: { places: [] } } } });
  const data = app.view.main.querySelectorAll('.add-sec').find((one) => one.querySelector('h3').textContent === 'Its data');
  assert.ok(words(data).includes('No backup place copies /shared yet. Add a backup place'));
  assert.equal(data.querySelector('a').getAttribute('href'), '/snapshots/backups');
});

test('backups that cannot be read say nothing of them, and the sheet is still drawn', async (context) => {
  const app = await openAdd(context, { rest: NGINX, world: { reads: { [PATHS.backups]: refusal('ADM-103', 'busy', 503) } } });
  const data = app.view.main.querySelectorAll('.add-sec').find((one) => one.querySelector('h3').textContent === 'Its data');
  assert.ok(!words(data).includes('backup'));
  assert.ok(control(app.view.main, 'install'));
});

test('an app whose commands run with the person\'s access names them, and what it uses is matched against the installed apps', async (context) => {
  const app = await openAdd(context, {
    rest: 'org%2Eexample%2Enotes',
    world: { reads: { '/api/1/apps/catalog/org.example.notes/sheet': notes(), '/api/1/apps/org.postgresql.postgresql': plain('org.postgresql.postgresql', 'PostgreSQL', '17.2') } },
  });
  const panel = app.view.main.querySelector('.add-panel');
  const commands = panel.querySelectorAll('.add-sec').find((one) => one.querySelector('h3').textContent === 'Commands that run with your access');
  assert.ok(words(commands).includes('notes-sync'));
  assert.ok(words(commands).includes('These run with the access of whoever runs them'));
  assert.ok(words(panel).includes('Needs PHP-FPM, which is not installed.'));
  assert.ok(app.calls.includes('/api/1/apps/org.postgresql.postgresql'), 'what is installed is read to see who offers what the sheet uses');
});

test('a sheet that uses nothing reads no installed app', async (context) => {
  const app = await openAdd(context, { rest: NGINX });
  assert.ok(!app.calls.includes('/api/1/apps/org.postgresql.postgresql'));
});

test('the buttons of a sheet are Install for everyone, Show command and Cancel, which goes back to the list with the search kept', async (context) => {
  const app = await openAdd(context, { rest: NGINX, query: 'q=ng' });
  const buttons = app.view.main.querySelector('.add-buttons');
  assert.deepEqual(buttons.querySelectorAll('.btn').map((one) => one.textContent), ['Install for everyone', 'Show command', 'Cancel']);
  assert.equal(control(buttons, 'close').getAttribute('href'), '/apps/add?q=ng');
  assert.equal(app.kept.length, 0, 'nothing was typed');
});

test('Show command shows the command that installs it, for anyone to type', async (context) => {
  const app = await openAdd(context, { rest: NGINX });
  const box = app.view.main.querySelector('.add-command');
  assert.equal(box.hidden, true);
  const show = control(app.view.main, 'show-command');
  assert.equal(show.getAttribute('aria-expanded'), 'false');
  await app.press(show);
  assert.equal(box.hidden, false);
  assert.equal(box.querySelector('pre').textContent, 'elevate bundle install org.nginx.nginx --for-everyone');
  assert.equal(show.getAttribute('aria-expanded'), 'true');
  await app.press(show);
  assert.equal(box.hidden, true);
});

test('a reader reads the sheet and is told who installs, with no Install button', async (context) => {
  const app = await openAdd(context, { rest: NGINX, session: { admin: false } });
  assert.equal(control(app.view.main, 'install'), null);
  assert.ok(words(app.view.main.querySelector('.add-panel')).includes('An administrator installs apps for everyone.'));
  assert.ok(control(app.view.main, 'show-command'));
});

test('an app that would be refused shows why in the error colour and has no Install button', async (context) => {
  const why = 'It needs another kind of machine.';
  const app = await openAdd(context, { rest: NGINX, world: { reads: { [SHEET]: nginx({ refusal: why }) } } });
  const banner = app.view.main.querySelector('.banner.bad');
  assert.equal(words(banner), `This app would not be installed. ${why}`);
  assert.equal(control(app.view.main, 'install'), null);
  assert.ok(!words(app.view.main.querySelector('.add-panel')).includes('An administrator installs apps for everyone.'));
});

test('an app that is installed says its version and links to its page, and has no Install button', async (context) => {
  const app = await openAdd(context, { rest: NGINX, world: { reads: { [SHEET]: nginx({ installed: '1.30.5' }) } } });
  const banner = app.view.main.querySelector('.banner.info');
  assert.equal(words(banner), 'Version 1.30.5 is installed. Updating is on its page. Open its page');
  assert.equal(banner.querySelector('a').getAttribute('href'), '/apps/org%2Enginx%2Enginx');
  assert.equal(control(app.view.main, 'install'), null);
});

test('a sheet that could not be fetched says why and that nothing was kept, with Try again and the error\'s page', async (context) => {
  const error = refusal('APP-101', 'No catalog lists that app', 404, { detail: 'org.example.gone is in no catalog.' });
  const app = await openAdd(context, { rest: 'org%2Eexample%2Egone', world: { reads: { '/api/1/apps/catalog/org.example.gone/sheet': error } } });
  const panel = app.view.main.querySelector('.add-panel');
  assert.equal(words(panel.querySelector('.banner.bad')), 'The app could not be fetched: org.example.gone is in no catalog.Nothing was installed or kept.What APP-101 means');
  assert.equal(panel.querySelector('.banner a').getAttribute('href'), '/help/errors/APP-101');
  assert.equal(app.problems.length, 0, 'the panel says it, not the head');
  app.answers['/api/1/apps/catalog/org.example.gone/sheet'] = notes({ id: 'org.example.gone', name: 'Gone' });
  await app.press(control(panel, 'again'));
  assert.equal(app.view.main.querySelector('.add-panel h2').textContent, 'Gone 2.0.1');
});

test('an address that cannot be an app\'s ID says so and goes back to the list', async (context) => {
  const app = await openAdd(context, { rest: '..%2Fx' });
  assert.equal(app.view.main.querySelector('.add-panel').textContent, 'No app can have the ID ..%2Fx.Back to the catalogs');
  assert.ok(!app.calls.some((path) => path.includes('/sheet')));
});

// ---- the install ----

test('Install for everyone runs the one step, a task, pinned by the sheet\'s hash, with its list drawn under the sheet', async (context) => {
  const app = await openAdd(context, {
    rest: NGINX,
    world: {
      install: async (step, answers) => {
        answers['/api/1/apps/org.nginx.nginx'] = plain('org.nginx.nginx', 'nginx', '1.30.6');
      },
    },
  });
  await app.press(control(app.view.main, 'install'));
  assert.equal(app.steps.length, 1);
  const [step] = app.steps[0].steps;
  assert.equal(step.action, 'apps.install');
  assert.deepEqual(step.args, { id: 'org.nginx.nginx', sha256: SHA });
  assert.equal(step.wait, 'task');
  assert.equal(step.title, 'Install nginx 1.30.6 for everyone');
  assert.deepEqual(step.words, {
    unlock: 'install nginx for everyone', running: 'Installing nginx', done: 'nginx is installed.', failed: 'nginx was not installed',
  });
  assert.equal(app.steps[0].view, app.view, 'the step runs on the page\'s own view');
});

test('the list of steps that the runner draws stands in the panel while the install runs', async (context) => {
  let release = null;
  let inside = null;
  const app = await openAdd(context, {
    rest: NGINX,
    world: {
      install: (step, answers) => new Promise((resolve) => {
        release = () => {
          answers[`/api/1/apps/${step.args.id}`] = plain(step.args.id, 'nginx', '1.30.6');
          resolve(undefined);
        };
      }),
    },
  });
  app.press(control(app.view.main, 'install'));
  await app.press(app.view.main);
  const run = app.view.main.querySelector('.add-panel .add-run');
  inside = run.querySelector('ol') !== null;
  assert.ok(inside, 'the runner\'s list is in the panel under the sheet');
  release();
  await app.press(app.view.main);
  assert.ok(app.view.main.querySelector('.add-done'));
});

test('what comes after: what was installed, its service that is off with Turn it on, where its data is, and the next step', async (context) => {
  const app = await openAdd(context, { rest: NGINX });
  await app.press(control(app.view.main, 'install'));
  const done = app.view.main.querySelector('.add-done');
  assert.deepEqual(done.querySelectorAll('li').map((one) => words(one)), [
    'Installed nginx 1.30.6 for everyone.',
    'Its service nginx is off. Turn it on Open its service',
    'Its data is in /shared/services/org.nginx.nginx/nginx once it has started.',
  ]);
  assert.equal(done.querySelector('a').getAttribute('href'), '/services/nginx');
  const next = control(app.view.main, 'next');
  assert.equal(next.textContent, 'Add a website');
  assert.equal(next.getAttribute('href'), '/websites/add');
  assert.equal(control(app.view.main, 'close').textContent, 'Back to the catalogs');
  assert.equal(app.view.main.querySelector('.add-panel h2').textContent, 'nginx 1.30.6');
  assert.equal(control(app.view.main, 'install'), null);
});

test('after the install the installed apps and the catalogs are read again, so that the table and the head say so', async (context) => {
  const app = await openAdd(context, { rest: NGINX });
  const listed = app.calls.filter((path) => path === PATHS.apps).length;
  const searched = app.calls.filter((path) => path.startsWith(PATHS.search)).length;
  app.answers[PATHS.apps] = installedList(['org.postgresql.postgresql', 'PostgreSQL', '17.2'], ['org.nginx.nginx', 'nginx', '1.30.6']);
  await app.press(control(app.view.main, 'install'));
  assert.equal(app.calls.filter((path) => path === PATHS.apps).length, listed + 1);
  assert.equal(app.calls.filter((path) => path.startsWith(PATHS.search)).length, searched + 1);
  assert.equal(app.view.headed.buttons[0].querySelector('a').textContent, 'Installed (2)');
  assert.equal(cellsOf(app.view.main.querySelector('table'))[0][4], 'InstalledVersion 1.30.6');
});

test('Turn it on runs service.enable for the service, with the words of the Services page, and the line says that it is running', async (context) => {
  const app = await openAdd(context, {
    rest: NGINX,
    world: {
      answers: {
        'service.enable': (args) => {
          app.answers[PATHS.services] = { now_ms: 1, services: [{ name: args.name, state: 'running', enabled: true }] };
          return { ok: true, result: { done: true, changed: true } };
        },
      },
    },
  });
  await app.press(control(app.view.main, 'install'));
  await app.press(control(app.view.main, 'turn-on-nginx'));
  assert.deepEqual(app.ran('service.enable'), [{ name: 'nginx' }]);
  assert.equal(app.ranWith('service.enable')[0][2].words.unlock, 'start nginx now and at every boot');
  assert.equal(words(app.view.main.querySelectorAll('.add-done li')[1]), 'Its service nginx is on: running, starts at boot. Open its service');
  assert.equal(control(app.view.main, 'turn-on-nginx'), null);
});

test('PostgreSQL leads to Add a database and PHP to Add a PHP website, and any other app to its own page', async (context) => {
  const postgres = await openAdd(context, {
    rest: 'org%2Epostgresql%2Epostgresql',
    world: { reads: { [PATHS.apps]: installedList(), '/api/1/apps/catalog/org.postgresql.postgresql/sheet': nginx({ id: 'org.postgresql.postgresql', name: 'PostgreSQL', version: '17.3' }) } },
  });
  await postgres.press(control(postgres.view.main, 'install'));
  assert.equal(control(postgres.view.main, 'next').getAttribute('href'), '/databases/add');
  const web = await openAdd(context, { rest: 'net%2Ephp%2Ephp' });
  await web.press(control(web.view.main, 'install'));
  assert.equal(control(web.view.main, 'next').getAttribute('href'), '/websites/add?kind=php');
  assert.equal(control(web.view.main, 'next').textContent, 'Add a PHP website');
  const other = await openAdd(context, { rest: 'org%2Eexample%2Enotes', world: { reads: { '/api/1/apps/catalog/org.example.notes/sheet': notes() } } });
  await other.press(control(other.view.main, 'install'));
  assert.equal(control(other.view.main, 'next').textContent, 'Open its page');
  assert.equal(control(other.view.main, 'next').getAttribute('href'), '/apps/org%2Eexample%2Enotes');
  assert.equal(other.view.main.querySelector('.add-done').querySelectorAll('li').length, 1, 'no service, no data');
});

test('an install that fails leaves the sheet, says that nothing was installed, and offers Try again, which runs the step again', async (context) => {
  let tries = 0;
  const app = await openAdd(context, {
    rest: NGINX,
    world: {
      install: async (step, answers) => {
        tries += 1;
        if (tries === 1) return failedWith({ id: 4, state: 'failed', log: ['bundle: the disk is full'] });
        answers[`/api/1/apps/${step.args.id}`] = plain(step.args.id, 'nginx', '1.30.6');
        return undefined;
      },
    },
  });
  await app.press(control(app.view.main, 'install'));
  const run = app.view.main.querySelector('.add-run');
  assert.ok(words(run).includes('Nothing was installed.'));
  assert.equal(control(run, 'read-again'), null);
  assert.ok(control(app.view.main, 'install'), 'the sheet stays');
  assert.equal(app.view.main.querySelector('.add-done'), null);
  await app.press(control(run, 'try-again'));
  assert.equal(tries, 2);
  assert.ok(app.view.main.querySelector('.add-done'));
});

test('a catalog that changed between the sheet and the install is APP-102: the page says so and Read it again reads the new sheet, which the next install is pinned by', async (context) => {
  let reads = 0;
  const app = await openAdd(context, {
    rest: NGINX,
    world: {
      reads: { [SHEET]: () => { reads += 1; return nginx({ sha256: reads === 1 ? SHA : OTHER_SHA, version: reads === 1 ? '1.30.6' : '1.30.7' }); } },
      install: async (step, answers) => {
        if (step.args.sha256 === SHA) return failedWith({ id: 5, state: 'failed', log: ['downloading', 'bundle: the catalog\'s file is not the one whose sheet you read (APP-102)'] });
        answers[`/api/1/apps/${step.args.id}`] = plain(step.args.id, 'nginx', '1.30.7');
        return undefined;
      },
    },
  });
  await app.press(control(app.view.main, 'install'));
  const run = app.view.main.querySelector('.add-run');
  assert.equal(run.querySelector('p.warn').textContent, 'The catalog has a newer version than the sheet you read. Read it again.');
  await app.press(control(run, 'read-again'));
  assert.equal(reads, 2);
  assert.equal(app.view.main.querySelector('.add-panel h2').textContent, 'nginx 1.30.7');
  assert.equal(app.view.main.querySelector('.add-run').textContent, '', 'the old failure is gone');
  await app.press(control(app.view.main, 'install'));
  assert.deepEqual(app.steps.map((one) => one.steps[0].args.sha256), [SHA, OTHER_SHA]);
  assert.ok(app.view.main.querySelector('.add-done'));
  assert.equal(words(app.view.main.querySelector('.add-done li')), 'Installed nginx 1.30.7 for everyone.');
});

test('an install is known to be stale by the note of the task as it ended as well as by its log, and no other task is read', async (context) => {
  const app = await openAdd(context, {
    rest: NGINX,
    world: { install: async () => failedWith({ id: 3, state: 'failed', note: 'APP-102: the catalog\'s file changed', log: [] }) },
  });
  await app.press(control(app.view.main, 'install'));
  assert.ok(control(app.view.main.querySelector('.add-run'), 'read-again'));
  assert.ok(!app.calls.some((path) => path.startsWith('/api/1/tasks')), 'what the runner gave is enough');
});

test('an install that failed with no task, as one that did not start, says that nothing was installed and does not say that the catalog moved', async (context) => {
  const app = await openAdd(context, { rest: NGINX, world: { install: async () => failedWith(undefined) } });
  await app.press(control(app.view.main, 'install'));
  const run = app.view.main.querySelector('.add-run');
  assert.ok(words(run).includes('Nothing was installed.'));
  assert.equal(control(run, 'read-again'), null);
});

test('an install whose page was left before it ended draws nothing more and keeps the buttons usable', async (context) => {
  const app = await openAdd(context, {
    rest: NGINX,
    world: { install: async () => ({ done: false, failed: null, answers: [], left: true }) },
  });
  await app.press(control(app.view.main, 'install'));
  const run = app.view.main.querySelector('.add-run');
  assert.ok(!words(run).includes('Nothing was installed.'));
  assert.equal(control(app.view.main, 'install').getAttribute('aria-disabled'), null);
});

test('while the install runs a second press does nothing', async (context) => {
  let release = null;
  const app = await openAdd(context, {
    rest: NGINX,
    world: {
      install: (step, answers) => new Promise((resolve) => {
        release = () => {
          answers[`/api/1/apps/${step.args.id}`] = plain(step.args.id, 'nginx', '1.30.6');
          resolve(undefined);
        };
      }),
    },
  });
  const button = control(app.view.main, 'install');
  button.click();
  await app.press(button);
  assert.equal(button.getAttribute('aria-disabled'), 'true');
  assert.equal(app.steps.length, 1);
  release();
  await app.press(app.view.main.querySelector('h1') ?? app.view.main);
  assert.equal(app.steps.length, 1);
  assert.ok(app.view.main.querySelector('.add-done'));
});

test('a locked administrator is asked to unlock, in words that name the install, before anything runs', async (context) => {
  const app = await openAdd(context, { rest: NGINX, unlocked: false, session: { kinds: ['password'], host: 'duke-nas' } });
  await app.press(control(app.view.main, 'install'));
  assert.equal(app.dialog().querySelector('h2').textContent, 'Unlock to make changes');
  assert.ok(app.dialog().textContent.includes('You are about to install nginx for everyone.'));
  assert.equal(app.steps.length, 0);
});

test('the install does not run again for an app whose sheet says it is installed, even if a button were pressed', async (context) => {
  const app = await openAdd(context, { rest: NGINX, world: { reads: { [SHEET]: nginx({ installed: '1.30.6' }) } } });
  assert.equal(control(app.view.main, 'install'), null);
  assert.equal(app.steps.length, 0);
});

test('the sheet of PHP-FPM, which offers a socket, says what it provides', async (context) => {
  const app = await openAdd(context, { rest: 'net%2Ephp%2Ephp' });
  const works = app.view.main.querySelectorAll('.add-sec').find((one) => one.querySelector('h3').textContent === 'Works with');
  assert.equal(words(works), 'Works withProvides fastcgi at /run/php-fpm.sock');
  assert.equal(php().sha256, SHA);
});
