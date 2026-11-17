// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Databases pages know of a database (lib/database-model.js): the
// name rule and the name a site's database gets, the settings and examples
// of the three ways to connect, which never hold the password, the networks
// other machines connect from, and what each action is sent.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  addArgs, allowArgs, ANYWHERE, connectionSettings, DEFAULT_LOCALE, denyArgs, EARLIER_LEAD, EARLIER_TAIL, fileName, hrefOf, isReserved,
  isSiteName, listensOnNetwork, LOCALES, machineOf, nameFromSite, nameProblem, needsAnywhere, networksProblem, NETWORKS_MAX,
  openedWords, parseNetworks, planRows, portWords, removeArgs, restarts, routeOf, secretOf, settingsFile, wayWords, WAYS,
} from '../lib/database-model.js';
import * as routes from '../lib/database-route.js';

const NAS = machineOf({ format: 1, name: 'nas.example.org', first: 'nas', domain: 'example.org', local: 'nas.local' });
const PASSWORD = 'Zq7-sentinel-password-0123456789ab';

test('a name starts with a letter and has lowercase letters, digits and _, up to 63 characters', () => {
  for (const good of ['shop', 'my_blog', 'a', 'shop_2', 'a1_b2', `a${'b'.repeat(62)}`]) assert.equal(nameProblem(good), '', good);
  for (const bad of ['Shop', '1shop', '_shop', 'my-blog', 'a;b', 'a b', 'shop"', `a${'b'.repeat(63)}`, 'é']) {
    assert.match(nameProblem(bad), /lowercase letters, digits and _/, bad);
  }
  assert.equal(nameProblem(''), 'Give the database a name.');
  assert.equal(nameProblem('', 'role'), 'Give the role a name.');
});

test('PostgreSQL\'s own names are refused', () => {
  for (const kept of ['pg_x', 'pg_', 'postgres', 'template0', 'template1']) {
    assert.match(nameProblem(kept), /PostgreSQL keeps/, kept);
    assert.equal(isReserved(kept), true);
  }
  assert.equal(nameProblem('postgres_2'), '');
  assert.equal(nameProblem('pgx'), '');
});

test('a site is a lowercase DNS name', () => {
  assert.equal(isSiteName('shop.example.org'), true);
  assert.equal(isSiteName('my-blog.example.org'), true);
  assert.equal(isSiteName('shop'), true);
  for (const bad of ['', 'Shop.example.org', 'a..b', '-x.example.org', '*.example.org', 'a b', `${'a'.repeat(64)}.org`, null, 5]) {
    assert.equal(isSiteName(bad), false, String(bad));
  }
  assert.equal(isSiteName(`${'a.'.repeat(126)}a`), true);
  assert.equal(isSiteName(`${'a.'.repeat(127)}a`), false);
});

test('the database of a site is named for its first word, and numbered when that is taken', () => {
  assert.equal(nameFromSite('shop.example.org'), 'shop');
  assert.equal(nameFromSite('my-blog.example.org'), 'my_blog');
  assert.equal(nameFromSite('shop.example.org', ['shop']), 'shop_2');
  assert.equal(nameFromSite('shop.example.org', ['shop', 'shop_2', 'blog']), 'shop_3');
  // A role of the name is as much in the way as a database.
  assert.equal(nameFromSite('shop.example.org', new Set(['shop'])), 'shop_2');
  assert.equal(nameFromSite('shop', []), 'shop');
});

test('a first word the rule would refuse is made into one that it allows', () => {
  assert.equal(nameFromSite('3d.example.org'), 'db_3d');
  assert.equal(nameFromSite('pg-admin.example.org'), 'db_pg_admin');
  assert.equal(nameFromSite('postgres.example.org'), 'postgres_2');
  assert.equal(nameFromSite('template0.example.org'), 'template0_2');
  const long = 'a'.repeat(63);
  assert.equal(nameFromSite(`${long}.org`), long);
  const second = nameFromSite(`${long}.org`, [long]);
  assert.equal(second, `${'a'.repeat(61)}_2`);
  assert.equal(second.length, 63);
  assert.equal(nameFromSite(`db-${'b'.repeat(60)}.org`).length, 63);
  for (const site of ['3d.example.org', 'pg-admin.example.org', 'postgres.example.org', 'my-blog.example.org', `${long}.org`]) {
    assert.equal(nameProblem(nameFromSite(site)), '', site);
  }
});

test('a name that is not a site gives no name', () => {
  for (const bad of ['', 'Shop.example.org', '*.example.org', null, undefined]) assert.equal(nameFromSite(bad), '');
});

test('each database has a page, and one called add is written so that it is not Add a database', () => {
  assert.equal(hrefOf('shop'), '/databases/shop');
  assert.equal(hrefOf('add'), '/databases/%61dd');
  assert.deepEqual(routeOf(''), { kind: 'list' });
  assert.deepEqual(routeOf(undefined), { kind: 'list' });
  assert.deepEqual(routeOf('add'), { kind: 'add' });
  assert.deepEqual(routeOf('shop'), { kind: 'database', name: 'shop' });
  assert.deepEqual(routeOf('shop/'), { kind: 'database', name: 'shop' });
  assert.deepEqual(routeOf('%61dd'), { kind: 'database', name: 'add' });
  assert.deepEqual(routeOf('%E0%A4%A'), { kind: 'database', name: '%E0%A4%A' });
  assert.equal(routeOf(hrefOf('add').replace('/databases/', '')).name, 'add');
});

test('the three ways to connect say what they do in the role\'s name', () => {
  assert.deepEqual(WAYS.map((way) => way.id), ['local', 'socket', 'network']);
  assert.match(wayWords('local', 'shop'), /127\.0\.0\.1, port 5432, as shop, with a long password made now and shown once/);
  assert.match(wayWords('socket', 'webdev'), /named for the account and signs in as it, with no password/);
  assert.match(wayWords('network', 'shop'), /sign in as shop, with the password, over TLS/);
  assert.match(wayWords('network', 'shop'), /restarts once/);
});

test('networks are read a line at a time, and written as PostgreSQL writes them', () => {
  const found = parseNetworks('192.168.1.0/24\n  2001:db8::/64  \n\n10.0.0.0/8\n192.168.1.0/24');
  assert.deepEqual(found.networks, ['192.168.1.0/24', '2001:db8::/64', '10.0.0.0/8']);
  assert.deepEqual(found.problems, []);
  assert.equal(found.anywhere, false);
  assert.equal(found.tooMany, false);
  assert.deepEqual(parseNetworks('').networks, []);
  assert.deepEqual(parseNetworks(undefined).networks, []);
});

test('a line that is not an address with its prefix length is a problem on its line', () => {
  const found = parseNetworks('192.168.1.0/24\n10.0.2.0\nanywhere\n10.0.0.0/33\n192.168.1.5/24\n2001:db8::/129');
  assert.deepEqual(found.networks, ['192.168.1.0/24']);
  assert.deepEqual(found.problems.map((problem) => [problem.line, problem.text]), [
    [2, '10.0.2.0'], [3, 'anywhere'], [4, '10.0.0.0/33'], [5, '192.168.1.5/24'], [6, '2001:db8::/129'],
  ]);
  assert.match(found.problems[0].why, /not an address with its prefix length, such as 192\.168\.1\.0\/24/);
  assert.equal(found.problems[3].why, '192.168.1.5/24 has bits set after its prefix. Write 192.168.1.0/24.');
});

test('every address is a network of no prefix, and says so', () => {
  assert.equal(parseNetworks('0.0.0.0/0').anywhere, true);
  assert.equal(parseNetworks('::/0').anywhere, true);
  assert.equal(parseNetworks('10.0.0.0/8\n0.0.0.0/0').anywhere, true);
  assert.equal(parseNetworks('10.0.0.0/8').anywhere, false);
});

test('at most 20 networks', () => {
  const some = (count) => Array.from({ length: count }, (_, at) => `10.${at}.0.0/16`).join('\n');
  assert.equal(parseNetworks(some(20)).tooMany, false);
  assert.equal(parseNetworks(some(21)).tooMany, true);
  // The same network twice is one.
  assert.equal(parseNetworks(`${some(20)}\n10.0.0.0/16`).tooMany, false);
});

test('the machine is reached by its full name, or by the name the local network answers to', () => {
  assert.deepEqual(NAS, { name: 'nas.example.org', first: 'nas', host: 'nas.example.org' });
  assert.deepEqual(machineOf({ name: 'web01', first: 'web01', domain: null, local: 'web01.local' }), { name: 'web01', first: 'web01', host: 'web01.local' });
  assert.deepEqual(machineOf({ name: 'web01' }), { name: 'web01', first: 'web01', host: 'web01' });
  assert.equal(machineOf({}), null);
  assert.equal(machineOf(null), null);
});

test('programs on this server connect to 127.0.0.1 as the role, with the password', () => {
  const found = connectionSettings({ way: 'local', database: 'shop', role: 'shop' });
  assert.deepEqual(found.rows, [
    { label: 'Host', value: '127.0.0.1' },
    { label: 'Port', value: '5432' },
    { label: 'Database', value: 'shop' },
    { label: 'User', value: 'shop' },
    { label: 'Password', value: 'The one shown once' },
    { label: 'TLS', value: 'Not needed' },
  ]);
  assert.deepEqual(found.examples, [
    { label: 'PHP', text: 'new PDO(\'pgsql:host=127.0.0.1;port=5432;dbname=shop\', \'shop\', $password);' },
    { label: 'psql', text: 'psql "host=127.0.0.1 dbname=shop user=shop"' },
    { label: 'A URL', text: 'postgresql://shop@127.0.0.1:5432/shop' },
  ]);
});

test('an account of this server connects by the socket, as itself, with no password', () => {
  const found = connectionSettings({ way: 'socket', database: 'shop', role: 'webdev' });
  assert.deepEqual(found.rows.map((row) => [row.label, row.value]), [
    ['Host', '/system/live/services/postgres'],
    ['Port', '5432'],
    ['Database', 'shop'],
    ['User', 'webdev'],
    ['Password', 'None: PostgreSQL knows the account'],
    ['TLS', 'Not needed'],
  ]);
  assert.equal(found.examples[0].text, 'psql -d shop');
  assert.match(found.examples[1].text, /host=\/system\/live\/services\/postgres dbname=shop user=webdev/);
  assert.equal(found.examples.some((example) => example.label === 'PHP'), false);
});

test('other machines connect to the machine\'s name over TLS, checking its certificate', () => {
  const found = connectionSettings({
    way: 'network', database: 'shop', role: 'shop', machine: NAS,
  });
  assert.equal(found.rows[0].value, 'nas.example.org');
  assert.equal(found.rows[5].value, 'Required: sslmode=verify-full, with this server\'s certificate');
  assert.equal(found.rows[4].value, 'The one shown once');
  assert.deepEqual(found.examples, [
    { label: 'PHP', text: 'new PDO(\'pgsql:host=nas.example.org;port=5432;dbname=shop;sslmode=verify-full;sslrootcert=nas.pem\', \'shop\', $password);' },
    { label: 'psql', text: 'psql "host=nas.example.org dbname=shop user=shop sslmode=verify-full sslrootcert=nas.pem"' },
    { label: 'A URL', text: 'postgresql://shop@nas.example.org:5432/shop?sslmode=verify-full&sslrootcert=nas.pem' },
  ]);
});

test('no setting and no example holds the password, whatever the page knows of it', () => {
  for (const way of ['local', 'socket', 'network']) {
    const found = connectionSettings({
      way, database: 'shop', role: 'shop', machine: NAS, password: PASSWORD, secret: PASSWORD,
    });
    const text = JSON.stringify(found);
    assert.equal(text.includes(PASSWORD), false, way);
    assert.equal(/password=/i.test(text), false, way);
  }
  // The one place a password goes is the text that is shown once and saved.
  const file = settingsFile({
    way: 'local', database: 'shop', role: 'shop', password: PASSWORD,
  });
  assert.match(file, new RegExp(`^Password: ${PASSWORD}$`, 'm'));
});

test('the file that is saved has the settings and the password, and the certificate\'s name for other machines', () => {
  const file = settingsFile({
    way: 'network', database: 'shop', role: 'shop', password: PASSWORD, machine: NAS,
  });
  assert.equal(file, [
    'PostgreSQL database shop',
    '',
    'Host: nas.example.org',
    'Port: 5432',
    'Database: shop',
    'User: shop',
    `Password: ${PASSWORD}`,
    'TLS: Required: sslmode=verify-full, with this server\'s certificate',
    'Root certificate: nas.pem',
    '',
    'The password is shown only once. Keep this file where only you can read it.',
    '',
  ].join('\n'));
  assert.equal(fileName('shop'), 'shop-postgresql.txt');
  const socket = settingsFile({
    way: 'socket', database: 'shop', role: 'webdev', password: null,
  });
  assert.match(socket, /^Password: None: PostgreSQL knows the account$/m);
  assert.equal(socket.includes('Root certificate'), false);
});

test('the answer that holds a password is the output line of the command\'s JSON', () => {
  const line = JSON.stringify({
    format: 1, database: 'shop', role: 'shop', password: PASSWORD, host: '127.0.0.1', port: 5432,
  });
  assert.deepEqual(secretOf({ done: true, changed: true, output: ['database: added shop (owner shop, en_US.UTF-8)', line] }), {
    database: 'shop', role: 'shop', password: PASSWORD, host: '127.0.0.1', port: 5432,
  });
  const peer = JSON.stringify({
    format: 1, database: 'shop', role: 'webdev', password: null, host: null, port: 5432,
  });
  assert.deepEqual(secretOf({ output: [peer] }), {
    database: 'shop', role: 'webdev', password: null, host: null, port: 5432,
  });
  assert.equal(secretOf({ output: [line.replace('"port":5432', '"port":"x"')] }).port, 5432);
});

test('an answer with no such line gives nothing, so that an empty password is never shown', () => {
  for (const answer of [null, {}, { output: [] }, { output: 'x' }, { output: ['not json', '{"a":1}', '[1]', '{"database":"shop"}'] }, { output: [JSON.stringify({ database: 'shop', role: 'shop' })] }]) {
    assert.equal(secretOf(answer), null);
  }
  assert.equal(secretOf({ output: [JSON.stringify({ database: 'shop', role: 'shop', password: '' })] }).password, null);
});

test('adding sends the owner only when it is not the name, and a socket sends the account', () => {
  assert.deepEqual(addArgs({ name: 'shop', owner: 'shop', locale: 'en_US.UTF-8', way: 'local' }), { name: 'shop', locale: 'en_US.UTF-8' });
  assert.deepEqual(addArgs({
    name: 'shop', owner: 'shop_owner', locale: 'C.UTF-8', site: 'shop.example.org', way: 'network',
  }), {
    name: 'shop', owner: 'shop_owner', locale: 'C.UTF-8', site: 'shop.example.org',
  });
  assert.deepEqual(addArgs({
    name: 'shop', owner: 'webdev', account: 'webdev', locale: 'en_US.UTF-8', way: 'socket',
  }), { name: 'shop', peer: 'webdev', locale: 'en_US.UTF-8' });
});

test('allowing, denying and removing send what the command takes', () => {
  assert.deepEqual(allowArgs('shop', ['192.168.1.0/24', '10.0.0.0/8'], 'this-machine'), { name: 'shop', from: '192.168.1.0/24\n10.0.0.0/8', certificate: 'this-machine' });
  assert.deepEqual(denyArgs('shop'), { name: 'shop' });
  assert.deepEqual(denyArgs('shop', '10.0.0.0/8'), { name: 'shop', from: '10.0.0.0/8' });
  assert.deepEqual(removeArgs('shop'), { name: 'shop' });
  assert.deepEqual(removeArgs('shop', { role: true }), { name: 'shop', role: true });
  assert.deepEqual(removeArgs('shop', { role: true, force: true }), { name: 'shop', role: true, force: true });
  assert.deepEqual(removeArgs('shop', { role: false, force: false }), { name: 'shop' });
});

test('PostgreSQL restarts to listen on the network unless it already does, with the same certificate', () => {
  const local = { listen: 'localhost', ssl: false, certificate: null };
  const open = { listen: '*', ssl: true, certificate: 'this-machine' };
  assert.equal(listensOnNetwork(local), false);
  assert.equal(listensOnNetwork(open), true);
  assert.equal(listensOnNetwork(null), false);
  assert.equal(restarts(local, 'this-machine'), true);
  assert.equal(restarts(open, 'this-machine'), false);
  assert.equal(restarts(open, 'blog.example.org'), true);
  assert.equal(restarts({ ...open, ssl: false }, 'this-machine'), true);
  assert.equal(restarts(null, 'this-machine'), true);
});

const form = {
  name: 'shop', owner: 'shop', account: '', way: 'local', locale: 'UTF-8, sorted as English (US)', networks: [], certificate: 'this-machine',
};

test('what happens for a password from this server', () => {
  const rows = planRows(form);
  assert.deepEqual(rows.map((row) => row.words), [
    'Makes the PostgreSQL role shop, which signs in with a password, from this server only',
    'Makes the database shop, owned by shop, UTF-8, sorted as English (US). Other roles may not connect to it.',
    'Firewall: port 5432 stays closed to other machines, since PostgreSQL listens on this machine only.',
    'Restart: none; the database is ready at once.',
  ]);
});

test('what happens for an account by the socket', () => {
  const rows = planRows({
    ...form, way: 'socket', account: 'webdev', owner: 'webdev',
  });
  assert.equal(rows[0].words, 'Makes the PostgreSQL role webdev, which signs in with no password, from the account webdev, by the socket');
  assert.match(rows[1].words, /^Makes the database shop, owned by webdev,/);
});

test('what happens for other machines says that PostgreSQL listens on the network and restarts', () => {
  const rows = planRows({ ...form, way: 'network', networks: ['192.168.1.0/24'] }, { listen: 'localhost', ssl: false, certificate: null });
  assert.equal(rows[0].words, 'Makes the PostgreSQL role shop, which signs in with a password, from 192.168.1.0/24 over TLS');
  assert.equal(rows[2].words, 'Firewall: port 5432 is opened to 192.168.1.0/24 only.');
  assert.equal(rows[3].words, 'Restart: PostgreSQL restarts once, and its open sessions end.');
  const reload = planRows({ ...form, way: 'network', networks: ['192.168.1.0/24'] }, { listen: '*', ssl: true, certificate: 'this-machine' });
  assert.equal(reload[3].words, 'Restart: none; the database is ready at once.');
  const many = planRows({ ...form, way: 'network', networks: ['10.0.0.0/8', '10.1.0.0/16', '10.2.0.0/16', '10.3.0.0/16'] });
  assert.match(many[0].words, /from 4 networks over TLS$/);
  const some = planRows({ ...form, way: 'network', networks: ['10.0.0.0/8', '10.1.0.0/16'] });
  assert.match(some[0].words, /from 10\.0\.0\.0\/8, 10\.1\.0\.0\/16 over TLS$/);
});

test('a database made while PostgreSQL listens on the network says that nothing changes', () => {
  const rows = planRows(form, { listen: '*', ssl: true, certificate: 'this-machine' });
  assert.match(rows[2].words, /^Firewall: nothing to change\./);
});

/** The server of a read that listens on the network, with the firewall's block as `networks` and `earlier` say. */
const away = (networks, earlier = null) => ({
  listen: '*', ssl: true, certificate: 'this-machine', firewall: { networks, earlier },
});

test('what the firewall says of the port is who it is open to, or that it is closed, or that no rule of its own holds it', () => {
  assert.equal(portWords(away(['192.168.1.0/24'])), 'Port 5432 is open to 192.168.1.0/24 only.');
  assert.equal(portWords(away(['192.168.1.0/24', '10.0.0.0/8'])), 'Port 5432 is open to 192.168.1.0/24 and 10.0.0.0/8 only.');
  assert.equal(portWords(away(['192.168.1.0/24', '10.0.0.0/8', '172.16.0.0/12'])), 'Port 5432 is open to 192.168.1.0/24, 10.0.0.0/8 and 172.16.0.0/12 only.');
  assert.equal(portWords({ listen: 'localhost', firewall: { networks: [], earlier: null } }), 'Port 5432 is closed to other machines: PostgreSQL listens on this machine only.');
  // A server that listens on the network before the block was written is not closed.
  assert.match(portWords(away([])), /^Port 5432 has no rule of its own: PostgreSQL listens on the network, so every address reaches it/);
  assert.match(portWords({ listen: '', firewall: { networks: [], earlier: null } }), /closed to other machines/);
});

test('what the firewall will say once networks are let in is the networks it is open to and theirs, each once, and no other', () => {
  assert.equal(openedWords(null, ['192.168.1.0/24']), 'Port 5432 is opened to 192.168.1.0/24 only.');
  assert.equal(openedWords(away(['10.0.0.0/8']), ['192.168.1.0/24', '10.0.0.0/8']), 'Port 5432 is opened to 10.0.0.0/8 and 192.168.1.0/24 only.');
  assert.equal(openedWords(away([]), []), 'Port 5432 is opened to the networks you name only.');
  assert.equal(openedWords({ listen: '*' }, []), 'Port 5432 is opened to the networks you name only.');
});

test('a rule that decides the port first is told in the words around it, and in a row of what happens for other machines', () => {
  assert.equal(`${EARLIER_LEAD}allow tcp 5000-6000${EARLIER_TAIL}`, 'A rule earlier in the firewall\'s rules decides for port 5432 first: allow tcp 5000-6000. Move or remove it on the Firewall page.');
  const asked = { ...form, way: 'network', networks: ['192.168.1.0/24'] };
  const rows = planRows(asked, away([], 'allow tcp 5000-6000'));
  assert.deepEqual(rows.map((row) => row.icon), ['accounts', 'db', 'check', 'warn', 'check']);
  assert.equal(rows[3].words, `${EARLIER_LEAD}allow tcp 5000-6000${EARLIER_TAIL}`);
  // The rule matters to what opens the port, not to a database that does not.
  assert.equal(planRows(form, away(['10.0.0.0/8'], 'allow tcp 5000-6000')).length, 4);
  assert.equal(planRows(asked, away([])).length, 4);
});

test('a database made for programs on this server leaves the port as the firewall has it', () => {
  const open = planRows(form, away(['192.168.1.0/24', '10.0.0.0/8']));
  assert.equal(open[2].words, 'Firewall: nothing to change. Port 5432 stays open to 192.168.1.0/24 and 10.0.0.0/8 only, and only programs on this server may sign in to this database.');
  assert.equal(planRows(form, { listen: 'localhost', firewall: { networks: [], earlier: null } })[2].words, 'Firewall: port 5432 stays closed to other machines, since PostgreSQL listens on this machine only.');
});

test('the text and sorting offered start with the one that is the default, and only the first has a note', () => {
  assert.equal(DEFAULT_LOCALE, 'en_US.UTF-8');
  assert.deepEqual(LOCALES.map((one) => one.value), ['en_US.UTF-8', 'C.UTF-8']);
  assert.match(LOCALES[0].note, /indexes to be rebuilt/);
  assert.equal(LOCALES[1].note, null);
  assert.equal(Object.isFrozen(LOCALES), true);
});

test('the networks are sent only when there is at least one, none is wrong and there are not too many', () => {
  assert.equal(networksProblem(parseNetworks('192.168.1.0/24\n10.0.0.0/8')), '');
  assert.match(networksProblem(parseNetworks('')), /Give at least one network/);
  assert.match(networksProblem(parseNetworks('  \n')), /Give at least one network/);
  assert.match(networksProblem(parseNetworks('10.0.2.0')), /^10\.0\.2\.0 is not an address with its prefix length/);
  const many = Array.from({ length: NETWORKS_MAX + 1 }, (_, at) => `10.${at}.0.0/16`).join('\n');
  assert.equal(networksProblem(parseNetworks(many)), `Give at most ${NETWORKS_MAX} networks.`);
});

test('every address needs the word typed, and nothing else does', () => {
  const everywhere = parseNetworks('0.0.0.0/0');
  assert.equal(needsAnywhere(everywhere, ''), true);
  assert.equal(needsAnywhere(everywhere, 'any'), true);
  assert.equal(needsAnywhere(everywhere, ` ${ANYWHERE} `), false);
  assert.equal(needsAnywhere(parseNetworks('10.0.0.0/8'), ''), false);
});

test('the addresses are in a file of their own, which the model gives again', () => {
  assert.equal(routes.hrefOf, hrefOf);
  assert.equal(routes.routeOf, routeOf);
});
