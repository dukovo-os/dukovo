// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Databases pages say (pages/databases/model.js): the one shape that
// `database list --json` is read into, the server's line, who connects, the
// rows, Remove…'s choices, the backups' words and the choices of Add a
// database.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  accessOf, accountChoices, backupWords, BACKUPS_PATH, certificateChoices, copiedWords, copyOf, covers, denyRestarts, factsOf, handWords,
  leftOutWords, localeWords, problemWords, readOf, reindexLines, removeOf, rowsOf, serverLine, serverState, sessionsWords, waysOf, whoLines,
} from '../pages/databases/model.js';
import {
  accounts, backups, broken, certificates, database, empty, list, missing, off, role, server,
} from './databases-fixtures.mjs';
import {
  at, backupsOf, NOW, place, time,
} from './snapshots-fixtures.mjs';

const read = () => readOf(list());
const shop = () => read().databases.find((one) => one.name === 'shop');

test('the answer is read into one shape, with each list in order', () => {
  const found = read();
  assert.deepEqual(found.databases.map((one) => one.name), ['blog', 'notes', 'shop']);
  assert.deepEqual(found.databases.find((one) => one.name === 'shop'), {
    name: 'shop',
    owner: 'shop',
    encoding: 'UTF8',
    provider: 'libc',
    locale: 'en_US.UTF-8',
    bytes: 38_000_000,
    sessions: 0,
    site: 'shop.example.org',
    madeHere: true,
    mismatch: false,
    copy: { atMs: at(2026, 11, 14, 2), bytes: 12_345_678 },
  });
  assert.deepEqual(found.server, {
    installed: true,
    enabled: true,
    running: true,
    version: '19.0',
    listen: '*',
    ssl: true,
    certificate: 'this-machine',
    firewall: { networks: ['192.168.1.0/24'], earlier: null },
    copies: true,
  });
  assert.deepEqual(found.roles[0], { name: 'shop', login: true, password: true, madeHere: true });
  assert.deepEqual(found.access[0], {
    line: 91, type: 'hostssl', databases: ['shop'], users: ['shop'], address: '192.168.1.0/24', method: 'scram-sha-256', byHand: false,
  });
  assert.deepEqual(found.problems, []);
});

test('what the answer leaves out is read as nothing, and an answer that is not one is none', () => {
  assert.equal(readOf(null), null);
  assert.equal(readOf([]), null);
  assert.equal(readOf('x'), null);
  const bare = readOf({});
  assert.deepEqual(bare.databases, []);
  assert.equal(bare.server.installed, false);
  assert.equal(bare.server.certificate, null);
  assert.deepEqual(bare.server.firewall, { networks: [], earlier: null });
  assert.equal(bare.server.copies, false);
  const odd = readOf({
    server: { installed: true, version: '19', ssl: 'on', certificate: 'blog.example.org' },
    databases: [{ name: 'a' }, { name: '' }, { owner: 'x' }, null, 5],
    roles: [{ name: 'a' }],
    access: [{ databases: 'a, b', users: 'a', by_hand: true }],
    problems: [{ words: 'x', line: 3 }, { id: 'DB-104', words: 'y' }],
  });
  assert.equal(odd.server.ssl, true);
  assert.equal(odd.server.certificate, 'blog.example.org');
  assert.deepEqual(odd.databases, [{
    name: 'a', owner: '', encoding: '', provider: '', locale: '', bytes: null, sessions: 0, site: null, madeHere: false, mismatch: false, copy: null,
  }]);
  assert.deepEqual(odd.roles, [{ name: 'a', login: true, password: false, madeHere: false }]);
  assert.deepEqual(odd.access[0].databases, ['a', 'b']);
  assert.deepEqual(odd.problems, [{ id: null, words: 'x', line: 3 }, { id: 'DB-104', words: 'y', line: null }]);
});

test('the firewall and the copies are read as the command wrote them, and what is not one is nothing', () => {
  const found = readOf(list({
    server: server({ firewall: { networks: ['10.0.0.0/8', 5, null, '192.168.1.0/24'], earlier: 'allow tcp 5000-6000' }, copies: true }),
    databases: [
      database('a', { copy: { at_ms: 5, bytes: 7 } }),
      database('b', { copy: { at_ms: 'x', bytes: 7 } }),
      database('c', { copy: { at_ms: 5 } }),
      database('d', { copy: 'x' }),
    ],
  }));
  assert.deepEqual(found.server.firewall, { networks: ['10.0.0.0/8', '192.168.1.0/24'], earlier: 'allow tcp 5000-6000' });
  assert.deepEqual(found.databases.map((one) => one.copy), [{ atMs: 5, bytes: 7 }, null, { atMs: 5, bytes: 0 }, null]);
  assert.equal(readOf(list({ server: server({ firewall: { networks: 'x', earlier: 5 } }) })).server.firewall.earlier, null);
  assert.deepEqual(readOf(list({ server: server({ firewall: 'x' }) })).server.firewall, { networks: [], earlier: null });
  assert.equal(readOf(list({ server: server({ copies: 'yes' }) })).server.copies, false);
});

test('a copy of today says its time, and one of another day says the day too, with its size', () => {
  const copy = (atMs, bytes) => ({ atMs, bytes });
  assert.equal(copiedWords(copy(at(2026, 11, 14, 2), 12_345_678), time, NOW), 'Copied at 2:00 AM, 12 MB');
  assert.equal(copiedWords(copy(at(2026, 11, 13, 2), 345_678), time, NOW), 'Copied Nov 13 at 2:00 AM, 346 KB');
  assert.equal(copiedWords(copy(at(2025, 12, 31, 14, 30), 8000), time, NOW), 'Copied Dec 31, 2025 at 2:30 PM, 8 KB');
  assert.equal(copiedWords(copy(at(2026, 11, 14, 2), 0), time, NOW), 'Copied at 2:00 AM, 0 B');
});

test('the copy row says when it was made, that none is yet, or that backups make none and where to set them up', () => {
  const found = read();
  const [blog, notes, shopDb] = found.databases;
  assert.deepEqual(copyOf(shopDb, found.server, time, NOW), { words: 'Copied at 2:00 AM, 12 MB', backups: false });
  assert.deepEqual(copyOf(blog, found.server, time, NOW), { words: 'Copied Nov 13 at 2:00 AM, 346 KB', backups: false });
  assert.deepEqual(copyOf(notes, found.server, time, NOW), { words: 'No copy yet: the next backup makes one', backups: false });
  const none = readOf(list({ server: server({ copies: false }) }));
  assert.deepEqual(copyOf(none.databases.find((one) => one.name === 'notes'), none.server, time, NOW), { words: 'No copies are made before backups', backups: true });
  // A copy that was made stays shown when the hook is off now.
  assert.equal(copyOf(none.databases.find((one) => one.name === 'shop'), none.server, time, NOW).backups, false);
  assert.equal(BACKUPS_PATH, '/snapshots/backups');
});

test('the version is its number, and the certificate its name, whichever way the command wrote them', () => {
  assert.equal(readOf({ server: { version: 'PostgreSQL 19.0 on x86_64' } }).server.version, '19.0');
  assert.equal(readOf({ server: { version: '19.1' } }).server.version, '19.1');
  assert.equal(readOf({ server: { version: '' } }).server.version, '');
  assert.equal(readOf({ server: { certificate: '/system/live/certificates/postgresql/blog.example.org/fullchain.pem' } }).server.certificate, 'blog.example.org');
  assert.equal(readOf({ server: { certificate: '/etc/other/server.crt' } }).server.certificate, null);
});

test('the server is missing, off, stopped or on', () => {
  assert.equal(serverState(readOf(missing()).server), 'missing');
  assert.equal(serverState(readOf(off()).server), 'off');
  assert.equal(serverState(readOf(list({ server: server({ running: false }) })).server), 'stopped');
  assert.equal(serverState(read().server), 'on');
});

test('the line says the version, that it runs and where it listens', () => {
  assert.equal(serverLine(readOf(list({ server: server() })).server), 'PostgreSQL 19.0 · running · listens on this machine only');
  assert.equal(serverLine(read().server), 'PostgreSQL 19.0 · running · listens on the network, TLS with this-machine\'s certificate');
  assert.equal(serverLine(readOf(list({ server: server({ version: '', listen: '' }) })).server), 'PostgreSQL · running · listens on this machine only');
  assert.match(serverLine(readOf(list({ server: server({ listen: '10.0.0.5', ssl: true }) })).server), /TLS with this-machine's certificate/);
});

test('who connects to a database, from its role and the lines of pg_hba.conf', () => {
  const found = read();
  const [blog, notes, shopDb] = found.databases;
  const forShop = accessOf(found, shopDb);
  assert.equal(forShop.password, true);
  assert.equal(forShop.peer, null);
  assert.deepEqual(forShop.networks, [{ address: '192.168.1.0/24', line: 91 }]);
  assert.deepEqual(forShop.hand, []);
  assert.deepEqual(waysOf(forShop), ['local', 'network']);
  const forBlog = accessOf(found, blog);
  assert.equal(forBlog.hand.length, 1);
  assert.deepEqual(forBlog.networks, []);
  assert.deepEqual(waysOf(forBlog), ['local']);
  const forNotes = accessOf(found, notes);
  assert.equal(forNotes.peer, 'webdev');
  assert.equal(forNotes.password, false);
  assert.deepEqual(waysOf(forNotes), ['socket']);
});

test('a database whose role is not known has no way, and says so', () => {
  const found = readOf(list({ roles: [], access: [] }));
  const none = accessOf(found, found.databases.find((one) => one.name === 'shop'));
  assert.equal(none.role, null);
  assert.deepEqual(waysOf(none), []);
  assert.deepEqual(whoLines(none), ['No one is known to connect']);
});

test('the lines of Who connects, one each, and the lines written by hand counted last', () => {
  const found = read();
  const [blog, notes, shopDb] = found.databases;
  assert.deepEqual(whoLines(accessOf(found, shopDb)), ['Programs on this server, with the password', '192.168.1.0/24, over TLS with the password']);
  assert.deepEqual(whoLines(accessOf(found, blog)), ['Programs on this server, with the password', 'and 1 line written by hand']);
  assert.deepEqual(whoLines(accessOf(found, notes)), ['webdev, by its account']);
  const two = readOf(list({ access: [{ ...list().access[1] }, { ...list().access[1], line: 41 }] }));
  assert.equal(whoLines(accessOf(two, two.databases[0])).at(-1), 'and 2 lines written by hand');
});

test('a line written by hand is told as it is written', () => {
  const [hand] = accessOf(read(), read().databases[0]).hand;
  assert.equal(handWords(hand), 'host blog blog 10.9.0.0/16 md5');
});

test('the rows of the table: a database, its owner, who connects, its size and its sessions', () => {
  const rows = rowsOf(read());
  assert.deepEqual(rows.map((row) => row.name), ['blog', 'notes', 'shop']);
  assert.deepEqual(rows[2], {
    name: 'shop',
    href: '/databases/shop',
    owner: 'shop',
    site: 'shop.example.org',
    mismatch: false,
    who: ['Programs on this server, with the password', '192.168.1.0/24, over TLS with the password'],
    size: '38 MB',
    sessions: 0,
  });
  assert.equal(rows[0].sessions, 2);
  assert.equal(rows[0].mismatch, true);
  assert.equal(rows[0].size, '1.2 MB');
  assert.equal(rowsOf(readOf(list({ databases: [{ name: 'x' }] })))[0].size, 'Not known');
});

test('a database called add has a page that is not Add a database', () => {
  const found = readOf(list({ databases: [database('add')] }));
  assert.equal(rowsOf(found)[0].href, '/databases/%61dd');
});

test('the text and sorting are told as Add a database words them', () => {
  assert.equal(localeWords(shop()), 'UTF-8, sorted as English (US)');
  assert.equal(localeWords(read().databases.find((one) => one.name === 'notes')), 'UTF-8, sorted by character code, the same on every machine');
  assert.equal(localeWords({ provider: 'icu', locale: 'de-DE', encoding: 'UTF8' }), 'UTF-8, sorted as de-DE');
  assert.equal(localeWords({ provider: '', locale: '', encoding: 'LATIN1' }), 'LATIN1');
  assert.equal(localeWords({ provider: '', locale: '', encoding: '' }), 'Not known');
});

test('the facts of a database, and the words of its sessions', () => {
  assert.deepEqual(factsOf(shop()), [
    { label: 'Owner', value: 'shop' },
    { label: 'Text and sorting', value: 'UTF-8, sorted as English (US)' },
    { label: 'Size', value: '38 MB' },
    { label: 'Sessions', value: '0' },
  ]);
  assert.equal(sessionsWords(0), 'No one is connected now.');
  assert.equal(sessionsWords(1), '1 program is connected now.');
  assert.equal(sessionsWords(2), '2 programs are connected now.');
});

test('the commands that bring a database\'s indexes up to date, and the words of an error PostgreSQL found', () => {
  assert.deepEqual(reindexLines('blog'), [
    'elevate psql -U postgres -d blog -c \'REINDEX DATABASE blog\'',
    'elevate psql -U postgres -d blog -c \'ALTER DATABASE blog REFRESH COLLATION VERSION\'',
  ]);
  const [problem] = readOf(broken()).problems;
  assert.equal(problemWords(problem), 'invalid authentication method "scram" (line 41 of pg_hba.conf)');
  assert.equal(problemWords({ id: null, words: 'bad', line: null }), 'bad');
});

test('Remove… offers the role when the page made it, ticked when it owns nothing else', () => {
  const found = read();
  assert.deepEqual(removeOf(found, shop()), {
    offered: true, ticked: true, stays: null, shared: null, sessions: 0,
  });
  const shared = readOf(list({ databases: [database('a', { owner: 'x' }), database('b', { owner: 'x' })], roles: [role('x')] }));
  const [first] = shared.databases;
  assert.deepEqual(removeOf(shared, first), {
    offered: true, ticked: false, stays: null, shared: 'The role x also owns 1 other database.', sessions: 0,
  });
  const byHand = readOf(list({ databases: [database('a', { owner: 'x' })], roles: [role('x', { made_by_database: false })] }));
  const kept = removeOf(byHand, byHand.databases[0]);
  assert.equal(kept.offered, false);
  assert.equal(kept.ticked, false);
  assert.match(kept.stays, /The role x was not made by this page, so it stays\./);
  const open = removeOf(found, found.databases[0]);
  assert.equal(open.sessions, 2);
  assert.equal(removeOf(readOf(list({ roles: [] })), shop()).stays, null);
});

test('stopping the last network sends PostgreSQL back to this machine only, and restarts it', () => {
  const found = read();
  assert.equal(denyRestarts(found, 'shop'), true);
  assert.equal(denyRestarts(found, 'shop', '192.168.1.0/24'), true);
  assert.equal(denyRestarts(found, 'shop', '10.0.0.0/8'), false);
  const two = readOf(list({ access: [list().access[0], { ...list().access[0], databases: ['blog'], users: ['blog'], line: 92 }] }));
  assert.equal(denyRestarts(two, 'shop'), false);
  assert.equal(denyRestarts(two, 'blog'), false);
  // The lines of the block are what `database` counts: a line written by hand is the administrator's, and is no reason to stay listening.
  assert.equal(denyRestarts(found, 'blog'), false);
  assert.equal(denyRestarts(readOf(list({ access: [list().access[1]] })), 'blog'), true);
});

test('the backups\' words, from the places that copy /shared', () => {
  assert.match(backupWords(backups()).words, /backup hook asks it for a checkpoint, so the copy starts as after a power cut, which PostgreSQL recovers from\. \/shared is in the nightly backup to nas\.$/);
  assert.equal(backupWords(backups()).covered, true);
  assert.match(backupWords(backupsOf(place('far', { every: 'week' }))).words, /weekly backup to far\.$/);
  assert.match(backupWords(backupsOf(place('usb', { every: 'hand' }))).words, /by-hand backup to usb\.$/);
  const none = backupWords(backupsOf(place('nas', { places: ['/users'] })));
  assert.equal(none.covered, false);
  assert.match(none.words, /No backup place copies \/shared yet\.$/);
  assert.equal(backupWords(backupsOf()).covered, false);
  const unknown = backupWords(null);
  assert.equal(unknown.covered, null);
  assert.doesNotMatch(unknown.words, /No backup place/);
});

test('a certificate names a host by its name, or one word under a wildcard', () => {
  assert.equal(covers(['nas.example.org'], 'nas.example.org'), true);
  assert.equal(covers(['*.example.org'], 'nas.example.org'), true);
  assert.equal(covers(['*.example.org'], 'a.b.example.org'), false);
  assert.equal(covers(['*.example.org'], 'example.org'), false);
  assert.equal(covers(['blog.example.org'], 'nas.example.org'), false);
  assert.equal(covers(null, 'nas.example.org'), false);
  assert.equal(covers([5, null], 'nas.example.org'), false);
});

test('the certificates to choose are this machine\'s first, then those that have not ended or been revoked', () => {
  const choices = certificateChoices(certificates());
  assert.deepEqual(choices.map((one) => one.value), ['this-machine', 'wide.example.org', 'blog.example.org']);
  assert.equal(choices[0].label, 'This machine\'s own');
  assert.deepEqual(choices[0].names, ['nas.example.org', 'nas.local']);
  assert.deepEqual(certificateChoices({ certificates: certificates() }).map((one) => one.value).length, 3);
  assert.deepEqual(certificateChoices(null), [{ value: 'this-machine', label: 'This machine\'s own', names: null }]);
});

test('the accounts to choose are those that are not disabled and have a name a role can have', () => {
  const found = accountChoices(accounts());
  assert.deepEqual(found.names, ['chase', 'webdev']);
  assert.equal(found.left, 1);
  assert.equal(leftOutWords(found.left), '1 account is not offered: the name of a role is made of lowercase letters, digits and _.');
  assert.equal(leftOutWords(2), '2 accounts are not offered: the name of a role is made of lowercase letters, digits and _.');
  assert.equal(leftOutWords(0), null);
  assert.deepEqual(accountChoices({ accounts: accounts() }).names, ['chase', 'webdev']);
  assert.deepEqual(accountChoices(null), { names: [], left: 0 });
});

test('an empty server reads as no databases', () => {
  assert.deepEqual(rowsOf(readOf(empty())), []);
});
