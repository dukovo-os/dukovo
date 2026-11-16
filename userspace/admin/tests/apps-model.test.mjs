// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Apps pages say, held word by word: the order of the list, an
// update and the flaws it fixes, the apps that use an app, the apps that are a
// person's own, the words of the remove dialog, the calls the buttons make,
// and the tasks that run on an app.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  appHref, askedMore, checkOf, fixesWords, flawWords, offersOf, sheetOf, updateWords,
} from '../pages/apps/apps-model.js';
import {
  bannerOf, byline, dataFolders, fixedWords, flawLine, flawsOf, heldWords, linksOf, mayUseOf, noFlawsWords, NO_PREVIOUS,
  REMOVE_CHOICES, removeLead, rollbackWords, ruleWords, runsAs, serviceLines, servicesOf, sizeWords, usedByOf, usersWords,
  versionsOf,
} from '../pages/apps/app-model.js';
import {
  captionWords, checkedWords, idOf, initialOf, ownCommands, paletteWords, rowsOf, stateWord, tintOf, updatable,
} from '../pages/apps/list-model.js';
import {
  holdCall, jobWords, makeJobs, removeCall, rollbackCall, runWords, updateAllCall, updateCall,
} from '../pages/apps/run-model.js';
import {
  last, list, plain, services, settings, task, time, valkey,
} from './apps-fixtures.mjs';

const idsOf = (rows) => rows.map((row) => row.id);

// ---- the address of a page ----

test('an app\'s address writes its dots out, and an address is read back into an ID, or into none', () => {
  assert.equal(appHref('io.valkey.valkey'), '/apps/io%2Evalkey%2Evalkey');
  assert.equal(appHref('org.nodejs.node@22'), '/apps/org%2Enodejs%2Enode%4022');
  assert.equal(idOf(''), '');
  assert.equal(idOf('/'), '');
  assert.equal(idOf('io%2Evalkey%2Evalkey'), 'io.valkey.valkey');
  assert.equal(idOf('io.valkey.valkey/'), 'io.valkey.valkey');
  assert.equal(idOf('org%2Enodejs%2Enode%4022'), 'org.nodejs.node@22');
  assert.equal(idOf('..%2Fx'), null);
  assert.equal(idOf('a%2'), null);
  assert.equal(idOf('a b'), null);
  assert.equal(idOf('a/b'), null);
});

test('a mark is the first letter of the name in capitals, and the same tint for the same ID', () => {
  assert.equal(initialOf('Valkey'), 'V');
  assert.equal(initialOf('  nginx'), 'N');
  assert.equal(initialOf('éclair'), 'É');
  assert.equal(initialOf(''), '?');
  assert.equal(tintOf('io.valkey.valkey'), tintOf('io.valkey.valkey'));
  for (const id of ['a', 'io.valkey.valkey', 'org.example.notes', '🙂']) assert.ok([0, 1, 2, 3, 4, 5].includes(tintOf(id)), id);
});

// ---- the last check ----

test('the offers of the apps are by ID, and the system, the firmware, a current offer and a withdrawn one are none', () => {
  const found = offersOf(last({
    offers: [
      { item: 'system', from: '1', to: '2', state: 'update' },
      { item: 'firmware', from: '1', to: '2', state: 'update' },
      { item: 'io.a.a', from: '1.0.0', to: '1.0.1', state: 'update' },
      { item: 'io.b.b', from: '1.0.0', to: '1.0.0', state: 'current' },
      { item: 'io.c.c', from: '1.0.0', to: '1.0.1', state: 'withdrawn' },
      { from: '1', to: '2' },
      null,
    ],
  }));
  assert.deepEqual([...found.keys()], ['io.a.a']);
  assert.equal(offersOf(null).size, 0);
  assert.equal(offersOf({}).size, 0);
});

test('the check says whether the apps were checked, were not, or could not be, and a read that failed is its own state', () => {
  assert.equal(checkOf(last()), 'checked');
  assert.equal(checkOf(null), 'none');
  assert.equal(checkOf(undefined), 'unknown');
  assert.equal(checkOf(last({ unchecked: [{ item: 'apps', why: 'no route' }] })), 'unchecked');
  assert.equal(checkOf(last({ unchecked: [{ item: 'security', why: 'no route' }] })), 'checked', 'only the apps count');
});

test('the line over the table says when the apps were checked and why they could not be, never "up to date" for a check that did not run', () => {
  assert.equal(checkedWords(last(), time), 'Checked 4 minutes ago.');
  assert.equal(checkedWords(last({ unchecked: [{ item: 'apps', why: 'no route to updates.dukovo.org' }] }), time), 'Checked 4 minutes ago. Could not check the apps: no route to updates.dukovo.org.');
  assert.equal(checkedWords(null, time), 'Not checked yet.');
  assert.equal(checkedWords(undefined, time), 'The last check could not be read.');
});

// ---- an update, in words ----

test('an update says its version and its kind, a waiting one its wave, a held one the version it is held at', () => {
  assert.deepEqual(updateWords({ from: '8.0.3', to: '8.0.4', state: 'update' }), {
    state: 'update', to: '8.0.4', kind: 'fixes only', pair: '8.0.3 to 8.0.4', words: '8.0.4 · fixes only',
  });
  assert.equal(updateWords({ from: '17.2', to: '18.0', state: 'update' }).words, '18.0 · new version');
  assert.equal(updateWords({ from: '8.4.2', to: '8.4.3', state: 'waiting', wave_day: 3 }).words, 'Reaches this machine on day 3 of its waves');
  assert.equal(updateWords({ from: '8.4.2', to: '8.4.3', state: 'waiting' }).words, 'Waits for its wave');
  assert.equal(updateWords({ from: '3.47.2', to: '3.48.0', state: 'held' }).words, 'Held at 3.47.2');
});

test('what an update fixes is counted, with the worst of it when there are several', () => {
  assert.equal(fixesWords([]), null);
  assert.equal(fixesWords([{ severity: 'high' }]), 'Fixes 1 security advisory, high');
  assert.equal(fixesWords([{ severity: 'low' }, { severity: 'critical' }, { severity: 'medium' }]), 'Fixes 3 security advisories, worst critical');
  assert.equal(fixesWords([{ severity: 'unknown' }]), 'Fixes 1 security advisory');
});

test('the flaws of the version installed are counted with the worst, and none known says nothing', () => {
  assert.equal(flawWords({ state: 'known', count: 2, worst: 'high' }), '2 known flaws, worst high');
  assert.equal(flawWords({ state: 'known', count: 1, worst: 'low' }), '1 known flaw, worst low');
  assert.equal(flawWords({ state: 'known', count: 1, worst: 'unknown' }), '1 known flaw');
  for (const none of [{ state: 'none', count: 0, worst: null }, { state: 'unknown' }, null, undefined, { state: 'known', count: 0 }]) assert.equal(flawWords(none), null);
});

// ---- the list ----

test('the list puts the flawed first, the worst first, then the updates, then the rest by name', () => {
  const rows = rowsOf(list(), last());
  assert.deepEqual(idsOf(rows.everyone), [
    'io.valkey.valkey', 'net.php.php-fpm', 'org.postgresql.postgresql', 'io.nginx.nginx', 'org.python.python', 'org.sqlite.sqlite',
  ]);
  const flawed = list();
  flawed.bundles.find((one) => one.id === 'io.nginx.nginx').flaws = { state: 'known', count: 1, worst: 'critical' };
  flawed.bundles.find((one) => one.id === 'org.python.python').flaws = { state: 'known', count: 3, worst: 'medium' };
  assert.deepEqual(idsOf(rowsOf(flawed, last()).everyone).slice(0, 3), ['io.nginx.nginx', 'io.valkey.valkey', 'org.python.python'], 'critical, high, medium');
});

test('two apps of one name are in the order of their IDs, and names are compared without their case', () => {
  const same = { bundles: [
    { id: 'b.same', name: 'same', version: '1', for_everyone: true },
    { id: 'a.same', name: 'Same', version: '1', for_everyone: true },
    { id: 'c.alpha', name: 'alpha', version: '1', for_everyone: true },
  ] };
  assert.deepEqual(idsOf(rowsOf(same, last()).everyone), ['c.alpha', 'a.same', 'b.same']);
});

test('an app that is a person\'s own is listed apart, Yours, with the commands that update and remove it', () => {
  const rows = rowsOf(list(), last());
  assert.deepEqual(idsOf(rows.yours), ['org.example.tool']);
  assert.ok(!idsOf(rows.everyone).includes('org.example.tool'));
  assert.deepEqual(ownCommands('org.example.tool'), { update: 'bundle update org.example.tool', remove: 'bundle remove org.example.tool' });
  assert.equal(captionWords(6, 'for everyone'), '6 apps installed for everyone');
  assert.equal(captionWords(1, 'for you'), '1 app installed for you');
});

test('a row says its update from the check, then from the list, and what it says when there is none depends on what the check says', () => {
  const byId = (rows, id) => rows.everyone.find((row) => row.id === id);
  const checked = rowsOf(list(), last());
  assert.equal(byId(checked, 'io.nginx.nginx').update.words, 'Up to date');
  assert.equal(byId(checked, 'io.valkey.valkey').update.words, '8.0.4 · fixes only');
  assert.equal(byId(checked, 'net.php.php-fpm').update.state, 'waiting');
  assert.equal(byId(checked, 'org.sqlite.sqlite').update.words, 'Held at 3.47.2');
  // No check has run: the list's own `update` is what is known, and the rest are not known to be current.
  const none = rowsOf(list(), null);
  assert.equal(byId(none, 'org.postgresql.postgresql').update.words, '17.3 · new version');
  assert.equal(byId(none, 'io.nginx.nginx').update.words, 'Not checked yet');
  assert.equal(byId(rowsOf(list(), undefined), 'io.nginx.nginx').update.words, 'Not known');
  assert.equal(byId(rowsOf(list(), last({ unchecked: [{ item: 'apps', why: 'x' }] })), 'io.nginx.nginx').update.words, 'Could not check');
});

test('a held app is held whatever the check says, and says the version it is held at', () => {
  const held = list();
  held.bundles.find((one) => one.id === 'io.nginx.nginx').held = true;
  const row = rowsOf(held, last()).everyone.find((one) => one.id === 'io.nginx.nginx');
  assert.equal(row.held, true);
  assert.equal(row.update.state, 'held');
  assert.equal(row.update.words, 'Held at 1.27.3');
});

test('a row carries the flaws of the version installed, in the warning colour for high and critical only', () => {
  const rows = rowsOf(list(), last()).everyone;
  assert.deepEqual(rows[0].flaws, { words: '2 known flaws, worst high', worst: 'high', urgent: true });
  assert.equal(rows[1].flaws, null);
  const low = rowsOf({ bundles: [{ id: 'a.b', name: 'AB', version: '1', for_everyone: true, flaws: { state: 'known', count: 1, worst: 'medium' } }] }, null).everyone[0];
  assert.equal(low.flaws.urgent, false);
});

test('a bundle with no ID is no row, a name that is missing is the ID, and a list that is not one is empty', () => {
  const rows = rowsOf({ bundles: [{ name: 'x', version: '1' }, { id: 'a.b', version: '2', for_everyone: true }, null] }, null);
  assert.deepEqual(rows.everyone.map((row) => [row.id, row.name, row.version]), [['a.b', 'a.b', '2']]);
  assert.deepEqual(rowsOf(null, null), { everyone: [], yours: [] });
  assert.deepEqual(rowsOf({ bundles: 3 }, null), { everyone: [], yours: [] });
});

test('the states the palette filters by, and the words after an app\'s name in it', () => {
  const rows = Object.fromEntries(rowsOf(list(), last()).everyone.map((row) => [row.id, row]));
  assert.equal(stateWord(rows['io.valkey.valkey']), 'flawed');
  assert.equal(stateWord(rows['org.sqlite.sqlite']), 'held');
  assert.equal(stateWord(rows['org.postgresql.postgresql']), 'update');
  assert.equal(stateWord(rows['net.php.php-fpm']), 'update');
  assert.equal(stateWord(rows['io.nginx.nginx']), null);
  assert.equal(paletteWords(rows['io.valkey.valkey']), '8.0.3 · update waits · 2 known flaws, worst high');
  assert.equal(paletteWords(rows['org.sqlite.sqlite']), '3.47.2 · held');
  assert.equal(paletteWords(rows['io.nginx.nginx']), '1.27.3');
  assert.deepEqual(updatable(Object.values(rows)).map((row) => row.id).sort(), ['io.valkey.valkey', 'org.postgresql.postgresql']);
});

// ---- an app's page ----

test('the line under an app\'s name says where it came from, that it is signed, and its licence', () => {
  assert.equal(byline(valkey()), 'Dukovo catalog · signed · BSD-3-Clause');
  assert.equal(byline(valkey({ trust: { kind: 'developer' }, license: 'MIT OR Apache-2.0' })), 'signed by its developer\'s key, trusted by hand · MIT OR Apache-2.0');
  assert.equal(byline(valkey({ trust: { kind: 'bound' }, license: undefined })), 'a file you installed · signed');
  assert.equal(byline(valkey({ trust: { kind: 'bound_for_everyone' } })), 'a file installed for everyone · signed · BSD-3-Clause');
  assert.equal(byline(valkey({ trust: { kind: 'unknown' }, update_feed: 'https://example.org/feed' })), 'its own feed · signed · BSD-3-Clause');
});

test('a flaw is fixed by the offered version when that version is the fix or later, and by none when the fix is not a number or not known', () => {
  const flaws = (to) => flawsOf(valkey(), to).map((flaw) => [flaw.id, flaw.byOffer]);
  assert.deepEqual(flaws('8.0.4'), [['DKSA-2026-0007', true], ['DKSA-2026-0003', false]]);
  assert.deepEqual(flaws('8.0.3'), [['DKSA-2026-0007', false], ['DKSA-2026-0003', false]]);
  assert.deepEqual(flaws('9.0.0'), [['DKSA-2026-0007', true], ['DKSA-2026-0003', false]]);
  assert.deepEqual(flaws('8.0.10'), [['DKSA-2026-0007', true], ['DKSA-2026-0003', false]], 'numbers, not words');
  assert.deepEqual(flaws(null), [['DKSA-2026-0007', false], ['DKSA-2026-0003', false]]);
  assert.deepEqual(flaws('v8.0.4'), [['DKSA-2026-0007', true], ['DKSA-2026-0003', false]]);
  assert.deepEqual(flawsOf(valkey({ flaws: { state: 'listed', known: [{ id: 'X', severity: 'low', fixed: 'later' }, null, { severity: 'low' }] } }), '2.0').map((flaw) => flaw.byOffer), [false]);
  assert.equal(fixedWords(flawsOf(valkey(), '8.0.4')[0]), 'Fixed in 8.0.4, which the update brings.');
  assert.equal(fixedWords(flawsOf(valkey(), '8.0.3')[0]), 'Fixed in 8.0.4.');
  assert.equal(fixedWords(flawsOf(valkey(), '8.0.4')[1]), 'No fix is known yet.');
  assert.equal(flawLine(flawsOf(valkey(), null)[0]), 'DKSA-2026-0007 · high · valkey: a crafted command stops the server');
});

test('the banner names the update and the flaws it fixes, and is red for a flaw rated high, amber for one that is not, and blue for an update alone', () => {
  const row = rowsOf(list(), last()).everyone[0];
  const banner = bannerOf(valkey(), row.update);
  assert.equal(banner.kind, 'bad');
  assert.equal(banner.heading, '8.0.4 is available (fixes only). 8.0.3 has 2 known flaws, worst high; 8.0.4 fixes 1 of them.');
  assert.equal(banner.flaws.length, 2);
  const mild = valkey({ flaws: { state: 'listed', known: [{ id: 'X-1', severity: 'low', component: 'valkey', fixed: '8.0.4', summary: 's' }] } });
  assert.deepEqual([bannerOf(mild, row.update).kind, bannerOf(mild, row.update).heading], ['warn', '8.0.4 is available (fixes only). 8.0.3 has 1 known flaw, worst low; 8.0.4 fixes all of them.']);
  const quiet = valkey({ flaws: { state: 'none', known: [] } });
  assert.deepEqual([bannerOf(quiet, row.update).kind, bannerOf(quiet, row.update).heading], ['info', '8.0.4 is available (fixes only).']);
  const current = { state: 'current', to: null, kind: null, pair: null, words: 'Up to date' };
  assert.equal(bannerOf(quiet, current), null);
  assert.equal(bannerOf(valkey(), current).heading, '8.0.3 has 2 known flaws, worst high, and the catalog lists no newer version yet.');
  assert.equal(bannerOf(valkey(), updateWords({ from: '8.0.3', to: '8.0.4', state: 'waiting' })).heading.split('. ')[0], '8.0.4 waits for its wave');
  assert.equal(bannerOf(quiet, updateWords({ from: '8.0.3', to: '8.0.4', state: 'held' })).heading, '8.0.4 is available, and 8.0.3 is held.');
});

test('a catalog that says nothing of the flaws is told so, and an app whose facts do not list flaws says nothing', () => {
  assert.equal(noFlawsWords(valkey({ flaws: { state: 'unknown', known: [] } })), 'The catalog says nothing of this version\'s flaws.');
  assert.equal(noFlawsWords(valkey({ flaws: { state: 'none', known: [] } })), 'None known.');
  assert.equal(noFlawsWords(valkey()), null, 'the flaws are in the banner');
  assert.equal(noFlawsWords(valkey({ flaws: undefined })), null);
});

test('the services are the manifest\'s, with Spark\'s state when Spark knows them, and the manifest\'s start when it does not', () => {
  assert.deepEqual(servicesOf(valkey(), services()), [{
    name: 'valkey', account: 'valkey', href: '/services/valkey', state: { word: 'Stopped', kind: 'off' }, boot: 'off at boot',
  }]);
  assert.deepEqual(servicesOf(valkey(), services('running', true))[0].state, { word: 'Running', kind: 'ok' });
  assert.equal(servicesOf(valkey(), services('running', true))[0].boot, 'starts at boot');
  assert.equal(servicesOf(valkey(), services('failed', true))[0].state.kind, 'bad');
  assert.equal(servicesOf(valkey(), services('odd', true))[0].state.word, 'Odd');
  assert.equal(servicesOf(valkey(), services().services)[0].state.word, 'Stopped', 'Spark\'s list alone');
  const without = servicesOf(valkey({ service_list: [{ name: 'valkey', account: 'valkey', start: 'on' }] }), null);
  assert.deepEqual([without[0].state, without[0].boot], [null, 'starts at boot']);
  assert.equal(servicesOf(valkey(), null)[0].boot, 'off at boot');
  assert.equal(servicesOf(valkey({ service_list: [{ name: 'a.b' }] }), null)[0].href, '/services/a%2Eb');
  assert.equal(servicesOf(valkey({ service_list: undefined }), null), null);
  assert.deepEqual(serviceLines(valkey({ service_list: undefined })), ['valkey: the server']);
  assert.deepEqual(servicesOf(plain('a.b', 'AB', '1')), []);
});

test('the account a service runs as is said once for each account', () => {
  assert.equal(runsAs(servicesOf(valkey())), 'its own account, valkey');
  assert.equal(runsAs([{ account: 'web' }, { account: 'worker' }, { account: 'web' }]), 'its own accounts, web, worker');
  assert.equal(runsAs([]), null);
});

test('what an app may use is each permission in the system\'s words with its folders and ports', () => {
  assert.deepEqual(mayUseOf(valkey()), [
    'keep its data in its own folder · folders /shared/services/io.valkey.valkey (read and write)',
    'listen for connections · port 6379',
  ]);
  assert.deepEqual(mayUseOf(valkey({ permissions: [{ name: 'x.y', ports: [1, 2], apps: ['io.a.a'] }, null, {}] })), ['x.y · ports 1, 2 · with io.a.a']);
  assert.deepEqual(mayUseOf(plain('a.b', 'AB', '1')), []);
});

test('what an app provides and uses are its interfaces and sockets, and an older bundle info that says none leaves the rows out', () => {
  assert.deepEqual(linksOf(valkey(), 'provides'), ['redis at /run/valkey/valkey.sock']);
  assert.equal(linksOf(valkey(), 'uses'), null);
  assert.equal(linksOf(valkey({ provides: undefined }), 'provides'), null);
  assert.deepEqual(linksOf(valkey({ uses: [{ interface: 'fastcgi' }, { socket: '/run/x.sock' }, {}] }), 'uses'), ['fastcgi', 'at /run/x.sock']);
});

test('the apps that use an app are links with what they use it for, and none is an empty list, not a missing one', () => {
  assert.deepEqual(usedByOf(valkey()), [{
    id: 'net.php.php-fpm', name: 'PHP-FPM', what: 'sessions', href: '/apps/net%2Ephp%2Ephp-fpm',
  }]);
  assert.deepEqual(usedByOf(valkey({ used_by: [] })), []);
  assert.equal(usedByOf(valkey({ used_by: undefined })), null, 'an older bundle info does not say');
  assert.deepEqual(usedByOf(valkey({ used_by: [{ id: 'a.b' }, {}] })).map((one) => [one.name, one.what]), [['a.b', null]]);
});

test('the data is the folder of each service, its size is told when it is known, and the rule is the Updates page\'s', () => {
  assert.deepEqual(dataFolders(valkey(), servicesOf(valkey())), ['/shared/services/io.valkey.valkey/valkey']);
  assert.deepEqual(dataFolders(valkey(), [{ account: 'a' }, { account: 'a' }, { account: 'b' }]), ['/shared/services/io.valkey.valkey/a', '/shared/services/io.valkey.valkey/b']);
  assert.equal(sizeWords(valkey({ bytes: 9_437_184 })), '9.44 MB');
  assert.equal(sizeWords(valkey({ bytes: null })), null);
  assert.equal(ruleWords(settings('auto')), 'Update by themselves');
  assert.equal(ruleWords(settings('notify')), 'Tell me');
  assert.equal(ruleWords(null), null);
});

test('the versions are the one offered, the one installed and the one kept, with what each is', () => {
  const row = rowsOf(list(), last()).everyone[0];
  assert.deepEqual(versionsOf(valkey(), row.update, time), [
    { version: '8.0.4', note: 'Fixes only', pill: { words: 'available', kind: 'info' }, back: false },
    { version: '8.0.3', note: 'Installed 4 days ago', pill: { words: 'installed', kind: 'ok' }, back: false },
    { version: '8.0.2', note: 'Kept for going back', pill: null, back: true },
  ]);
  const current = { state: 'current', to: null };
  assert.deepEqual(versionsOf(plain('a.b', 'AB', '1.0.0', { installed: undefined }), current, time), [
    { version: '1.0.0', note: null, pill: { words: 'installed', kind: 'ok' }, back: false },
  ]);
  const held = versionsOf(valkey({ held: true }), updateWords({ from: '8.0.3', to: '8.0.4', state: 'held' }), time);
  assert.deepEqual(held.map((one) => one.pill?.words), ['held back', 'held', undefined]);
  assert.match(NO_PREVIOUS, /^No earlier version is kept yet/);
});

test('going back says what it does to the data, and a held app says what the hold means', () => {
  assert.equal(rollbackWords(valkey()), 'Goes back to 8.0.2 and holds it there. Its data comes back as 8.0.2 left it: its services\' data at once, the rest at its next run (bundle(1)). Data written since the update is kept in its snapshot for 30 days.');
  assert.equal(heldWords(valkey({ held: true })), 'Held at 8.0.3: no update is taken, security fixes included. Release the hold to take updates again.');
});

// ---- the remove dialog ----

test('the remove dialog offers to keep the data, which is the first, and to remove it with the app', () => {
  assert.deepEqual(REMOVE_CHOICES.map((choice) => [choice.value, choice.label]), [['keep', 'Keep its data'], ['purge', 'Remove its data too']]);
  assert.equal(removeLead(valkey()), 'Valkey 8.0.3 is removed for everyone, and its services stop and go.');
});

test('the remove dialog says what stops without an app, in the words of each app that uses it', () => {
  assert.deepEqual(usersWords(usedByOf(valkey())), ['PHP-FPM uses it: removing it stops PHP-FPM\'s sessions.']);
  assert.deepEqual(usersWords(usedByOf(valkey({ used_by: [{ id: 'a.b', name: 'AB', what: 'sessions' }, { id: 'c.d', name: 'CD' }] }))), [
    'AB uses it: removing it stops AB\'s sessions.',
    'CD uses it, and may stop working without it.',
  ]);
  assert.deepEqual(usersWords([]), []);
});

// ---- what the buttons run ----

test('each button makes the call of its action, and the arguments that action takes', () => {
  assert.deepEqual(updateCall('io.valkey.valkey'), { action: 'apps.update', args: { id: 'io.valkey.valkey' } });
  assert.deepEqual(updateCall('io.valkey.valkey', { yes: true }), { action: 'apps.update', args: { id: 'io.valkey.valkey', yes: true } });
  assert.deepEqual(updateAllCall(), { action: 'apps.update-all', args: {} });
  assert.deepEqual(rollbackCall('a.b'), { action: 'apps.rollback', args: { id: 'a.b' } });
  assert.deepEqual(holdCall('a.b', false), { action: 'apps.hold', args: { id: 'a.b' } });
  assert.deepEqual(holdCall('a.b', true), { action: 'apps.unhold', args: { id: 'a.b' } });
  assert.deepEqual(removeCall('a.b', 'keep'), { action: 'apps.remove', args: { id: 'a.b', data: 'keep' } });
  assert.deepEqual(removeCall('a.b', 'purge'), { action: 'apps.remove', args: { id: 'a.b', data: 'purge' } });
});

test('each button says what it does to the unlock dialog, the notice and its own face', () => {
  assert.deepEqual(runWords('update', 'Valkey', '8.0.4'), {
    unlock: 'update Valkey', running: 'Updating Valkey to 8.0.4', done: 'Valkey is updated.', busy: 'Starting…',
  });
  assert.equal(runWords('rollback', 'Valkey', '8.0.2').running, 'Going back on Valkey to 8.0.2');
  assert.equal(runWords('remove', 'Valkey').running, 'Removing Valkey');
  assert.equal(runWords('hold', 'Valkey', '8.0.3').done, 'Valkey is held at 8.0.3.');
  assert.equal(runWords('unhold', 'Valkey').unlock, 'release Valkey');
});

// ---- the tasks ----

test('a task that runs on an app is learned from the list, and one that does not is left out', () => {
  const jobs = makeJobs();
  jobs.learn([
    task(4, 'apps.update', { id: 'io.valkey.valkey' }, { percent: 41 }),
    task(5, 'backups.run', { name: 'nas' }),
    task(6, 'apps.remove', { id: 'a.b' }, { state: 'done' }),
    task(7, 'apps.rollback', { id: 'c.d' }, { state: 'waiting', step: 2, steps: 4 }),
  ]);
  assert.equal(jobWords(jobs.of('io.valkey.valkey')), 'Updating… 41%');
  assert.equal(jobWords(jobs.of('c.d')), 'Going back… step 2 of 4');
  assert.equal(jobs.of('a.b'), null);
  assert.equal(jobs.of('x.y'), null);
  assert.equal(jobs.any(), true);
  jobs.learn(null);
  assert.equal(jobs.any(), false);
});

test('a task this page began is kept at once, and its events move it and end it', () => {
  const jobs = makeJobs();
  jobs.started(9, 'apps.update', { id: 'io.valkey.valkey' });
  assert.equal(jobWords(jobs.of('io.valkey.valkey')), 'Updating…');
  assert.deepEqual(jobs.event({ id: 9, action: 'apps.update', state: 'running', percent: 12 }), {});
  assert.equal(jobWords(jobs.of('io.valkey.valkey')), 'Updating… 12%');
  assert.deepEqual(jobs.event({ id: 9, action: 'apps.update', state: 'running', step: 3, steps: 4 }), {});
  assert.equal(jobWords(jobs.of('io.valkey.valkey')), 'Updating… 12%', 'a percent is kept when only a step comes');
  const ended = jobs.event({ id: 9, action: 'apps.update', state: 'done' });
  assert.deepEqual([ended.ended.app, ended.state], ['io.valkey.valkey', 'done']);
  assert.equal(jobs.of('io.valkey.valkey'), null);
});

test('an event of a task not known yet asks to be read whole, and an event of another action is nothing', () => {
  const jobs = makeJobs();
  assert.deepEqual(jobs.event({ id: 3, action: 'apps.remove', state: 'running' }), { learn: 3 });
  assert.deepEqual(jobs.event({ id: 3, action: 'apps.remove', state: 'done' }), {});
  assert.deepEqual(jobs.event({ id: 4, action: 'backups.run', state: 'running' }), {});
  assert.deepEqual(jobs.event(null), {});
  jobs.put(task(3, 'apps.remove', { id: 'a.b' }));
  assert.equal(jobWords(jobs.of('a.b')), 'Removing…');
});

test('the task that updates every app counts for an app that has an update, and for no other', () => {
  const jobs = makeJobs();
  jobs.started(12, 'apps.update-all', {});
  assert.equal(jobs.of('io.valkey.valkey', true).id, 12);
  assert.equal(jobs.of('io.nginx.nginx', false), null);
  assert.equal(jobs.of('io.nginx.nginx'), null);
  assert.equal(jobs.any(), true);
});

test('a percent is kept between 0 and 100, and a step of one is no step', () => {
  const jobs = makeJobs();
  jobs.learn([task(1, 'apps.update', { id: 'a.b' }, { percent: 140 }), task(2, 'apps.update', { id: 'c.d' }, { step: 1, steps: 1 })]);
  assert.equal(jobWords(jobs.of('a.b')), 'Updating… 100%');
  assert.equal(jobWords(jobs.of('c.d')), 'Updating…');
});

test('an update that stopped to ask is told from one that failed, and its sheet is what came before the question', () => {
  const asked = {
    state: 'failed',
    log: ['valkey 8.0.4 asks for more than 8.0.3 does:', '  - it may listen on all addresses', 'Install it?', 'bundle: not installed'],
  };
  assert.equal(askedMore(asked), true);
  assert.equal(sheetOf(asked), 'valkey 8.0.4 asks for more than 8.0.3 does:\n  - it may listen on all addresses');
  assert.equal(askedMore({ ...asked, state: 'done' }), false);
  assert.equal(askedMore({ state: 'failed', log: ['bundle: no route to the catalog'] }), false);
  assert.equal(askedMore(null), false);
  assert.equal(sheetOf({ log: ['one', 'two'] }), 'one\ntwo');
  assert.equal(sheetOf(null), '');
});
