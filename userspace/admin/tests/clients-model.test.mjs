// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// pages/clients/model.js (ADR 0690): the levels an area can be given and
// the scopes that give them, the command that adds a client, the words of
// a row, a card and the history, the badge, and what a client's settings
// hold.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  addCommand, badgeOf, cardWords, certificateOf, changesWords, chosenFor, clientName, clientRows, DAYS, DAYS_DEFAULT, endsWords,
  historyLines, httpsSettings, levelsFor, levelsOf, nameProblem, reachOf, scopesOf, sshCheck, sshHost, sshSettings, waitingWords,
} from '../pages/clients/model.js';
import {
  about, areas as areasOf, AT, every, FINGERPRINT, history, machine, NOW, request, time, TOKEN,
} from './clients-fixtures.mjs';

const areas = areasOf().areas;
const labels = (name) => levelsOf(areas.find((area) => area.area === name)).map((level) => level.label);

test('importing the module needs no page', () => {
  assert.equal(typeof globalThis.document, 'undefined');
});

test('an area is offered the levels that would give something, named for what the highest adds', () => {
  assert.deepEqual(labels('services'), ['Off', 'Read', 'Read all and ask']);
  assert.deepEqual(labels('logs'), ['Off', 'Read', 'Read all']);
  assert.deepEqual(labels('storage'), ['Off', 'Read', 'Read and ask']);
  assert.deepEqual(labels('power'), ['Off', 'Ask']);
  // An area that decides who gets in has nothing to ask for.
  assert.deepEqual(labels('accounts'), ['Off', 'Read', 'Read all']);
  assert.deepEqual(levelsOf({ reads: 0, reads_all: 0, asks: 0 }).map((level) => level.id), ['off']);
});

test('the starts are every read a person runs, and the most each area has', () => {
  const read = chosenFor(areas, 'read');
  assert.equal(read.services, 'read');
  assert.equal(read.power, 'off', 'nothing to read there');
  assert.deepEqual([...new Set(Object.values(chosenFor(areas, 'all')))], ['all']);
});

test('the scopes say what was chosen in the fewest words', () => {
  assert.deepEqual(scopesOf(areas, chosenFor(areas, 'read')), ['*:read']);
  assert.deepEqual(scopesOf(areas, chosenFor(areas, 'all')), ['*']);
  assert.deepEqual(scopesOf(areas, { ...chosenFor(areas, 'read'), services: 'all', power: 'all' }), ['*:read', 'services', 'power']);
  // Not every readable area: each is named, in the areas' order.
  assert.deepEqual(scopesOf(areas, { logs: 'read', services: 'all' }), ['services', 'logs:read']);
  assert.deepEqual(scopesOf(areas, { power: 'all' }), ['power']);
  assert.deepEqual(scopesOf(areas, {}), []);
  assert.deepEqual(scopesOf([], {}), []);
  // A level an area does not have comes to the nearest below it.
  assert.deepEqual(scopesOf(areas, { power: 'read' }), []);
});

test('the scopes of a client give back the level of each area', () => {
  for (const chosen of [chosenFor(areas, 'read'), chosenFor(areas, 'all'), { ...chosenFor(areas, 'read'), services: 'all' }]) {
    assert.deepEqual(levelsFor(areas, scopesOf(areas, chosen)), chosen);
  }
  const some = levelsFor(areas, ['services', 'logs:read']);
  assert.deepEqual([some.services, some.logs, some.storage], ['all', 'read', 'off']);
});

test('what a client reaches is every area, or the areas by their titles', () => {
  assert.deepEqual(reachOf(areas, ['*']), {
    reads: { every: true, titles: ['Services', 'Logs', 'Storage', 'Certificates', 'Updates', 'Accounts', 'The console\'s settings'] },
    asks: { every: true, titles: ['Services', 'Storage', 'Certificates', 'Updates', 'Restart and shut down'] },
  });
  const watch = reachOf(areas, ['logs:read', 'services:read']);
  assert.deepEqual(watch.reads, { every: false, titles: ['Services', 'Logs'] });
  assert.deepEqual(watch.asks, { every: false, titles: [] });
  // An area this console does not have is not shown.
  assert.deepEqual(reachOf(areas, ['printers']).reads.titles, []);
});

test('a name is what authd takes, is none of the command\'s words and is not taken', () => {
  assert.equal(nameProblem('', []), 'Give the client a name.');
  for (const wrong of ['Desk', 'a b', 'desk_1', 'x'.repeat(33)]) assert.match(nameProblem(wrong, []), /^A name is 1 to 32/, wrong);
  for (const word of ['add', 'list', 'remove', 'help']) assert.match(nameProblem(word, []), /a word of the admin mcp command/, word);
  assert.match(nameProblem('desk', ['desk']), /^You already have a client named desk\./);
  assert.equal(nameProblem('desk-2', ['desk']), null);
  assert.equal(nameProblem('x'.repeat(32), []), null);
});

test('the command that adds a client quotes the stars and goes on under itself when it is long', () => {
  assert.equal(addCommand({ name: '', way: 'ssh', scopes: ['*:read'], days: 90 }), 'admin mcp add NAME --scope \'*:read\' --days 90');
  assert.equal(addCommand({ name: 'desk', way: 'https', scopes: ['*'], days: 30 }), 'admin mcp add desk --https --scope \'*\' --days 30');
  assert.equal(
    addCommand({ name: 'desk', way: 'https', scopes: ['*:read', 'services', 'certificates', 'updates', 'storage'], days: 90 }),
    'admin mcp add desk --https --scope \'*:read\' \\\n    --scope services,certificates,updates,storage --days 90',
  );
  for (const line of addCommand({ name: 'x'.repeat(32), way: 'https', scopes: areas.map((area) => `${area.area}:read`), days: 366 }).split('\n').slice(0, -1)) {
    assert.ok(line.endsWith(' \\'), line);
  }
});

test('a client ends in the days offered, 90 unless another is chosen and a year at most', () => {
  assert.deepEqual(DAYS, [30, 90, 180, 366]);
  assert.equal(DAYS_DEFAULT, 90);
  assert.equal(endsWords(90, 'UTC', NOW), 'In 90 days · Feb 22, 2027');
  assert.equal(endsWords(30, 'UTC', NOW), 'In 30 days · Dec 24');
});

test('a row says how a client connects, what it reaches, when it was used and when it ends', () => {
  const [assistant, watch, laptop] = clientRows(every().clients, areas, time, NOW);
  assert.deepEqual([assistant.way, assistant.reads.every, assistant.asks.titles], ['SSH', true, ['Services', 'Certificates', 'Updates']]);
  assert.deepEqual([assistant.used, assistant.from, assistant.ends, assistant.ended], ['2 minutes ago', 'from 192.168.1.20', 'Feb 22, 2027', false]);
  assert.deepEqual([watch.way, watch.reads.titles, watch.asks.titles, watch.ends], ['HTTPS, token', ['Services', 'Logs', 'Storage'], [], 'Dec 24']);
  assert.deepEqual([laptop.owner, laptop.used, laptop.from, laptop.ends, laptop.ended], ['alice', 'Never', null, 'Ended Nov 20', true]);
  // A client with no scopes that read, as an older authd might list it.
  assert.deepEqual(clientRows([{ owner: 'chase', name: 'bare', way: 'serial', until: AT, used: 0 }], areas, time, NOW)[0].way, 'serial');
});

test('a client goes by its name, with whose it is when it is another\'s', () => {
  assert.equal(clientName('chase', 'desk', 'chase'), 'desk');
  assert.equal(clientName('alice', 'desk', 'chase'), 'alice\'s desk');
});

test('a card says who asked, when it lapses, the reason, the command and the files', () => {
  assert.deepEqual(cardWords(request(), time, NOW), {
    id: 12,
    title: 'Restart a service: nginx',
    waiting: 'Waiting 2 minutes',
    asked: 'Asked by workstation-assistant, chase\'s client, at 9:40 AM · request 12 · lapses at 10:40 AM if nobody answers',
    reason: 'nginx stopped answering after last night\'s certificate renewal. A restart loads the new certificate.',
    command: 'elevate service nginx restart',
    files: 'None',
  });
  const written = cardWords(request({ command: null, files: ['/system/settings/nginx/nginx.conf', '/system/settings/hosts'] }), time, NOW);
  assert.equal(written.command, null);
  assert.equal(written.files, '/system/settings/nginx/nginx.conf, /system/settings/hosts');
  assert.equal(waitingWords(AT - 59, NOW), 'Waiting less than a minute');
  assert.equal(waitingWords(AT - 60, NOW), 'Waiting 1 minute');
  assert.equal(waitingWords(AT - 3599, NOW), 'Waiting 59 minutes');
});

test('whether a yes would change anything is said as the check found it', () => {
  assert.equal(changesWords(true), 'Yes.');
  assert.match(changesWords(false), /^No\. It is already so/);
  assert.equal(changesWords(null), 'The console cannot tell before it runs.');
  assert.equal(changesWords(undefined), 'The console could not find out.');
});

test('the history says what each client asked for and what became of it, and joins its reads', () => {
  const lines = historyLines(history().entries, areas, time, 'chase');
  assert.deepEqual(lines.map((line) => [line.at, line.title, line.detail, line.kind, line.task]), [
    ['9:40 AM', 'workstation-assistant asked to restart a service: nginx', 'Waiting for a yes', 'info', null],
    ['9:39 AM', 'workstation-assistant read 6 things', 'Services, Logs, Certificates', null, null],
    ['9:12 AM', 'workstation-assistant asked to update an app: valkey', 'Approved by chase at 9:13 AM · task 1841', 'ok', 1841],
    ['8:53 AM', 'alice\'s alice-laptop read 1 thing', 'Logs', null, null],
    ['8:43 AM', 'workstation-assistant asked to renew a certificate: this-machine', 'Approved by chase at 8:44 AM · done', 'ok', null],
    ['7:43 AM', 'workstation-assistant asked to restart a service: valkey', 'Approved by alice at 7:44 AM · it failed, and the client was told why', 'bad', null],
    ['Yesterday 9:43 AM', 'workstation-assistant asked to check a volume: data', 'Refused by chase', 'warn', null],
    ['Nov 22 9:43 AM', 'night-watch asked to restart a service: nginx', 'Lapsed: nobody said yes, or the client was removed', null, null],
  ]);
  // A change that was approved and has not ended, and an area this console does not name.
  const [running, read] = historyLines([
    { at: AT, owner: 'chase', client: 'desk', kind: 'ask', request: 3, action: 'service.restart', title: 'Restart a service: nginx', state: 'approved', by: 'chase' },
    { at: AT, owner: 'chase', client: 'desk', kind: 'read', since: AT, count: 2, areas: ['printers'] },
  ], areas, time, 'chase');
  assert.equal(running.detail, 'Approved by chase · it runs now');
  assert.equal(read.detail, 'printers');
});

test('the badge counts what waits, and there is none for nothing', () => {
  assert.deepEqual(badgeOf(2), { count: 2, label: 'waiting for a yes', kind: 'info' });
  assert.equal(badgeOf(0), null);
});

test('a client\'s settings name the machine, how it is reached and, over HTTPS, the token', () => {
  const ssh = JSON.parse(sshSettings({ server: 'duke-nas', host: 'duke-nas.local', owner: 'chase', name: 'desk' }));
  assert.deepEqual(ssh, { mcpServers: { 'duke-nas': { command: 'ssh', args: ['chase@duke-nas.local', 'admin', 'mcp', 'desk'] } } });
  assert.equal(sshCheck({ host: 'duke-nas.local', owner: 'chase', name: 'desk' }), 'ssh chase@duke-nas.local admin mcp desk --check');
  assert.equal(sshHost('[fe80::1]'), 'fe80::1');
  assert.equal(sshHost('192.168.1.10'), '192.168.1.10');
  const https = JSON.parse(httpsSettings({ server: 'duke-nas', origin: 'https://duke-nas.local:9090', token: TOKEN }));
  assert.deepEqual(https, {
    mcpServers: { 'duke-nas': { type: 'http', url: 'https://duke-nas.local:9090/mcp', headers: { Authorization: `Bearer ${TOKEN}` } } },
  });
});

test('the console\'s certificate is the machine\'s own, another\'s, or not known', () => {
  const own = certificateOf(about(), machine());
  assert.equal(own.own, true);
  assert.deepEqual(own.fingerprint, FINGERPRINT.toUpperCase().match(/.{16}/g).map((group) => group.match(/../g).join(':')));
  assert.equal(certificateOf(about('00ff'), machine()).own, false);
  assert.deepEqual(certificateOf(null, machine()), { own: null, fingerprint: [] });
  assert.equal(certificateOf(about(), null).own, null);
});
