// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the pages that add an app say: the words of a search, the rows and
// banners of the catalogs, the setups (lib/catalog-model.js), the sheet of an
// app that is not yet installed for each kind of trust, a refusal, a service,
// a warning and a folder, the stale sheet, and what comes after the install
// (lib/sheet-model.js).

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  addHref, emptyOf, MAX_WORDS, rowsOf, searchPath, setupsOf, sheetHref, sheetPath, unreadOf, wordsOf,
} from '../lib/catalog-model.js';
import { NGINX_ID, PHP_ID, POSTGRESQL_ID } from '../lib/known-bundles.js';
import {
  afterOf, DATA_WORDS, EVERYONE_WORDS, fetchWords, installCommand, isStale, nextStep, providersOf, READER_WORDS, SANDBOX_WORDS,
  sheetOf, UNKNOWN_KEY, YOUR_ACCESS_WORDS,
} from '../lib/sheet-model.js';
import {
  backups, installedList, nginx, notes, OTHER_SHA, php, provider, search, SHA,
} from './catalog-fixtures.mjs';
import { services, valkey } from './apps-fixtures.mjs';

// ---- the search ----

test('what is typed is lowercased and split at spaces, and anything the search would refuse is said before it is sent', () => {
  assert.deepEqual(wordsOf('  Web  SERVER '), { words: ['web', 'server'], problem: null });
  assert.deepEqual(wordsOf(''), { words: [], problem: null });
  assert.deepEqual(wordsOf(undefined), { words: [], problem: null });
  assert.deepEqual(wordsOf('org.example@notes my-app'), { words: ['org.example@notes', 'my-app'], problem: null });
  assert.equal(wordsOf('a b c d e').problem, null);
  assert.equal(MAX_WORDS, 5);
  assert.deepEqual(wordsOf('a b c d e f'), { words: [], problem: 'Search for at most 5 words at a time.' });
  assert.deepEqual(wordsOf('../x'), { words: [], problem: 'Words of a search are letters, digits, dots, @ and dashes.' });
  assert.equal(wordsOf('café').problem, 'Words of a search are letters, digits, dots, @ and dashes.');
  assert.equal(wordsOf('x'.repeat(65)).problem, 'Words of a search are letters, digits, dots, @ and dashes.');
  assert.equal(wordsOf('x'.repeat(64)).problem, null);
});

test('the bundles that the console knows by name have the IDs of the catalog', () => {
  assert.deepEqual([NGINX_ID, PHP_ID, POSTGRESQL_ID], ['org.nginx.nginx', 'net.php.php', 'org.postgresql.postgresql']);
});

test('the addresses: the search with its words, the sheet of an app, and the pages with what was typed kept', () => {
  assert.equal(searchPath([]), '/api/1/apps/catalog/search');
  assert.equal(searchPath(['web', 'server']), '/api/1/apps/catalog/search?words=web%20server');
  assert.equal(sheetPath('org.nginx.nginx'), '/api/1/apps/catalog/org.nginx.nginx/sheet');
  assert.equal(addHref(), '/apps/add');
  assert.equal(addHref('web server'), '/apps/add?q=web%20server');
  assert.equal(sheetHref('org.nginx.nginx'), '/apps/add/org%2Enginx%2Enginx');
  assert.equal(sheetHref('org.nginx.nginx', 'ng'), '/apps/add/org%2Enginx%2Enginx?q=ng');
  assert.equal(installCommand('org.nginx.nginx'), 'elevate bundle install org.nginx.nginx --for-everyone');
});

// ---- the rows of the catalogs ----

test('the rows are sorted by name as people read it, with the initial, the colour, the version, the catalog and the address of the sheet', () => {
  const rows = rowsOf(search(), [], 'x');
  assert.deepEqual(rows.map((row) => row.name), ['nginx', 'Notes', 'PHP', 'PostgreSQL', 'python']);
  const first = rows[0];
  assert.equal(first.id, 'org.nginx.nginx');
  assert.equal(first.version, '1.30.6');
  assert.equal(first.catalog, 'dukovo');
  assert.equal(first.initial, 'N');
  assert.ok([0, 1, 2, 3, 4, 5].includes(first.tint));
  assert.equal(first.href, '/apps/add/org%2Enginx%2Enginx?q=x');
  assert.equal(rowsOf(search()).find((row) => row.id === 'net.php.php').href, '/apps/add/net%2Ephp%2Ephp');
});

test('two apps of one name are told apart by their IDs, and a row that has no ID is left out', () => {
  const answer = {
    bundles: [{ id: 'b.x.x', name: 'Same' }, { id: 'a.x.x', name: 'same' }, { name: 'No ID' }, null, { id: '', name: 'Empty' }],
  };
  assert.deepEqual(rowsOf(answer).map((row) => row.id), ['a.x.x', 'b.x.x']);
});

test('a row with no name has its ID for one, and a bundle that says nothing of its version or catalog has them empty', () => {
  const [row] = rowsOf({ bundles: [{ id: 'org.example.bare' }] });
  assert.equal(row.name, 'org.example.bare');
  assert.equal(row.version, '');
  assert.equal(row.catalog, '');
  assert.equal(row.initial, 'O');
  assert.deepEqual(row.flaws, { words: 'Flaws not listed', urgent: false });
  assert.deepEqual(rowsOf(null), []);
  assert.deepEqual(rowsOf({}), []);
});

test('the flaws of a row are said as the list of installed apps says them, in the warning colour for high and critical only', () => {
  const by = (id) => rowsOf(search()).find((row) => row.id === id).flaws;
  assert.deepEqual(by('org.postgresql.postgresql'), { words: '2 known flaws, worst high', urgent: true });
  assert.deepEqual(by('net.php.php'), { words: '1 known flaw, worst medium', urgent: false });
  assert.deepEqual(by('org.nginx.nginx'), { words: 'No known flaws', urgent: false });
  assert.deepEqual(by('org.example.notes'), { words: 'Flaws not listed', urgent: false });
  const critical = rowsOf({ bundles: [{ id: 'a.b.c', flaws: { state: 'known', count: 3, worst: 'critical' } }] })[0].flaws;
  assert.deepEqual(critical, { words: '3 known flaws, worst critical', urgent: true });
  assert.deepEqual(rowsOf({ bundles: [{ id: 'a.b.c', flaws: { state: 'known', count: 0, worst: null } }] })[0].flaws, { words: 'No known flaws', urgent: false });
});

test('an app is installed by the version the search says, else by the version of the list of installed apps, else it is not', () => {
  const answer = search();
  const list = installedList(['org.nginx.nginx', 'nginx', '1.30.5'], ['org.postgresql.postgresql', 'PostgreSQL', '17.0']).bundles;
  const by = (id) => rowsOf(answer, list).find((row) => row.id === id).installed;
  assert.equal(by('org.postgresql.postgresql'), '17.2', 'the search\'s own member comes first');
  assert.equal(by('org.nginx.nginx'), '1.30.5');
  assert.equal(by('net.php.php'), null);
  assert.equal(rowsOf(answer).find((row) => row.id === 'org.nginx.nginx').installed, null);
});

test('a catalog that could not be read has a banner with its reason, and says that the others are shown only when something is', () => {
  const answer = search({ unread: [{ catalog: 'dukovo', why: 'the server did not answer.' }, { catalog: 'lab', why: 'the index is damaged' }] });
  assert.deepEqual(unreadOf(answer).map((one) => one.words), [
    'The catalog dukovo could not be read: the server did not answer. The list shows the others.',
    'The catalog lab could not be read: the index is damaged. The list shows the others.',
  ]);
  const none = unreadOf({ bundles: [], unread: [{ catalog: 'dukovo', why: 'no route to host' }] });
  assert.equal(none[0].words, 'The catalog dukovo could not be read: no route to host.');
  assert.deepEqual(unreadOf(search()), []);
  assert.deepEqual(unreadOf(null), []);
  assert.equal(unreadOf({ unread: [{ catalog: 'x' }] })[0].words, 'The catalog x could not be read: no reason was given.');
  assert.deepEqual(unreadOf({ unread: [{ why: 'a' }, null] }), [], 'a catalog with no name has no banner');
});

test('the Updates link is offered when the reason is the network and not when the catalog itself is at fault', () => {
  const reasons = {
    'the network is unreachable': true,
    'could not resolve catalog.example.org': true,
    'connection refused': true,
    'timed out after 30 seconds': true,
    'the index is damaged': false,
    'the signature does not match': false,
  };
  for (const [why, network] of Object.entries(reasons)) {
    assert.equal(unreadOf({ unread: [{ catalog: 'c', why }] })[0].network, network, why);
  }
});

test('an empty table says why: no catalog is on, nothing matches, or no catalog could be read', () => {
  assert.equal(emptyOf(search(), []), null);
  assert.deepEqual(emptyOf({ bundles: [], unread: [] }, []), { kind: 'none', words: 'No catalog is turned on.', command: 'bundle catalog' });
  assert.deepEqual(emptyOf({ bundles: [], unread: [] }, ['zzz', 'x']), { kind: 'nomatch', words: 'No app in the catalogs matches “zzz x”.' });
  assert.deepEqual(emptyOf({ bundles: [], unread: [{ catalog: 'a', why: 'b' }] }, ['zzz']), { kind: 'unread', words: 'No app is listed: no catalog could be read.' });
  assert.equal(emptyOf(null, []).kind, 'none');
});

// ---- the setups ----

test('the setups are the four the machine does, each with what it uses and where it goes', () => {
  const setups = setupsOf('', []);
  assert.deepEqual(setups.map((one) => one.title), ['Static website', 'PHP website with a database', 'A database', 'Private access for your devices']);
  assert.deepEqual(setups.map((one) => one.href), ['/websites/add?kind=static', '/websites/add?kind=php&database=1', '/databases/add', '/security#private']);
  assert.deepEqual(setups.map((one) => one.uses), [['nginx'], ['nginx', 'PHP-FPM', 'PostgreSQL'], ['PostgreSQL'], ['WireGuard']]);
  assert.ok(setups.every((one) => one.words.endsWith('.')));
});

test('a setup whose apps are not installed says which it installs first, and WireGuard, which is part of the system, never is', () => {
  const none = setupsOf('', []);
  assert.deepEqual(none.map((one) => one.says), ['Installs nginx first', 'Installs nginx, PHP-FPM and PostgreSQL first', 'Installs PostgreSQL first', null]);
  const some = setupsOf('', ['org.nginx.nginx', 'org.postgresql.postgresql']);
  assert.deepEqual(some.map((one) => one.says), [null, 'Installs PHP-FPM first', null, null]);
  assert.deepEqual(some[1].installs, ['PHP-FPM']);
  const two = setupsOf('php', ['org.nginx.nginx']);
  assert.equal(two[0].says, 'Installs PHP-FPM and PostgreSQL first');
  const all = setupsOf('', ['org.nginx.nginx', 'net.php.php', 'org.postgresql.postgresql']);
  assert.ok(all.every((one) => one.says === null && one.installs.length === 0));
});

test('the search filters the setups by their title and by what they use, every word of it', () => {
  assert.deepEqual(setupsOf('nginx', []).map((one) => one.id), ['static', 'php']);
  assert.deepEqual(setupsOf('NGINX php', []).map((one) => one.id), ['php']);
  assert.deepEqual(setupsOf('database', []).map((one) => one.id), ['php', 'database']);
  assert.deepEqual(setupsOf('wireguard', []).map((one) => one.id), ['private']);
  assert.deepEqual(setupsOf('postgresql', []).map((one) => one.id), ['php', 'database']);
  assert.deepEqual(setupsOf('zzz', []), []);
  assert.deepEqual(setupsOf(undefined, []).length, 4);
  assert.ok(setupsOf('', null).every((one) => one.says === null && one.installs.length === 0), 'apps that were not read say nothing of what is missing');
});

// ---- the sheet ----

test('the title line says the catalog, that the app is signed, and its licence as the sheet gives it', () => {
  const sheet = sheetOf(nginx());
  assert.deepEqual(sheet.line, { words: 'Dukovo catalog · signed · BSD-2-Clause', warn: false });
  assert.equal(sheet.name, 'nginx');
  assert.equal(sheet.version, '1.30.6');
  assert.equal(sheet.summary, 'Web server and reverse proxy');
  assert.equal(sheet.initial, 'N');
  assert.equal(sheetOf(nginx({ license: 'MIT OR Apache-2.0' })).line.words, 'Dukovo catalog · signed · MIT OR Apache-2.0');
  assert.equal(sheetOf(nginx({ license: undefined })).line.words, 'Dukovo catalog · signed');
  assert.equal(sheetOf(nginx({ catalog: undefined })).line.words, 'signed · BSD-2-Clause');
});

test('each kind of trust has its words, and only a key this computer does not know yet is in the warning colour', () => {
  const by = (kind) => sheetOf(nginx({ trust: { kind } })).line;
  assert.deepEqual(by('catalog'), { words: 'Dukovo catalog · signed · BSD-2-Clause', warn: false });
  assert.deepEqual(by('developer'), { words: 'Dukovo catalog · signed by its developer\'s key, trusted by hand · BSD-2-Clause', warn: false });
  assert.deepEqual(by('bound'), { words: 'Dukovo catalog · signed · BSD-2-Clause', warn: false });
  assert.deepEqual(by('bound_for_everyone'), { words: 'Dukovo catalog · signed · BSD-2-Clause', warn: false });
  assert.deepEqual(by('unknown'), { words: `Dukovo catalog · ${UNKNOWN_KEY} · BSD-2-Clause`, warn: true });
  assert.equal(UNKNOWN_KEY, 'signed by a key this computer does not know yet');
  assert.deepEqual(sheetOf(nginx({ trust: undefined })).line.warn, false);
});

test('who made it: the publisher, and the support as a link only when it is one', () => {
  const sheet = sheetOf(nginx());
  assert.equal(sheet.publisher, 'The nginx project');
  assert.deepEqual(sheet.support, { words: 'https://nginx.org/support', link: true });
  assert.deepEqual(sheetOf(notes()).support, { words: 'support@example.org', link: false });
  assert.deepEqual(sheetOf(nginx({ support: 'javascript:alert(1)' })).support, { words: 'javascript:alert(1)', link: false });
  assert.equal(sheetOf(nginx({ support: null })).support, null);
  assert.equal(sheetOf(nginx({ publisher: '' })).publisher, null);
});

test('its size says what is downloaded and what is installed, and what it knows of only one', () => {
  assert.equal(sheetOf(nginx()).size, '3.1 MB to download, 9 MB installed');
  assert.equal(sheetOf(nginx({ download_bytes: undefined })).size, '9 MB installed');
  assert.equal(sheetOf(nginx({ bytes: undefined })).size, '3.1 MB to download');
  assert.equal(sheetOf(nginx({ bytes: null, download_bytes: null })).size, null);
});

test('its flaws are the lines of the Apps page, with where each is fixed, or that none is known, or that the catalog says nothing', () => {
  const none = sheetOf(nginx()).flaws;
  assert.deepEqual(none, { worst: null, urgent: false, lines: [], none: 'None known.' });
  const listed = sheetOf(nginx({
    flaws: {
      state: 'listed',
      known: [
        { id: 'DKSA-2026-0007', severity: 'high', component: 'nginx', fixed: '1.30.7', summary: 'a crafted request stops the server' },
        { id: 'DKSA-2026-0003', severity: 'low', component: 'nginx', fixed: null, summary: 'a slow request holds a worker' },
      ],
    },
  })).flaws;
  assert.equal(listed.worst, 'high');
  assert.equal(listed.urgent, true);
  assert.equal(listed.none, null);
  assert.deepEqual(listed.lines, [
    { words: 'DKSA-2026-0007 · high · nginx: a crafted request stops the server', fixed: 'Fixed in 1.30.7.' },
    { words: 'DKSA-2026-0003 · low · nginx: a slow request holds a worker', fixed: 'No fix is known yet.' },
  ]);
  assert.equal(sheetOf(nginx({ flaws: { state: 'unknown', known: [] } })).flaws.none, 'The catalog says nothing of this version\'s flaws.');
  assert.equal(sheetOf(nginx({ flaws: undefined })).flaws.none, null);
});

test('each service is said with its account and whether it starts at boot, and an app that runs services is installed for everyone', () => {
  const sheet = sheetOf(nginx());
  assert.deepEqual(sheet.services.map((one) => one.words), ['nginx · as its own account, nginx · off until you turn it on']);
  assert.equal(sheet.everyone, EVERYONE_WORDS);
  assert.equal(EVERYONE_WORDS, 'A bundle that runs services is installed for everyone.');
  const boots = sheetOf(nginx({ service_list: [{ name: 'a', account: 'b', start: 'on' }, { name: 'c' }] })).services;
  assert.deepEqual(boots.map((one) => one.words), ['a · as its own account, b · starts at boot', 'c · as its own account, c · off until you turn it on']);
  const none = sheetOf(notes());
  assert.deepEqual(none.services, []);
  assert.equal(none.everyone, null);
  const older = sheetOf(nginx({ service_list: undefined }));
  assert.deepEqual(older.services.map((one) => one.words), ['nginx: the web server']);
});

test('what it may use is each permission in the system\'s words, a warning one marked and with why, and an optional one said so', () => {
  const sheet = sheetOf(nginx());
  assert.deepEqual(sheet.may, [
    { words: 'listen for connections · ports 80, 443', warning: false, why: null },
    { words: 'read the folders of /shared · folders /shared/www (read)', warning: true, why: 'It serves your websites from /shared/www.' },
  ]);
  const more = sheetOf(nginx({ permissions: [{ name: 'net.outbound', lets: 'reach any address', tier: 'with a warning', need: 'optional', why: '' }, null, {}] })).may;
  assert.deepEqual(more, [{ words: 'reach any address (optional)', warning: true, why: null }]);
  assert.deepEqual(sheetOf(notes()).may, []);
  assert.equal(sheetOf(nginx({ permissions: [{ name: 'x', tier: 'warning' }] })).may[0].warning, true, 'the word the design uses is understood too');
  assert.equal(sheetOf(nginx({ permissions: [{ name: 'x', tier: 'on first use' }] })).may[0].warning, false);
});

test('its commands: those that run with the person\'s access are named with the words of bundle(1), and none says that they run in the sandbox', () => {
  assert.deepEqual(sheetOf(notes()).commands, { names: ['notes-sync'], words: YOUR_ACCESS_WORDS });
  assert.equal(YOUR_ACCESS_WORDS, 'These run with the access of whoever runs them; the first time each asks, with a warning, in that person\'s terminal.');
  assert.deepEqual(sheetOf(nginx()).commands, { names: [], words: SANDBOX_WORDS });
  assert.equal(SANDBOX_WORDS, 'Its commands run in its sandbox.');
});

test('its data is each service\'s folder, said with who may read it, and whether snapshots and backups copy /shared', () => {
  const covered = sheetOf(nginx(), { backups: backups('/users', '/shared') }).data;
  assert.deepEqual(covered, {
    folders: ['/shared/services/org.nginx.nginx/nginx'],
    words: DATA_WORDS,
    backup: { words: 'Snapshots and backups copy /shared.', link: null },
  });
  assert.equal(DATA_WORDS, 'Made at its first start; only its account may read it.');
  const bare = sheetOf(nginx(), { backups: backups('/users') }).data.backup;
  assert.deepEqual(bare, { words: 'No backup place copies /shared yet.', link: { words: 'Add a backup place', href: '/snapshots/backups' } });
  assert.deepEqual(sheetOf(nginx(), { backups: { places: [] } }).data.backup.link.href, '/snapshots/backups');
  assert.equal(sheetOf(nginx()).data.backup, null, 'backups that could not be read say nothing');
  assert.equal(sheetOf(notes()).data, null);
});

test('a sheet that gives no list of folders has them worked out from its services, as the Apps page does', () => {
  const sheet = sheetOf(nginx({ data: 'its data folder', service_list: [{ name: 'a', account: 'b', start: 'off' }] }));
  assert.deepEqual(sheet.data.folders, ['/shared/services/org.nginx.nginx/b']);
  assert.equal(sheetOf(nginx({ data: undefined, service_list: undefined })).data, null);
});

test('what it provides, and what it uses with the installed apps that offer it, or that it needs one that is not installed', () => {
  assert.deepEqual(sheetOf(php()).provides, ['fastcgi at /run/php-fpm.sock']);
  assert.deepEqual(sheetOf(nginx()).provides, []);
  const present = [provider('net.php.php', 'PHP', [{ interface: 'fastcgi', socket: '/run/php-fpm.sock' }])];
  const used = sheetOf(notes(), { installed: providersOf(present.concat([valkey()])) });
  assert.deepEqual(used.uses, [{ words: 'fastcgi', by: ['PHP'], missing: false, name: 'PHP-FPM' }]);
  assert.deepEqual(used.needs, []);
  const missing = sheetOf(notes(), { installed: providersOf([valkey()]) });
  assert.deepEqual(missing.uses[0], { words: 'fastcgi', by: [], missing: true, name: 'PHP-FPM' });
  assert.deepEqual(missing.needs, ['Needs PHP-FPM, which is not installed.']);
  const unread = sheetOf(notes());
  assert.deepEqual(unread.uses[0].by, null);
  assert.deepEqual(unread.needs, [], 'what is installed was not read, so nothing is said of what is missing');
});

test('a use is matched by its interface when it names one, else by its socket', () => {
  const offers = providersOf([{ id: 'a.b.c', name: 'ABC', provides: [{ interface: 'redis', socket: '/run/a.sock' }] }]);
  const by = (use) => sheetOf(notes({ uses: [use] }), { installed: offers }).uses[0].by;
  assert.deepEqual(by({ interface: 'redis' }), ['ABC']);
  assert.deepEqual(by({ interface: 'fastcgi', socket: '/run/a.sock' }), [], 'the interface decides when there is one');
  assert.deepEqual(by({ socket: '/run/a.sock' }), ['ABC']);
  assert.deepEqual(by({ socket: '/run/b.sock' }), []);
  assert.deepEqual(sheetOf(notes({ uses: [{}] })).uses, [], 'a use that names nothing is not listed');
  assert.deepEqual(sheetOf(notes({ uses: [{ socket: '/run/b.sock' }] }), { installed: offers }).needs, ['Needs at /run/b.sock, which no installed app provides.']);
});

test('the apps that are installed, for matching, are their ID, name and what they provide', () => {
  assert.deepEqual(providersOf([{ id: 'a.b.c', provides: [{ interface: 'x' }] }, { name: 'No ID' }, null]), [{ id: 'a.b.c', name: 'a.b.c', provides: [{ interface: 'x' }] }]);
  assert.deepEqual(providersOf([{ id: 'a.b.c', name: 'ABC' }]), [{ id: 'a.b.c', name: 'ABC', provides: [] }]);
});

test('an app that is installed says its version and that updating is on its page, and offers no install', () => {
  const sheet = sheetOf(nginx({ installed: '1.30.5' }), { admin: true });
  assert.deepEqual(sheet.installed, { words: 'Version 1.30.5 is installed. Updating is on its page.', href: '/apps/org%2Enginx%2Enginx' });
  assert.equal(sheet.install.can, false);
  assert.equal(sheetOf(nginx(), { admin: true }).installed, null);
});

test('a refusal is the words bundled gives, and an app that would be refused offers no install', () => {
  const words = 'It is not signed by a key this computer takes.';
  const sheet = sheetOf(nginx({ refusal: words }), { admin: true });
  assert.equal(sheet.refusal, words);
  assert.equal(sheet.install.can, false);
  assert.equal(sheetOf(nginx({ refusal: '' }), { admin: true }).refusal, null);
  assert.equal(sheetOf(nginx({ refusal: null }), { admin: true }).install.can, true);
});

test('Install is offered to an administrator for a sheet that has its SHA-256, and a reader is told who installs', () => {
  const admin = sheetOf(nginx(), { admin: true });
  assert.deepEqual(admin.install, { can: true, reader: false, command: 'elevate bundle install org.nginx.nginx --for-everyone' });
  assert.equal(admin.sha256, SHA);
  const reader = sheetOf(nginx(), { admin: false });
  assert.deepEqual(reader.install, { can: false, reader: true, command: 'elevate bundle install org.nginx.nginx --for-everyone' });
  assert.equal(READER_WORDS, 'An administrator installs apps for everyone.');
  const bad = sheetOf(nginx({ sha256: 'XYZ' }), { admin: true });
  assert.equal(bad.sha256, null);
  assert.equal(bad.install.can, false, 'an install is pinned by the sheet\'s hash, so none without one');
  assert.equal(sheetOf(nginx({ sha256: OTHER_SHA }), { admin: true }).sha256, OTHER_SHA);
});

test('an app that has no name has its ID, and the sheet has none of what a catalog did not say', () => {
  const sheet = sheetOf({ id: 'org.example.bare' });
  assert.equal(sheet.name, 'org.example.bare');
  assert.equal(sheet.version, '');
  assert.equal(sheet.summary, null);
  assert.equal(sheet.size, null);
  assert.deepEqual(sheet.may, []);
  assert.deepEqual(sheet.services, []);
  assert.equal(sheet.line.words, 'signed');
});

// ---- when it fails ----

test('a sheet that could not be fetched says why, without a double full stop', () => {
  assert.equal(fetchWords({ title: 'No catalog lists that app', detail: 'org.example.gone is in no catalog.' }), 'The app could not be fetched: org.example.gone is in no catalog.');
  assert.equal(fetchWords({ title: 'The catalog could not be read' }), 'The app could not be fetched: The catalog could not be read.');
  assert.equal(fetchWords(new Error()), 'The app could not be fetched: no reason was given.');
  assert.equal(fetchWords(null), 'The app could not be fetched: no reason was given.');
});

test('an install that the catalog moved past is known by its error, and by the note or the log of the task that failed', () => {
  assert.equal(isStale({ id: 'APP-102' }), true);
  assert.equal(isStale({ id: 'APP-101' }), false);
  assert.equal(isStale(null), false);
  assert.equal(isStale(null, { note: 'bundle: the catalog\'s file is not the one whose sheet you read (APP-102)' }), true);
  assert.equal(isStale(null, { log: ['downloading', 'bundle: APP-102'] }), true);
  assert.equal(isStale(null, { log: ['downloading', 'bundle: APP-103'], note: 'failed' }), false);
  assert.equal(isStale(null, { log: [1, null] }), false);
});

// ---- after the install ----

test('the next step is nginx\'s websites, PostgreSQL\'s databases, PHP\'s pages, and for any other app its own page', () => {
  assert.deepEqual(nextStep('org.nginx.nginx'), { words: 'Add a website', href: '/websites/add' });
  assert.deepEqual(nextStep('org.postgresql.postgresql'), { words: 'Add a database', href: '/databases/add' });
  assert.deepEqual(nextStep('net.php.php'), { words: 'Add a PHP website', href: '/websites/add?kind=php' });
  assert.deepEqual(nextStep('org.example.notes'), { words: 'Open its page', href: '/apps/org%2Eexample%2Enotes' });
});

test('after the install: what was installed, each service with whether it is on, where its data is, and the next step', () => {
  const info = valkey({ id: 'org.nginx.nginx', name: 'nginx', version: '1.30.6', service_list: [{ name: 'nginx', account: 'nginx', start: 'off' }] });
  const off = afterOf(info, { services: [{ name: 'nginx', state: 'stopped', enabled: false }] });
  assert.equal(off.installed, 'Installed nginx 1.30.6 for everyone.');
  assert.deepEqual(off.services, [{ name: 'nginx', href: '/services/nginx', on: false, words: 'Its service nginx is off.', kind: 'off' }]);
  assert.equal(off.data, 'Its data is in /shared/services/org.nginx.nginx/nginx once it has started.');
  assert.deepEqual(off.next, { words: 'Add a website', href: '/websites/add' });
  const on = afterOf(info, { services: [{ name: 'nginx', state: 'running', enabled: true }] });
  assert.deepEqual(on.services, [{ name: 'nginx', href: '/services/nginx', on: true, words: 'Its service nginx is on: running, starts at boot.', kind: 'ok' }]);
});

test('what is said when Spark does not know the service or could not be read, and for an app with no service', () => {
  const info = valkey({ id: 'org.nginx.nginx', name: 'nginx', version: '1.30.6', service_list: [{ name: 'nginx', account: 'nginx', start: 'off' }] });
  assert.equal(afterOf(info, null).services[0].words, 'Its service nginx is off.');
  assert.equal(afterOf(info, services('stopped', false)).services[0].words, 'Its service nginx is off.', 'Spark does not list it');
  const started = valkey({ id: 'org.nginx.nginx', name: 'nginx', version: '1.30.6', service_list: [{ name: 'nginx', account: 'nginx', start: 'on' }] });
  assert.equal(afterOf(started, null).services[0].words, 'Its service nginx is on: starts at boot.');
  const bare = afterOf(valkey({ id: 'org.example.notes', name: 'Notes', version: '2.0.1', service_list: [] }));
  assert.deepEqual(bare.services, []);
  assert.equal(bare.data, null);
  assert.deepEqual(bare.next, { words: 'Open its page', href: '/apps/org%2Eexample%2Enotes' });
  const older = afterOf(valkey({ service_list: undefined, services: ['valkey: the server'] }));
  assert.deepEqual(older.services, []);
});
