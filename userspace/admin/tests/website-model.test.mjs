// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Websites pages say and send, as plain values: the name rule,
// the `www.` name, the folder, the certificate choices, the arguments, the
// steps and what stands after each fails, the rows of what happens, and the
// words of a site's list and page.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  addStep, allNames, backupRow, bothWords, certificateChoices, certificateNameOf, certificateNameProblem, certificateOf,
  certificatesFor, certificateSteps, checkWords, checkedProblems, confRows, covers, firewallRows, folderFor, folderProblem, folderWords,
  getArgs, getStep, handEditWords, httpsArgs, httpsLocked, httpsOf, httpsStep, installStep, KINDS, lastToUse, listed,
  liveUrl, machineNamesOf, missingApps, nameProblem, namesOf, namesProblem, offLead, openPortArgs, outcomeOf, ownerChoices, planChecks, planRows,
  portsFor, previewFiles, removeArgs, removeLead, serveWords, serverLines, siteArgs, snapshotRow, stateOf, STANDS, stepsOf, testArgs,
  wwwDefault, wwwOf,
} from '../lib/website-model.js';

const own = {
  name: 'this-machine', kind: 'self-signed', names: ['duke-nas.example.org', 'duke-nas', 'duke-nas.local', 'localhost'], days_left: 791,
};
const shop = {
  name: 'shop.example.org', kind: 'acme', names: ['shop.example.org', 'www.shop.example.org'], days_left: 75, not_after: '2027-01-28T00:00:00Z', renews: '2026-12-30T03:00:00Z',
};
const wild = { name: 'example.org', kind: 'acme', names: ['*.example.org'], days_left: 40 };
const added = {
  name: 'intranet', kind: 'added', names: ['intranet.example.org'], days_left: 200, not_after: '2027-06-01T00:00:00Z',
};
const dead = { ...shop, name: 'old.example.org', names: ['shop.example.org'], days_left: -3 };
const revoked = { ...shop, name: 'blog.example.org', revoked: true };
const list = [own, shop, wild, added, dead, revoked];
const date = (text) => text.slice(0, 10);

const form = (more = {}) => ({
  names: ['shop.example.org', 'www.shop.example.org'],
  kind: 'php',
  folder: '/shared/www/sites/shop.example.org',
  owner: 'webdev',
  how: 'letsencrypt',
  certificate: 'shop.example.org',
  email: '',
  ...more,
});

// ---- names ----

test('names are split at spaces, commas and line ends, lowercased, each once', () => {
  assert.deepEqual(namesOf(' Shop.Example.org, www.shop.example.org\nshop.example.org  '), ['shop.example.org', 'www.shop.example.org']);
  assert.deepEqual(namesOf(''), []);
  assert.deepEqual(namesOf(null), []);
});

test('the name rule is cert\'s: lowercase letters, digits and - in words of up to 63, no wildcard, no address', () => {
  for (const good of ['shop.example.org', 'a', 'localhost', 'a-b.c', 'xn--80ak6aa92e.com', '01.2.3.4', '9lives.example', 'a'.repeat(63), `${'a'.repeat(63)}.example`]) {
    assert.equal(nameProblem(good), '', good);
  }
  for (const bad of ['-x', 'x-', 'a..b', '.a', 'a.', 'Shop.org', 'x_y.org', 'a b', 'ünï.example', 'a'.repeat(64), `${'a.'.repeat(127)}a`, '']) {
    assert.notEqual(nameProblem(bad), '', JSON.stringify(bad));
  }
  assert.match(nameProblem('*.example.org'), /wildcard/);
  assert.match(nameProblem('192.168.1.10'), /address/);
  assert.match(nameProblem('::1'), /address/);
  assert.match(nameProblem('2001:db8::1'), /address/);
  assert.match(nameProblem('a..b'), /not a name a website can have/);
});

test('a name of 253 characters fits and one of 254 does not', () => {
  const labels = (count) => Array.from({ length: count }, () => 'a'.repeat(62)).join('.');
  const fits = `${labels(4)}.b`;
  assert.equal(fits.length, 253);
  assert.equal(nameProblem(fits), '');
  assert.notEqual(nameProblem(`${fits}b`), '', 'one more character is too much');
});

test('the names as a set: one to twenty, each right', () => {
  assert.match(namesProblem([]), /domain/);
  assert.equal(namesProblem(['a.example']), '');
  assert.equal(namesProblem(Array.from({ length: 20 }, (_, at) => `n${at}.example`)), '');
  assert.match(namesProblem(Array.from({ length: 21 }, (_, at) => `n${at}.example`)), /1 to 20 names, not 21/);
  assert.match(namesProblem(['a.example', '*.b.example']), /wildcard/);
});

test('the www name is offered for a name that has none and has a dot, and starts ticked unless the name is the machine\'s', () => {
  assert.equal(wwwOf('shop.example.org'), 'www.shop.example.org');
  assert.equal(wwwOf('www.shop.example.org'), null);
  assert.equal(wwwOf('localhost'), null);
  assert.equal(wwwDefault('shop.example.org', own.names), true);
  assert.equal(wwwDefault('duke-nas.example.org', own.names), false);
  assert.equal(wwwDefault('www.example.org', own.names), false);
  assert.equal(wwwDefault('localhost', own.names), false);
});

test('the names of the site: the domain, its www name when asked, and the others, each once', () => {
  assert.deepEqual(allNames({ domain: 'shop.example.org', www: true, more: ['blog.example.org', 'www.shop.example.org'] }), ['shop.example.org', 'www.shop.example.org', 'blog.example.org']);
  assert.deepEqual(allNames({ domain: 'shop.example.org', www: false, more: [] }), ['shop.example.org']);
  assert.deepEqual(allNames({ domain: 'localhost', www: true, more: [] }), ['localhost']);
  assert.deepEqual(allNames({ domain: '', www: true, more: ['x.example'] }), ['x.example']);
});

test('words for how many names there are, and for lists', () => {
  assert.deepEqual([1, 2, 3].map((count) => bothWords(Array(count).fill('x'))), ['the name', 'both names', 'all 3 names']);
  assert.equal(listed(['a']), 'a');
  assert.equal(listed(['a', 'b']), 'a and b');
  assert.equal(listed(['a', 'b', 'c']), 'a, b and c');
});

// ---- the folder and its owner ----

test('the folder is one under the sites\' folder, with no empty part, dot or line end', () => {
  assert.equal(folderFor('shop.example.org'), '/shared/www/sites/shop.example.org');
  for (const good of ['/shared/www/sites/x', '/shared/www/sites/x/y', '/shared/www/sites/a.b']) assert.equal(folderProblem(good), '', good);
  for (const bad of ['/etc', '/shared/www/sites', '/shared/www/sites/', '/shared/www/sitesx/y', '/shared/www/sites/..', '/shared/www/sites/a/../b', '/shared/www/sites//a', '/shared/www/sites/a/', '/shared/www/sites/a\nb', '/shared/www/sites/.', 'shared/www/sites/a']) {
    assert.notEqual(folderProblem(bad), '', JSON.stringify(bad));
  }
  assert.match(folderProblem('/etc'), /\/shared\/www\/sites\//);
});

test('the plan\'s folder says its files are kept only when it is there', () => {
  assert.equal(folderWords({ path: '/x', exists: false }), '');
  assert.equal(folderWords(null), '');
  assert.equal(folderWords({ path: '/x', exists: true, entries: 14 }), 'The folder is there with 14 files: they are kept, and served as they are.');
  assert.match(folderWords({ path: '/x', exists: true, entries: 1 }), /with 1 file:/);
  assert.match(folderWords({ path: '/x', exists: true }), /^The folder is there:/);
});

test('the owners are the accounts that are not disabled, the signed-in person first, then the administrators', () => {
  const accounts = [
    { name: 'sam', admin: false }, { name: 'old', admin: false, disabled: true }, { name: 'alice', admin: true }, { name: 'chase', admin: true }, { name: 'webdev', admin: false },
  ];
  assert.deepEqual(ownerChoices(accounts, 'chase'), ['chase', 'alice', 'sam', 'webdev']);
  assert.deepEqual(ownerChoices({ accounts }, 'sam'), ['sam', 'alice', 'chase', 'webdev']);
  assert.deepEqual(ownerChoices(null, 'sam'), []);
  assert.deepEqual(ownerChoices([{ name: '' }, {}], 'sam'), []);
});

// ---- certificates ----

test('a certificate covers a name that it names, or that a wildcard of one word names', () => {
  assert.equal(covers(['shop.example.org'], 'shop.example.org'), true);
  assert.equal(covers(['*.example.org'], 'shop.example.org'), true);
  assert.equal(covers(['*.example.org'], 'a.shop.example.org'), false, 'a wildcard stands for one word');
  assert.equal(covers(['*.example.org'], 'example.org'), false);
  assert.equal(covers(['*.example.org'], 'shop.example.com'), false);
  assert.equal(covers(['shop.example.org'], 'www.shop.example.org'), false);
  assert.equal(covers(undefined, 'a'), false);
});

test('the certificates that can serve a set of names exclude this machine\'s, the revoked and those out of time', () => {
  assert.deepEqual(certificatesFor(['shop.example.org', 'www.shop.example.org'], list).map((one) => one.name), ['shop.example.org']);
  assert.deepEqual(certificatesFor(['shop.example.org'], list).map((one) => one.name), ['shop.example.org', 'example.org']);
  assert.deepEqual(certificatesFor([], list), []);
  assert.deepEqual(certificatesFor(['a.example.org'], null), []);
  assert.deepEqual(machineNamesOf(list), own.names);
  assert.deepEqual(machineNamesOf([]), []);
});

test('the certificate\'s name is the first name, or the one given when that is too long', () => {
  assert.equal(certificateNameOf(['shop.example.org']), 'shop.example.org');
  const long = `${'a'.repeat(60)}.example.org`;
  assert.equal(certificateNameOf([long]), null);
  assert.equal(certificateNameOf([long], '  shop  '), 'shop');
  assert.equal(certificateNameOf(['a'.repeat(64)]), 'a'.repeat(64));
  assert.equal(certificateNameOf([]), null);
});

test('a certificate\'s name follows cert\'s rule', () => {
  for (const good of ['shop', 'shop.example.org', 'a-b.c', '9x', 'a'.repeat(64)]) assert.equal(certificateNameProblem(good), '', good);
  for (const bad of ['', '-x', '.x', 'a..b', 'A', 'a b', 'a/b', 'a'.repeat(65), 'a_b']) assert.notEqual(certificateNameProblem(bad), '', JSON.stringify(bad));
});

test('the choices for a name that is not the machine\'s: Let\'s Encrypt first, none last in the warning colour', () => {
  const { options, initial } = certificateChoices({ names: ['blog.example.net'], list });
  assert.deepEqual(options.map((one) => one.id), ['letsencrypt', 'none']);
  assert.equal(initial, 'letsencrypt');
  assert.deepEqual(options.map((one) => one.warn), [false, true]);
  assert.equal(options[0].off, false);
});

test('a name that is the machine\'s offers its own certificate, which starts chosen', () => {
  const { options, initial } = certificateChoices({ names: ['duke-nas.local', 'localhost'], list });
  assert.deepEqual(options.map((one) => one.id), ['letsencrypt', 'machine', 'none']);
  assert.equal(initial, 'machine');
  assert.equal(options[0].off, true, 'localhost has no dot');
  assert.match(options[0].why, /localhost has none/);
  // One name that is not the machine's takes this option away.
  assert.deepEqual(certificateChoices({ names: ['duke-nas.local', 'shop.example.org'], list }).options.map((one) => one.id), ['letsencrypt', 'none']);
});

test('a certificate that is here is offered when it names every name, and starts chosen when the page was sent to use it', () => {
  const names = ['shop.example.org', 'www.shop.example.org'];
  const plain = certificateChoices({ names, list });
  assert.deepEqual(plain.options.map((one) => one.id), ['letsencrypt', 'there', 'none']);
  assert.equal(plain.initial, 'letsencrypt');
  assert.deepEqual(plain.here.map((one) => one.name), ['shop.example.org']);
  assert.equal(certificateChoices({ names, list, preset: 'shop.example.org' }).initial, 'there');
  assert.equal(certificateChoices({ names, list, preset: 'nobody' }).initial, 'letsencrypt', 'a certificate that does not cover them is not used');
  // A name without a dot that a certificate covers is served by it, as Let's Encrypt cannot.
  const solo = certificateChoices({ names: ['intranet'], list: [{ ...added, names: ['intranet'] }] });
  assert.equal(solo.initial, 'there');
  assert.equal(certificateChoices({ names: ['intranet'], list }).initial, 'none');
});

test('the certificate a choice gives', () => {
  const names = ['shop.example.org'];
  assert.equal(certificateOf({ how: 'machine', names }), 'this-machine');
  assert.equal(certificateOf({ how: 'there', names, there: 'example.org' }), 'example.org');
  assert.equal(certificateOf({ how: 'there', names }), null);
  assert.equal(certificateOf({ how: 'letsencrypt', names }), 'shop.example.org');
  assert.equal(certificateOf({ how: 'letsencrypt', names: [`${'a'.repeat(70)}.org`], short: 'shop' }), 'shop');
  assert.equal(certificateOf({ how: 'none', names }), null);
});

// ---- what the page sends ----

test('the arguments of the plan and the add leave out what is as it would be, and name the certificate only when it is here already', () => {
  assert.deepEqual(siteArgs(form()), { name: 'shop.example.org', kind: 'php', owner: 'webdev', names: 'www.shop.example.org' });
  assert.deepEqual(siteArgs(form({ names: ['a.example'], folder: '/shared/www/sites/other', how: 'machine', certificate: 'this-machine', kind: 'static' })), {
    name: 'a.example', kind: 'static', owner: 'webdev', folder: '/shared/www/sites/other', certificate: 'this-machine',
  });
  assert.equal(siteArgs(form({ how: 'there', certificate: 'example.org' })).certificate, 'example.org');
  assert.equal('certificate' in siteArgs(form({ how: 'none' })), false);
  assert.equal('names' in siteArgs(form({ names: ['a.example'] })), false);
  assert.equal(siteArgs(form({ names: ['a', 'b', 'c'], folder: folderFor('a') })).names, 'b\nc');
});

test('the arguments of the order agree to the terms and name the email only when there is one', () => {
  assert.deepEqual(getArgs(form()), { name: 'shop.example.org', names: 'shop.example.org\nwww.shop.example.org', agree_terms: true });
  assert.equal(getArgs(form({ email: ' me@example.org ' })).email, 'me@example.org');
  assert.deepEqual(openPortArgs(443), { action: 'allow', protocol: 'tcp', ports: '443', try: 600 });
  assert.equal(openPortArgs(80, 30).try, 30);
  assert.deepEqual(httpsArgs('a', 'c'), { name: 'a', certificate: 'c' });
  assert.deepEqual(httpsArgs('a', null), { name: 'a', off: true });
  assert.deepEqual(removeArgs('a', false), { name: 'a' });
  assert.deepEqual(removeArgs('a', true), { name: 'a', files: true });
  assert.deepEqual(testArgs(443), { protocol: 'tcp', port: 443, from: '203.0.113.1' });
});

// ---- the steps ----

test('the steps of an add: the apps first, the site, then for Let\'s Encrypt the certificate and HTTPS', () => {
  const apps = [{ id: 'org.nginx.nginx', name: 'nginx', sha256: 'a'.repeat(64) }, { id: 'net.php.php', name: 'PHP-FPM', sha256: 'b'.repeat(64) }];
  const steps = stepsOf(form(), apps);
  assert.deepEqual(steps.map((one) => [one.id, one.action]), [
    ['install', 'apps.install'], ['install', 'apps.install'], ['add', 'websites.add'], ['get', 'certificate.get'], ['https', 'websites.https'],
  ]);
  assert.deepEqual(steps[0].args, { id: 'org.nginx.nginx', sha256: 'a'.repeat(64) });
  assert.deepEqual(steps[3].args, getArgs(form()));
  assert.deepEqual(steps[4].args, { name: 'shop.example.org', certificate: 'shop.example.org' });
  assert.ok(steps.every((one) => one.wait === 'task' && one.title && one.words.stands && one.words.unlock && one.words.running && one.words.failed));
  assert.deepEqual(stepsOf(form({ how: 'machine', certificate: 'this-machine' })).map((one) => one.id), ['add']);
  assert.deepEqual(stepsOf(form({ how: 'none' })).map((one) => one.id), ['add']);
  assert.deepEqual(stepsOf(form({ how: 'there', certificate: 'example.org' })).map((one) => one.id), ['add']);
  assert.deepEqual(certificateSteps(form()).map((one) => one.id), ['get', 'https']);
});

test('what stands after each step fails is said in the step\'s words', () => {
  const steps = stepsOf(form(), [{ id: 'org.nginx.nginx', name: 'nginx', sha256: 'a'.repeat(64) }]);
  const [install, add, get, https] = steps;
  assert.equal(install.words.stands, 'Nothing about the website was changed.');
  assert.equal(add.words.stands, 'Nothing was changed: the command took its changes back.');
  assert.equal(get.words.stands, 'shop.example.org is live over HTTP. The certificate was not got: the site\'s page has Get the certificate to try again.');
  assert.equal(https.words.stands, 'shop.example.org is live over HTTP, and certd keeps the certificate shop.example.org. Try again turns HTTPS on.');
  assert.equal(STANDS.https('a', 'b'), 'a is live over HTTP, and certd keeps the certificate b. Try again turns HTTPS on.');
  assert.equal(installStep({ id: 'x', name: 'nginx', sha256: 's' }).words.failed, 'nginx was not installed');
  assert.equal(addStep(form()).words.failed, 'shop.example.org was not added');
  assert.equal(getStep(form()).words.unlock, 'get a certificate for shop.example.org');
  assert.equal(httpsStep('a.example', 'c').words.running, 'Turning on HTTPS for a.example');
});

test('the end of a run: done, left, and each failure with where Try again starts', () => {
  const steps = stepsOf(form(), [{ id: 'org.nginx.nginx', name: 'nginx', sha256: 's' }]);
  assert.deepEqual(outcomeOf({ done: true, failed: null, answers: [], left: false }, steps), {
    ok: true, words: '', again: null, live: true,
  });
  assert.deepEqual(outcomeOf({ done: false, failed: null, answers: [], left: true }, steps), {
    ok: false, words: '', again: null, live: false,
  });
  const failed = (at) => outcomeOf({ done: false, failed: at, answers: [], left: false }, steps);
  assert.deepEqual([failed(0).again, failed(0).live], [0, false], 'the install is tried again from itself');
  assert.deepEqual([failed(1).again, failed(1).live], [1, false], 'the add is tried again, the app being installed');
  assert.deepEqual([failed(2).again, failed(2).live, failed(2).words], [null, true, steps[2].words.stands], 'the certificate is got again from the site\'s page');
  assert.deepEqual([failed(3).again, failed(3).live], [3, true], 'HTTPS is tried again from itself');
});

test('the address of a site that was added', () => {
  assert.equal(liveUrl('a.example', 'none'), 'http://a.example/');
  for (const how of ['letsencrypt', 'machine', 'there']) assert.equal(liveUrl('a.example', how), 'https://a.example/');
});

test('the apps a kind needs that are not installed', () => {
  const answer = { nginx: { installed: false }, php_fpm: { installed: false } };
  assert.deepEqual(missingApps(answer, 'static'), [{ id: 'org.nginx.nginx', name: 'nginx' }]);
  assert.deepEqual(missingApps(answer, 'php').map((one) => one.name), ['nginx', 'PHP-FPM']);
  assert.deepEqual(missingApps({ nginx: { installed: true }, php_fpm: { installed: true } }, 'php'), []);
  assert.deepEqual(missingApps(null, 'php'), []);
  assert.deepEqual(KINDS.map((one) => one.id), ['static', 'php']);
});

// ---- what happens ----

const world = {
  backups: { places: [{ name: 'nas', every: 'day', places: ['/users', '/shared'] }] },
  snapshots: { settings: { places: ['/users', '/shared'] } },
};

test('the rows for a PHP site with Let\'s Encrypt, before the plan is read', () => {
  const rows = planRows(form(), null, { ...world, firewall: [{ port: 80, allowed: true }, { port: 443, allowed: true }] });
  assert.deepEqual(rows.map((one) => one.key), ['certificate', 'folder', 'site', 'restart', 'firewall', 'backups', 'snapshots']);
  assert.equal(rows[0].words, 'Gets a certificate for shop.example.org and www.shop.example.org from Let\'s Encrypt; certd renews it by itself.');
  assert.equal(rows[1].words, 'Makes /shared/www/sites/shop.example.org if it is not there with a starter page, owned by webdev.');
  assert.match(rows[2].words, /^Adds nginx's site for both names, then turns HTTPS on once the certificate is got/);
  assert.match(rows[3].words, /^Restarts nginx once, to give it the certificate and port 443: Spark holds its ports, so no visitor is turned away\.$/);
  assert.equal(rows[4].words, 'Nothing to change: ports 80 and 443 reach nginx (no rule closes them)');
  assert.equal(rows[5].words, 'Nothing to change: /shared is in the nightly backup to nas');
  assert.equal(rows[6].words, 'Snapshots: /shared has a snapshot every day');
});

test('the rows follow the plan: a folder that is there, what it turns on, the rule that closes a port', () => {
  const plan = {
    folder: { path: '/shared/www/sites/shop.example.org', exists: true, entries: 14, owner: 'sam' },
    restarts_nginx: true,
    turns_on: ['nginx', 'php-fpm'],
    firewall: [{ port: 80, allowed: true, rule: null }, { port: 443, allowed: false, rule: 3, text: 'deny tcp 443' }],
  };
  const rows = planRows(form({ how: 'machine', certificate: 'this-machine' }), plan, { install: [{ name: 'nginx' }], ...world });
  assert.deepEqual(rows.map((one) => one.key), ['install-nginx', 'certificate', 'folder', 'site', 'restart', 'on-nginx', 'on-php-fpm', 'firewall', 'port-443', 'backups', 'snapshots']);
  const byKey = Object.fromEntries(rows.map((one) => [one.key, one]));
  assert.equal(byKey['install-nginx'].words, 'Installs nginx first, from the catalog.');
  assert.equal(byKey.certificate.words, 'Serves it with this machine\'s own certificate. Browsers warn until they trust it.');
  assert.equal(byKey.folder.words, 'Keeps /shared/www/sites/shop.example.org as it is, owned by sam.');
  assert.equal(byKey.site.words, 'Adds nginx\'s site for both names, HTTPS only: http:// sends visitors to https://.');
  assert.equal(byKey['on-php-fpm'].words, 'Turns PHP-FPM on.');
  assert.equal(byKey['on-nginx'].words, 'Turns nginx on.');
  assert.equal(byKey['port-443'].words, 'Rule 3 closes port 443: deny tcp 443');
  assert.equal(byKey['port-443'].port, 443);
  assert.equal(byKey['port-443'].tone, 'warn');
  assert.equal(byKey.firewall.words, 'Nothing to change: port 80 reaches nginx (no rule closes it)');
  assert.equal(byKey.restart.words.startsWith('Restarts nginx once'), true);
});

test('a site over HTTP only says so in the warning colour and restarts nothing', () => {
  const rows = planRows(form({ how: 'none', names: ['a.example'] }), { restarts_nginx: false, firewall: [] }, {});
  assert.deepEqual(rows.map((one) => one.key), ['certificate', 'folder', 'site']);
  assert.equal(rows[0].tone, 'warn');
  assert.equal(rows[0].words, 'Serves it over HTTP only: pages and passwords travel unencrypted.');
  assert.equal(rows[2].words, 'Adds nginx\'s site for the name, over HTTP.');
  const there = planRows(form({ how: 'there', certificate: 'example.org' }), null, {});
  assert.equal(there[0].words, 'Serves it with the certificate example.org that is here.');
});

test('the firewall rows: one that all is well, one for each port a rule closes, each port once', () => {
  assert.deepEqual(firewallRows([]), []);
  assert.deepEqual(firewallRows(null), []);
  const rows = firewallRows([{ port: 80, allowed: false, rule: 2, text: 'deny tcp 80' }, { port: 80, allowed: true }, { port: 443, allowed: true }]);
  assert.deepEqual(rows.map((one) => [one.key, one.port]), [['firewall', null], ['port-80', 80]]);
  assert.equal(rows[0].words, 'Nothing to change: port 443 reaches nginx (no rule closes it)');
  assert.equal(firewallRows([{ port: 80, allowed: false, rule: null }])[0].words, 'The firewall does not let port 80 in');
});

test('the backup cover: /shared, /shared/www, none, and not read', () => {
  const rowOf = (places, every = 'day') => backupRow({ places: [{ name: 'nas', every, places }] });
  assert.equal(rowOf(['/users', '/shared']).tone, 'ok');
  assert.equal(rowOf(['/shared/www']).words, 'Nothing to change: /shared is in the nightly backup to nas');
  assert.equal(rowOf(['/shared'], 'week').words, 'Nothing to change: /shared is in the weekly backup to nas');
  assert.equal(rowOf(['/shared'], 'hand').words, 'Nothing to change: /shared is in the backup to nas, which is made by hand');
  const none = rowOf(['/users', '/system/settings']);
  assert.deepEqual([none.tone, none.words, none.link], ['warn', 'No backup place copies /shared yet', { href: '/snapshots/backups', words: 'Add a backup place' }]);
  assert.equal(backupRow({ places: [] }).tone, 'warn');
  assert.equal(backupRow(null), null);
  assert.equal(backupRow({}), null);
});

test('the snapshot row: snap.toml names /shared or not, and not read', () => {
  assert.equal(snapshotRow({ settings: { places: ['/users'] } }).tone, 'warn');
  assert.match(snapshotRow({ settings: { places: ['/users'] } }).words, /does not name \/shared/);
  assert.equal(snapshotRow({ places: [{ place: '/shared' }] }).tone, 'ok');
  assert.equal(snapshotRow(null), null);
  assert.equal(snapshotRow({}), null);
});

test('the plan\'s checks that stop the add leave out an app that is not installed', () => {
  const plan = {
    checks: [
      { id: 'WEB-101', result: 'failed', words: 'taken' },
      { id: 'WEB-105', result: 'failed', words: 'nginx is not installed' },
      { id: null, result: 'passed', words: 'ok' },
      { id: 'WEB-103', result: 'warning', words: 'later' },
    ],
  };
  const { blocking, warnings } = planChecks(plan);
  assert.deepEqual(blocking.map((one) => one.id), ['WEB-101']);
  assert.deepEqual(warnings.map((one) => one.id), ['WEB-103']);
  assert.deepEqual(planChecks(null), { blocking: [], warnings: [] });
});

test('the ports a plan asks about, and the files of the plan with their lines numbered', () => {
  assert.deepEqual(portsFor('none'), [80]);
  assert.deepEqual(portsFor('letsencrypt'), [80, 443]);
  const files = previewFiles({ files: [{ path: '/a', new: true, text: 'one\ntwo\n' }, { path: '/b', new: false, text: '' }, { nothing: true }] });
  assert.deepEqual(files, [
    { path: '/a', state: 'new', lines: [{ number: 1, text: 'one' }, { number: 2, text: 'two' }] },
    { path: '/b', state: 'changes', lines: [{ number: 1, text: '' }] },
  ]);
  assert.deepEqual(previewFiles(null), []);
});

// ---- the list and a site ----

const site = (more = {}) => ({
  name: 'shop.example.org', names: ['shop.example.org', 'www.shop.example.org'], file: '/system/settings/websites/shop.example.org.conf', kind: 'php', folder: '/shared/www/sites/shop.example.org', owner: 'webdev', https: true, certificate: 'shop.example.org', redirects_http: true, by_hand: false, added_by_hand: false, problems: [], ...more,
});

test('what a site serves', () => {
  assert.equal(serveWords(site()), 'PHP pages from /shared/www/sites/shop.example.org, owned by webdev');
  assert.equal(serveWords(site({ kind: 'static', owner: null })), 'Files from /shared/www/sites/shop.example.org');
  assert.equal(serveWords(site({ kind: 'other', folder: null })), 'Set up by hand');
});

test('the state of a site: nginx first, then its own problem, then PHP-FPM', () => {
  const up = { nginx: { installed: true, running: true }, phpFpm: { installed: true, running: true } };
  assert.deepEqual(stateOf(site(), up), { tone: 'ok', words: 'Answers', id: null });
  assert.equal(stateOf(site(), { ...up, nginx: { installed: false } }).words, 'nginx is not installed');
  assert.equal(stateOf(site(), { ...up, nginx: { installed: true, running: false } }).words, 'nginx is off');
  assert.deepEqual(stateOf(site({ problems: [{ id: null, words: 'Its folder /x is not there' }] }), up), { tone: 'warn', words: 'Its folder /x is not there', id: null });
  assert.equal(stateOf(site({ problems: [{ id: 'WEB-103', words: 'bad' }] }), up).id, 'WEB-103');
  assert.equal(stateOf(site(), { ...up, phpFpm: { installed: true, running: false } }).words, 'PHP-FPM is off: its PHP pages fail');
  assert.equal(stateOf(site(), { ...up, phpFpm: { installed: false } }).words, 'PHP-FPM is not installed: its PHP pages fail');
  assert.equal(stateOf(site({ kind: 'static' }), { ...up, phpFpm: { installed: true, running: false } }).tone, 'ok', 'a site without PHP does not mind');
  assert.equal(stateOf(site()).tone, 'ok');
});

test('a site\'s HTTPS in the list\'s words', () => {
  assert.deepEqual(httpsOf(site({ https: false, certificate: null }), list, date), { words: 'HTTP only', tone: 'warn', certificate: null, ends: null, renews: null });
  assert.equal(httpsOf(site({ certificate: null }), list, date).tone, 'warn');
  assert.equal(httpsOf(site({ certificate: 'gone' }), list, date).words, 'Certificate gone, which certd does not have');
  assert.deepEqual(httpsOf(site(), list, date), {
    words: 'Let\'s Encrypt · renews 2026-12-30', tone: '', certificate: 'shop.example.org', ends: '2027-01-28', renews: '2026-12-30',
  });
  assert.equal(httpsOf(site({ certificate: 'example.org' }), [{ ...wild, not_after: '2027-02-01T00:00:00Z' }], date).words, 'Let\'s Encrypt · ends 2027-02-01');
  assert.equal(httpsOf(site({ certificate: 'this-machine' }), list, date).words, 'This machine\'s certificate');
  assert.equal(httpsOf(site({ certificate: 'intranet' }), list, date).words, 'Certificate intranet (added)');
  const gone = httpsOf(site({ certificate: 'blog.example.org' }), list, date);
  assert.deepEqual([gone.tone, gone.words], ['warn', 'Let\'s Encrypt · blog.example.org is revoked']);
  assert.equal(httpsOf(site(), null, date).words, 'Certificate shop.example.org, which certd does not have');
});

test('a hand edit is shown, and the console says it does not write that file', () => {
  assert.deepEqual(handEditWords(site()), { tone: '', by: null, words: 'As website wrote it' });
  assert.deepEqual(handEditWords(site({ by_hand: true })), { tone: 'warn', by: 'changed', words: 'Changed by hand · kept as you wrote it' });
  assert.equal(handEditWords(site({ added_by_hand: true, by_hand: true })).by, 'added');
  assert.equal(httpsLocked(site()), '');
  assert.match(httpsLocked(site({ by_hand: true })), /changed by hand/);
  assert.match(httpsLocked(site({ added_by_hand: true })), /changed by hand/);
  assert.match(httpsLocked(site({ kind: 'other' })), /made by hand/);
});

test('removing a site, or taking its HTTPS away, restarts nginx only when no other site uses its certificate', () => {
  const a = site();
  const b = site({ name: 'blog.example.org', certificate: 'shop.example.org' });
  const c = site({ name: 'other.example.org', certificate: 'other' });
  const http = site({ name: 'plain.example.org', https: false, certificate: null });
  assert.equal(lastToUse(a, [a]), true);
  assert.equal(lastToUse(a, [a, b]), false);
  assert.equal(lastToUse(a, [a, c]), true, 'another certificate does not keep this one in nginx\'s service file');
  assert.equal(lastToUse(a, [a, http]), true);
  assert.equal(lastToUse(http, [a, http]), false, 'a site without HTTPS has nothing to lose');
  assert.match(removeLead(a, [a])[0], /^Removes nginx's site for shop\.example\.org\. Its folder and certificate are kept\.$/);
  assert.match(removeLead(a, [a])[1], /^nginx restarts once, since no other site uses this certificate/);
  assert.match(removeLead(a, [a, b])[1], /^nginx reads its settings again/);
  assert.equal(removeLead(http, [http])[0], 'Removes nginx\'s site for shop.example.org. Its folder is kept.');
  assert.match(offLead(a, [a])[0], /^shop\.example\.org is served over HTTP only afterwards, and pages and passwords travel unencrypted\. The certificate stays in certd\.$/);
  assert.match(offLead(a, [a, b])[1], /no visitor is turned away/);
});

test('the lines above the list say what does not run, and where to turn it on or add it', () => {
  const sites = [site(), site({ kind: 'static', name: 'b' })];
  assert.deepEqual(serverLines({ nginx: { installed: true, running: true }, php_fpm: { installed: true, running: true } }, sites), []);
  assert.deepEqual(serverLines({ nginx: { installed: true, running: false }, php_fpm: { installed: true, running: true } }, sites), [
    { key: 'nginx', words: 'nginx is off: no site answers.', enable: 'nginx', sheet: null },
  ]);
  assert.deepEqual(serverLines({ nginx: { installed: false }, php_fpm: { installed: false } }, sites).map((one) => [one.key, one.sheet]), [
    ['nginx', '/apps/add/org.nginx.nginx'], ['php-fpm', '/apps/add/net.php.php'],
  ]);
  assert.deepEqual(serverLines({ nginx: { installed: true, running: true }, php_fpm: { installed: true, running: false } }, sites).map((one) => [one.key, one.enable]), [['php-fpm', 'php-fpm']]);
  assert.deepEqual(serverLines({ nginx: { installed: true, running: true }, php_fpm: { installed: false } }, [sites[1]]), [], 'PHP-FPM is not asked for when no site needs it');
  assert.deepEqual(serverLines(null, []), []);
});

// ---- Check the sites ----

test('nginx\'s check and its settings: the lines that are missing and where they go', () => {
  const ok = { nginx_conf: { path: '/x/nginx.conf', includes: true, hides_sites: true }, check: { passed: true, line: null }, websites: [] };
  assert.deepEqual(confRows(ok), []);
  assert.deepEqual(checkWords(ok), { passed: true, tone: 'ok', words: 'nginx\'s check of the sites passed.', line: null });
  const broken = { nginx_conf: { path: '/x/nginx.conf', includes: false, hides_sites: false }, check: { passed: false, line: 'nginx: [emerg] unknown directive "x" in /x/a.conf:3' }, websites: [{ name: 'a', problems: [{ id: 'WEB-103', words: 'bad' }] }, { name: 'b' }] };
  const rows = confRows(broken);
  assert.deepEqual(rows.map((one) => [one.key, one.id, one.lines]), [
    ['include', 'WEB-102', ['include /system/settings/websites/*.conf;']],
    ['hides', 'WEB-102', ['location ^~ /sites/ {', '    return 404;', '}']],
  ]);
  assert.match(rows[0].words, /\(\/x\/nginx\.conf\)/);
  assert.deepEqual(confRows({ nginx_conf: { includes: true, hides_sites: false } }).map((one) => one.key), ['hides']);
  assert.deepEqual(checkWords(broken), {
    passed: false, tone: 'bad', words: 'nginx\'s check of the sites failed, so nothing is changed until it passes.', line: 'nginx: [emerg] unknown directive "x" in /x/a.conf:3',
  });
  assert.deepEqual(checkedProblems(broken), { a: [{ id: 'WEB-103', words: 'bad' }], b: [] });
  assert.deepEqual([confRows(null), checkWords(null), checkedProblems(null)], [[], null, {}]);
});
