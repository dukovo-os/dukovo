// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Move to new hardware page says and decides (design 6), from
// fixtures of `backup find --json` and `backup restore --machine --plan
// --json` (design 11.6): what the address asks for, the backups and keys to
// choose from, the arguments of `move.plan` and `move.stage` (and that a
// typed key is in no other), the cards' choices and their lines, whether the
// files fit, the rows of what comes across and of the hardware for a record
// of format 2 and for one of format 1, the restart's words, the four steps
// and why Stage it is off.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  AUTOMATIC, FORMAT_ONE_WORDS, RESTART_COMMAND, STAGED_WORDS, STEPS, afterOf, argsOf, cardLines, cardsOf, changesOf, choiceGroups, chosenByEntry,
  clashWords, comesAcross, crumbsOf, currentStep, defaultKey, defaultPicks, entryOf, folderFault, hardwareOf, keptAddresses, keysOf, leadOf, showArgs,
  sourcesOf, spaceOf, stageFault, stepsOf,
} from '../pages/move/model.js';
import {
  NEW0, NEW1, OLD0, OLD1, found, freshBackups, hardware, nasBackups, nothingFound, plan, planFull, planOne, planTwoCards, time,
} from './move-fixtures.mjs';

const TYPED = 'AGE-SECRET-KEY-1TYPED-MARKER-7731';

const query = (text) => new URLSearchParams(text);

// ---- the address ----

test('the address chooses a backup place or a folder, and says whether the person came from Finish setting up', () => {
  assert.deepEqual(entryOf(query('')), { from: null, source: null, setup: false });
  assert.deepEqual(entryOf(query('from=nas')), { from: 'nas', source: null, setup: false });
  assert.deepEqual(entryOf(query('source=/volumes/BACKUP/dukovo-backup/duke-nas&via=setup')), { from: null, source: '/volumes/BACKUP/dukovo-backup/duke-nas', setup: true });
});

test('a value of the address that is not what it names is left out', () => {
  assert.equal(entryOf(query('from=../etc')).from, null);
  assert.equal(entryOf(query('from=a b')).from, null);
  assert.equal(entryOf(query(`from=${'x'.repeat(33)}`)).from, null);
  assert.equal(entryOf(query('source=relative/path')).source, null);
  assert.equal(entryOf(query('source=/volumes/../etc')).source, null);
  assert.equal(entryOf(query('via=other')).setup, false);
});

test('the crumb is the page that the person came from', () => {
  assert.deepEqual(crumbsOf({ setup: true }), [{ title: 'Finish setting up', path: '/setup' }]);
  assert.deepEqual(crumbsOf({ setup: false }), [{ title: 'Snapshots and backups', path: '/snapshots' }]);
});

test('a folder that is typed must be a whole path with no .. and no control character', () => {
  assert.equal(folderFault('/volumes/BACKUP/dukovo-backup/duke-nas'), '');
  assert.equal(folderFault(''), 'Type the whole path of the folder.');
  assert.equal(folderFault('backups'), 'Write the whole path, starting with /.');
  assert.equal(folderFault('/volumes/../etc'), 'The backups cannot be read from that path.');
  assert.equal(folderFault('/volumes/a\nb'), 'The backups cannot be read from that path.');
  assert.equal(folderFault(`/${'x'.repeat(4096)}`), 'The backups cannot be read from that path.');
});

// ---- step 1: the backups and the keys ----

test('the backups of step 1 are the machines on plugged-in disks and the places of this machine', () => {
  const { disks, places } = sourcesOf(found(), nasBackups(), time);
  assert.deepEqual(disks.map((one) => [one.id, one.title, one.detail]), [
    ['disk:/volumes/BACKUP/dukovo-backup/duke-nas', 'USB disk BACKUP', 'duke-nas, today 1:30 AM'],
    ['disk:/volumes/DATA/backups/old-pi', 'Disk DATA', 'old-pi, Nov 12 10:05 PM'],
  ]);
  assert.equal(disks[0].source, '/volumes/BACKUP/dukovo-backup/duke-nas');
  assert.equal(disks[0].machine, 'duke-nas');
  assert.deepEqual(places.map((one) => [one.id, one.kind, one.source, one.title, one.detail]), [
    ['place:nas', 'place', 'nas', 'Backup place nas', 'ssh://backup@nas.local/srv/dukovo'],
  ]);
});

test('a machine that was just installed has no places, and nothing found is an empty list', () => {
  assert.deepEqual(sourcesOf(nothingFound(), freshBackups(), time), { disks: [], places: [] });
  assert.deepEqual(sourcesOf(null, null, time), { disks: [], places: [] });
  assert.equal(sourcesOf(found(), freshBackups(), time).places.length, 0);
});

test('a backup without a newest time or a machine is still a choice, and is not told apart by what it lacks', () => {
  const { disks } = sourcesOf({ sources: [{ source: '/volumes/X/backups/m', volume: 'X', removable: false }] }, null, time);
  assert.deepEqual(disks.map((one) => [one.title, one.detail, one.machine]), [['Disk X', '', null]]);
});

test('the address chooses the backup place or the disk that it names, and nothing else', () => {
  const { disks, places } = sourcesOf(found(), nasBackups(), time);
  const items = [...disks, ...places];
  assert.equal(chosenByEntry(items, { from: 'nas', source: null }).id, 'place:nas');
  assert.equal(chosenByEntry(items, { from: null, source: '/volumes/DATA/backups/old-pi' }).id, 'disk:/volumes/DATA/backups/old-pi');
  assert.equal(chosenByEntry(items, { from: 'gone', source: null }), null);
  assert.equal(chosenByEntry(items, { from: null, source: '/volumes/NOPE' }), null);
  assert.equal(chosenByEntry(items, { from: null, source: null }), null);
});

test('a key file is named by its volume, and a key that backup find gave no path for is not offered', () => {
  const keys = keysOf(found());
  assert.deepEqual(keys.map((key) => [key.path, key.name, key.for, key.title]), [
    ['/volumes/KEYS/duke-nas-recovery.key', 'duke-nas-recovery.key', 'nas', 'Recovery key from the KEYS stick'],
    ['/volumes/KEYS/spare.key', 'spare.key', 'spare', 'Recovery key from the KEYS stick'],
  ]);
  assert.deepEqual(keysOf({ keys: [{ for: 'x' }, { path: '/tmp/odd.key' }] }).map((key) => key.title), ['Recovery key odd.key']);
  assert.deepEqual(keysOf(null), []);
});

test('the key named after the machine is chosen by default, and nothing is chosen when none is', () => {
  const keys = keysOf(found());
  const [disk] = sourcesOf(found(), null, time).disks;
  assert.equal(defaultKey(keys, disk).path, '/volumes/KEYS/duke-nas-recovery.key');
  const [, other] = sourcesOf(found(), null, time).disks;
  assert.equal(defaultKey(keys, other), null, 'a key for another machine is not guessed');
  assert.equal(defaultKey([], disk), null);
});

test('a key made for the backup place that was chosen is chosen, by its name or by what backup find says it is for', () => {
  const place = sourcesOf(null, nasBackups(), time).places[0];
  assert.equal(defaultKey(keysOf(found()), place).path, '/volumes/KEYS/duke-nas-recovery.key', 'its key says it is for nas');
  const named = keysOf({ keys: [{ path: '/volumes/KEYS/NAS-recovery.key', for: 'other' }] });
  assert.equal(defaultKey(named, place).path, '/volumes/KEYS/NAS-recovery.key', 'the case of a name does not matter');
  assert.equal(defaultKey(keysOf({ keys: [{ path: '/volumes/KEYS/nas.key', for: null }] }), place).name, 'nas.key');
});

// ---- the arguments of the actions ----

test('one of the key file and the typed key goes to the action, and the typed key wins', () => {
  assert.deepEqual(argsOf({ source: '/s', file: '/volumes/KEYS/a.key' }), { source: '/s', key_file: '/volumes/KEYS/a.key' });
  assert.deepEqual(argsOf({ source: '/s', typed: TYPED }), { source: '/s', key: TYPED });
  assert.deepEqual(argsOf({ source: '/s', file: '/volumes/KEYS/a.key', typed: TYPED }), { source: '/s', key: TYPED }, 'the file is left out when a key is typed');
  assert.deepEqual(argsOf({ source: 'nas' }), { source: 'nas' });
});

test('the cards and the time of the backup go with the call when there are some', () => {
  const args = argsOf({
    source: '/s', file: '/k', cards: `${OLD0}=${NEW0}`, at: '2026-11-14-013000',
  });
  assert.deepEqual(args, {
    source: '/s', key_file: '/k', cards: `${OLD0}=${NEW0}`, at: '2026-11-14-013000',
  });
  assert.equal('cards' in argsOf({ source: '/s', file: '/k' }), false);
  assert.equal('at' in argsOf({ source: '/s', file: '/k' }), false);
});

test('the Show command panel is never given a typed key: its key is empty, and a key file is its path', () => {
  const typed = showArgs({
    source: '/s', how: 'typed', file: '/volumes/KEYS/a.key', cards: `${OLD0}=automatic`, at: '2026-11-14-013000',
  });
  assert.deepEqual(typed, {
    source: '/s', key: '', cards: `${OLD0}=automatic`, at: '2026-11-14-013000',
  });
  assert.equal(JSON.stringify(typed).includes('MARKER'), false);
  const file = showArgs({ source: '/s', how: 'file', file: '/volumes/KEYS/a.key' });
  assert.deepEqual(file, { source: '/s', key_file: '/volumes/KEYS/a.key' });
});

// ---- the network cards' choices ----

test('each old card with a fixed address is paired with the new card that the plan pairs it with', () => {
  const found2 = plan();
  assert.deepEqual(cardsOf(found2).map((one) => one.old.mac), [OLD0]);
  assert.deepEqual(defaultPicks(found2), { [OLD0]: NEW0 });
  assert.equal(cardLines(found2, defaultPicks(found2)), `${OLD0}=${NEW0}`);
});

test('a card with no new card to pair with is left to ask for an address', () => {
  const lone = plan({ cards: [{ old: { name: 'net0', mac: OLD0, addresses: ['192.168.1.12/24'], router: null }, new: null, choice: 'automatic' }] });
  assert.deepEqual(defaultPicks(lone), { [OLD0]: AUTOMATIC });
  assert.equal(cardLines(lone, defaultPicks(lone)), `${OLD0}=automatic`);
});

test('the lines of the cards argument are OLD=NEW or OLD=automatic, one for each old card', () => {
  const two = planTwoCards();
  assert.equal(cardLines(two, defaultPicks(two)), `${OLD0}=${NEW0}\n${OLD1}=${NEW1}`);
  assert.equal(cardLines(two, { [OLD0]: AUTOMATIC, [OLD1]: NEW0 }), `${OLD0}=automatic\n${OLD1}=${NEW0}`);
  assert.equal(cardLines(two, {}), `${OLD0}=automatic\n${OLD1}=automatic`, 'a card with no choice made is automatic, the safe one');
  assert.equal(cardLines(plan({ cards: [] }), {}), '');
});

test('no more than eight cards are offered, which is as many as the action takes', () => {
  const many = plan({ cards: Array.from({ length: 12 }, (_, at) => ({ old: { name: `net${at}`, mac: `52:54:00:cc:00:${String(at).padStart(2, '0')}`, addresses: ['10.0.0.1/24'], router: null }, new: null, choice: 'automatic' })) });
  assert.equal(cardsOf(many).length, 8);
  assert.equal(cardLines(many, {}).split('\n').length, 8);
  assert.equal(cardsOf(plan({ cards: [{ old: { name: 'x', mac: '', addresses: [] }, new: null }] })).length, 0, 'a card with no MAC cannot be named');
});

test('a card given the address of another, or left to ask, changes the settings; a card kept as it was does not', () => {
  const two = planTwoCards();
  assert.equal(changesOf(two, { [OLD0]: OLD0, [OLD1]: OLD1 }), 0);
  assert.equal(changesOf(two, defaultPicks(two)), 2);
  assert.equal(changesOf(two, { [OLD0]: NEW0, [OLD1]: OLD1 }), 1);
  assert.equal(changesOf(two, {}), 2);
});

test('the addresses that the choices keep are those of the old cards that are not left automatic', () => {
  const two = planTwoCards();
  assert.deepEqual(keptAddresses(two, defaultPicks(two)), ['192.168.1.12', '10.0.0.5']);
  assert.deepEqual(keptAddresses(two, { [OLD0]: AUTOMATIC, [OLD1]: NEW1 }), ['10.0.0.5']);
  assert.deepEqual(keptAddresses(two, {}), []);
});

test('two old cards given one new card are refused in words, and it is said for the cards by name', () => {
  const two = planTwoCards();
  assert.equal(clashWords(two, defaultPicks(two)), '');
  assert.equal(clashWords(two, { [OLD0]: NEW0, [OLD1]: NEW0 }), 'net0 and net1 cannot both take the same port. Let one of them get an address automatically.');
  assert.equal(clashWords(two, { [OLD0]: AUTOMATIC, [OLD1]: AUTOMATIC }), '', 'two that ask for an address do not clash');
});

test('the choice of a card is each new card, and an address asked for', () => {
  const two = planTwoCards();
  const [first] = choiceGroups(two, defaultPicks(two), 'duke-nas');
  assert.equal(first.mac, OLD0);
  assert.equal(first.label, 'net0\'s address');
  assert.deepEqual(first.options.map((one) => [one.value, one.words, one.checked]), [
    [NEW0, 'Give net0 (Realtek RTL8125) duke-nas\'s address, 192.168.1.12, fixed', true],
    [NEW1, 'Give net1 (Intel I226) duke-nas\'s address, 192.168.1.12, fixed', false],
    [AUTOMATIC, 'Let it get an address automatically', false],
  ]);
  const one = choiceGroups(plan(), defaultPicks(plan()), 'duke-nas')[0];
  assert.deepEqual(one.options.map((option) => option.words), ['Give it duke-nas\'s address, 192.168.1.12, fixed', 'Let it get an address automatically']);
  assert.equal(choiceGroups(plan(), { [OLD0]: AUTOMATIC }, 'duke-nas')[0].options.find((option) => option.checked).value, AUTOMATIC);
});

// ---- whether the files fit ----

test('the files fit when the plan says so, with what is spare', () => {
  const space = spaceOf(plan());
  assert.equal(space.known, true);
  assert.equal(space.fits, true);
  assert.equal(space.words, '');
  assert.equal(Math.round(space.spare / 1e9), 620);
});

test('the files that do not fit are said so in the sentence of the design', () => {
  const space = spaceOf(planFull());
  assert.equal(space.fits, false);
  assert.equal(space.words, 'The files need 1.24 TB and this machine has 900 GB free.');
});

test('a plan that does not say how much space there is does not refuse, and one that says no fit is believed', () => {
  assert.deepEqual([spaceOf(planOne()).known, spaceOf(planOne()).fits, spaceOf(planOne()).words], [false, true, '']);
  assert.equal(spaceOf(plan({ fits: false, needed: null, free: null })).fits, false);
  assert.equal(spaceOf(plan({ fits: undefined, needed: 5, free: 4 })).fits, false, 'without `fits`, the numbers decide');
  assert.equal(spaceOf(null).fits, true);
});

// ---- what comes across ----

test('what comes across is the name and accounts, the settings, the apps, the files and the older snapshots that are not copied', () => {
  const rows = comesAcross(plan(), defaultPicks(plan()));
  assert.deepEqual(rows.map((row) => row.id), ['names', 'settings', 'apps', 'files', 'snapshots']);
  const by = Object.fromEntries(rows.map((row) => [row.id, row]));
  assert.equal(by.names.title, 'Name, accounts and passwords');
  assert.equal(by.names.detail, 'duke-nas · owner and helper, with their passkeys and SSH keys');
  assert.equal(by.apps.detail, '2 apps at the same versions, each with the permissions it was given');
  assert.equal(by.files.detail, '/users and /shared, 502 GB', 'the settings and apps have rows of their own');
  assert.deepEqual(by.files.pill, { tone: 'ok', words: 'Comes across' });
  assert.equal(by.snapshots.pill, null);
  assert.equal(by.snapshots.muted, 'Not copied');
  assert.equal(by.snapshots.detail, 'They stay in the backups; this machine starts its own');
});

test('the settings row counts the changes that the cards\' choices make', () => {
  const one = plan();
  assert.equal(comesAcross(one, { [OLD0]: OLD0 }).find((row) => row.id === 'settings').detail, '/system/settings');
  assert.equal(comesAcross(one, { [OLD0]: NEW0 }).find((row) => row.id === 'settings').detail, '/system/settings, with 1 change for the new network port (below)');
  const two = planTwoCards();
  assert.equal(comesAcross(two, defaultPicks(two)).find((row) => row.id === 'settings').detail, '/system/settings, with 2 changes for the new network ports (below)');
});

test('files that do not fit are marked, and a record with no sizes names the folders without them', () => {
  assert.deepEqual(comesAcross(planFull(), {}).find((row) => row.id === 'files').pill, { tone: 'bad', words: 'Does not fit' });
  assert.equal(comesAcross(planOne(), {}).find((row) => row.id === 'files').detail, '/users and /shared');
  const bare = comesAcross(plan({ accounts: [], apps: [], places: [] }), {});
  assert.equal(bare.find((row) => row.id === 'names').detail, 'duke-nas · no accounts of people');
  assert.equal(comesAcross(planOne(), {}).find((row) => row.id === 'names').detail, 'duke-nas · its accounts, with their passkeys and SSH keys', 'a record of format 1 does not name them');
  assert.equal(bare.find((row) => row.id === 'apps').detail, 'No apps are installed');
  assert.equal(bare.find((row) => row.id === 'files').detail, 'No folders');
});

test('the files of a disk of their own are restored beside, and are not counted with the files that come across', () => {
  const places = [
    { path: '/system/settings', folder: 'settings', bytes: 1_000_000 },
    { path: '/users', folder: 'users', bytes: 412_000_000_000 },
    { path: '/volumes/PHOTOS', folder: 'photos', bytes: 900_000_000_000 },
  ];
  const rows = comesAcross(plan({ places }), {});
  assert.deepEqual(rows.map((row) => row.id), ['names', 'settings', 'apps', 'files', 'other-disks', 'snapshots']);
  const by = Object.fromEntries(rows.map((row) => [row.id, row]));
  assert.equal(by.files.detail, '/users, 412 GB');
  assert.equal(by['other-disks'].title, 'Files on other disks');
  assert.equal(by['other-disks'].detail, '/volumes/PHOTOS: each is restored as a snapshot on this machine\'s disk of the same name, when it has one, and otherwise stays in the backups');
  assert.equal(by['other-disks'].pill, null);
  assert.equal(by['other-disks'].muted, 'Restored beside');
});

// ---- what differs in the hardware: a record of format 2 ----

test('a record of format 2 has the rows of the network ports, the disks, the serial console and the processor', () => {
  const found2 = hardwareOf(plan(), defaultPicks(plan()), { storage: true });
  assert.equal(found2.note, '');
  assert.deepEqual(found2.rows.map((row) => row.id), ['ports', 'disks', 'serial', 'cpu']);
});

test('the network ports row says what each machine has, offers a choice for each fixed address and lists the firewall rules that do nothing', () => {
  const [ports] = hardwareOf(plan(), defaultPicks(plan())).rows;
  assert.equal(ports.title, 'Network ports');
  assert.deepEqual(ports.pill, { tone: 'info', words: '1 choice' });
  assert.deepEqual(ports.lines, ['duke-nas had net0 (Realtek RTL8111) and net1 (Intel I210). This machine has net0 (Realtek RTL8125).']);
  assert.deepEqual(ports.notes, ['The firewall rule "deny ping on net1" names a port this machine does not have, so it does nothing here.']);
  assert.equal(ports.groups.length, 1);
  assert.equal(ports.clash, '');
});

test('the network ports row of a record that has no fixed address has nothing to choose', () => {
  const [ports] = hardwareOf(plan({ cards: [], firewall_on_missing: [] }), {}).rows;
  assert.deepEqual(ports.pill, { tone: 'ok', words: 'Nothing to choose' });
  assert.deepEqual(ports.groups, []);
});

test('an old card that this machine has no card for is said to be kept and unused', () => {
  const none = plan({ hardware: hardware({ now: { ...hardware().now, cards: [] } }), cards: [{ old: plan().cards[0].old, new: null, choice: 'automatic' }] });
  const [ports] = hardwareOf(none, defaultPicks(none)).rows;
  assert.ok(ports.lines[0].endsWith('This machine has no network port.'));
  assert.ok(ports.notes.includes('net0\'s settings are kept but unused.'));
  assert.deepEqual(ports.groups, [], 'a card that has no card to go to has no choice to offer');
});

test('the disks row says what the old machine kept its data on and what this one has, and warns of no mirror', () => {
  const [, disks] = hardwareOf(plan(), defaultPicks(plan()), { storage: true }).rows;
  assert.equal(disks.title, 'Disks');
  assert.deepEqual(disks.pill, { tone: 'warn', words: 'No mirror' });
  assert.deepEqual(disks.lines, ['duke-nas kept its data on a mirror of two 2 TB disks. This machine has one disk, Samsung 870 (2 TB) with 1.86 TB free, so the files fit with 620 GB to spare.']);
  assert.deepEqual(disks.notes, ['Without a mirror, a disk that fails loses the changes since the last backup. Storage shows how to add a disk.']);
});

test('the note about adding a disk names Storage only when this console has it', () => {
  const [, disks] = hardwareOf(plan(), {}, { storage: false }).rows;
  assert.deepEqual(disks.notes, ['Without a mirror, a disk that fails loses the changes since the last backup.']);
});

test('the disks row fits when this machine is as mirrored as the old one, and does not when the files do not fit', () => {
  const mirrored = planTwoCards();
  const [, fits] = hardwareOf(mirrored, defaultPicks(mirrored)).rows;
  assert.deepEqual(fits.pill, { tone: 'ok', words: 'Fits' });
  assert.deepEqual(fits.notes, []);
  const [, full] = hardwareOf(planFull(), {}).rows;
  assert.deepEqual(full.pill, { tone: 'bad', words: 'Does not fit' });
  assert.ok(full.notes.includes('The files need 1.24 TB and this machine has 900 GB free.'));
  assert.ok(!full.lines[0].includes('to spare'));
});

test('disks of different sizes are listed by size', () => {
  const odd = plan({ hardware: hardware({ then: { ...hardware().then, mirror: false, disks: [{ model: 'a', size: 2e12 }, { model: 'b', size: 1e12 }, { model: 'c', size: 1e12 }] } }) });
  const [, disks] = hardwareOf(odd, {}).rows;
  assert.ok(disks.lines[0].startsWith('duke-nas kept its data on three disks of 2 TB, 1 TB and 1 TB.'));
});

test('the serial console row is there only when the old machine had one and this one has none', () => {
  const rows = hardwareOf(plan(), {}).rows;
  const serial = rows.find((row) => row.id === 'serial');
  assert.deepEqual(serial.pill, { tone: 'info', words: 'Plug it in' });
  assert.equal(serial.lines[0], 'duke-nas sent its console to a USB serial adapter, FT232R. Plug the same adapter in here and it carries on.');
  assert.equal(hardwareOf(planTwoCards(), {}).rows.some((row) => row.id === 'serial'), false, 'this machine has one as well');
  const never = plan({ hardware: hardware({ then: { ...hardware().then, serial: null } }) });
  assert.equal(hardwareOf(never, {}).rows.some((row) => row.id === 'serial'), false);
});

test('the processor row says the two models when they differ, and nothing to change', () => {
  const same = hardwareOf(plan(), {}).rows.find((row) => row.id === 'cpu');
  assert.deepEqual(same.pill, { tone: 'ok', words: 'Nothing to change' });
  assert.equal(same.lines[0], 'Intel N100, as before.');
  const other = plan({ hardware: hardware({ now: { ...hardware().now, cpu: 'AMD Ryzen 5' } }) });
  assert.equal(hardwareOf(other, {}).rows.find((row) => row.id === 'cpu').lines[0], 'AMD Ryzen 5 replaces Intel N100.');
});

test('two old cards given one new card put the refusal on the ports row', () => {
  const two = planTwoCards();
  const [ports] = hardwareOf(two, { [OLD0]: NEW0, [OLD1]: NEW0 }).rows;
  assert.equal(ports.clash, 'net0 and net1 cannot both take the same port. Let one of them get an address automatically.');
  assert.deepEqual(ports.pill, { tone: 'info', words: '2 choices' });
});

// ---- what differs in the hardware: a record of format 1 ----

test('a record of format 1 says why there are no hardware rows, and offers the cards from the network settings alone', () => {
  const old = planOne();
  const { note, rows } = hardwareOf(old, defaultPicks(old));
  assert.equal(note, FORMAT_ONE_WORDS);
  assert.equal(note, 'These backups were made before Dukovo kept the machine\'s hardware with them, so the differences cannot be shown.');
  assert.deepEqual(rows.map((row) => row.id), ['ports']);
  assert.equal(rows[0].lines[0], 'duke-nas had fixed addresses on net0.');
  assert.equal(rows[0].groups.length, 1);
  assert.deepEqual(rows[0].groups[0].options.map((option) => option.value), [NEW0, AUTOMATIC], 'only the card that the plan paired it with');
});

test('a record of format 1 with no fixed address has the line and no row at all', () => {
  const { note, rows } = hardwareOf(planOne({ cards: [] }), {});
  assert.equal(note, FORMAT_ONE_WORDS);
  assert.deepEqual(rows, []);
});

test('the rest of a record of format 1 reads as any other', () => {
  const old = planOne();
  assert.equal(comesAcross(old, defaultPicks(old)).find((row) => row.id === 'files').detail, '/users and /shared');
  assert.equal(stageFault(old, defaultPicks(old)), '');
});

// ---- after the restart ----

test('the restart says to turn the old machine off, in bad while its fixed address is kept', () => {
  const kept = afterOf(plan(), defaultPicks(plan()), { port: '9090' });
  assert.equal(kept.banner.tone, 'bad');
  assert.equal(kept.banner.lead, 'Turn the old duke-nas off before this machine restarts.');
  assert.equal(kept.banner.detail, 'Otherwise two machines answer as duke-nas at 192.168.1.12, and both run the nightly backups.');
  const auto = afterOf(plan(), { [OLD0]: AUTOMATIC }, { port: '9090' });
  assert.equal(auto.banner.tone, 'warn');
  assert.equal(auto.banner.detail, 'Otherwise two machines answer as duke-nas, and both run the nightly backups.');
});

test('after the restart the console is at the old machine\'s name, and at its address when that is kept', () => {
  const kept = afterOf(plan(), defaultPicks(plan()), { port: '9090' });
  assert.equal(kept.open, 'After the restart, open https://duke-nas.local:9090/ or https://192.168.1.12:9090/ and sign in as one of duke-nas\'s administrators.');
  assert.equal(afterOf(plan(), {}, { port: '9090' }).open, 'After the restart, open https://duke-nas.local:9090/ and sign in as one of duke-nas\'s administrators.');
  assert.ok(afterOf(plan(), {}).open.includes('https://duke-nas.local/'), 'without a port, the default one');
  const six = plan({ cards: [{ old: { name: 'net0', mac: OLD0, addresses: ['fd00::12/64'], router: null }, new: { name: 'net0', mac: NEW0 }, choice: 'fixed' }] });
  assert.ok(afterOf(six, defaultPicks(six), { port: '9090' }).open.includes('https://[fd00::12]:9090/'), 'an IPv6 address is in brackets');
});

test('the three lines of what happens after the restart are the board\'s, and the command is elevate reboot', () => {
  const { lines } = afterOf(plan(), {});
  assert.equal(lines.length, 3);
  assert.equal(lines[0], 'The services start with duke-nas\'s settings, then the console checks each one and each website');
  assert.equal(lines[1], 'Anything that fails a check is listed on the Overview, with a way to fix or undo it');
  assert.equal(lines[2], 'This fresh install is kept, read-only, in /snapshots/machine-restore until you remove it');
  assert.equal(RESTART_COMMAND, 'elevate reboot');
});

// ---- the four steps ----

test('there are four steps, and only the first is open at the start', () => {
  assert.deepEqual(STEPS.map((step) => step.title), ['Find the backup', 'Unlock it', 'Check what changes', 'Restart to finish']);
  const steps = stepsOf({});
  assert.deepEqual(steps.map((step) => [step.id, step.done, step.reachable]), [
    ['find', false, true], ['unlock', false, false], ['check', false, false], ['restart', false, false],
  ]);
  assert.equal(currentStep(steps, 'restart'), 'find', 'a step that cannot be reached is not shown');
});

test('a step is done by what the person did, and opens the next', () => {
  const [disk] = sourcesOf(found(), null, time).disks;
  const chosen = stepsOf({ choice: disk });
  assert.deepEqual(chosen.map((step) => step.done), [true, false, false, false]);
  assert.equal(chosen[0].summary, 'USB disk BACKUP · duke-nas, today 1:30 AM');
  assert.deepEqual(chosen.map((step) => step.reachable), [true, true, false, false]);
  assert.equal(currentStep(chosen, 'find'), 'find');
  assert.equal(currentStep(chosen, null), 'unlock');

  const unlocked = stepsOf({ choice: disk, plan: plan(), key: { how: 'file', title: 'Recovery key from the KEYS stick' } });
  assert.deepEqual(unlocked.map((step) => step.done), [true, true, false, false]);
  assert.equal(unlocked[1].summary, 'Recovery key from the KEYS stick');
  assert.equal(currentStep(unlocked, null), 'check');

  const reviewed = stepsOf({ choice: disk, plan: plan(), key: { how: 'typed', title: 'x' }, reviewed: true });
  assert.deepEqual(reviewed.map((step) => step.done), [true, true, true, false]);
  assert.equal(reviewed[1].summary, 'Recovery key typed');
  assert.equal(currentStep(reviewed, null), 'restart');
});

test('the person may go back to a step that is done, and the steps after it stay as they were', () => {
  const [disk] = sourcesOf(found(), null, time).disks;
  const reviewed = stepsOf({ choice: disk, plan: plan(), key: { how: 'file', title: 'k' }, reviewed: true });
  assert.equal(currentStep(reviewed, 'unlock'), 'unlock');
  assert.equal(currentStep(reviewed, 'find'), 'find');
});

test('a restore that is staged, or being staged, opens the last step by itself', () => {
  const staged = stepsOf({ staged: true });
  assert.equal(staged[3].reachable, true);
  assert.equal(staged[3].done, true);
  assert.equal(staged[3].summary, 'Staged');
  assert.equal(currentStep(staged, null), 'find', 'the first that is not done; the page opens the last one itself');
  assert.equal(currentStep(staged, 'restart'), 'restart');
  const staging = stepsOf({ staging: true });
  assert.equal(staging[3].reachable, true);
  assert.equal(staging[3].done, false);
});

test('the lead names the machine that is moved, when it is known', () => {
  assert.ok(leadOf('duke-nas').startsWith('This machine takes duke-nas\'s name, accounts, settings, apps and files from a backup.'));
  assert.ok(leadOf(null).startsWith('This machine takes the old machine\'s name'));
  assert.ok(leadOf(null).endsWith('you can cancel until then.'));
});

// ---- why Stage it is off ----

test('Stage it is on when the files fit and no two cards were given one port', () => {
  assert.equal(stageFault(plan(), defaultPicks(plan())), '');
});

test('Stage it is off with the sentence when the files do not fit', () => {
  assert.equal(stageFault(planFull(), {}), 'The files need 1.24 TB and this machine has 900 GB free.');
});

test('Stage it is off when two old cards were given one port, and when a restore is staged or being staged', () => {
  const two = planTwoCards();
  assert.equal(stageFault(two, { [OLD0]: NEW0, [OLD1]: NEW0 }), 'net0 and net1 cannot both take the same port. Let one of them get an address automatically.');
  assert.equal(stageFault(plan(), {}, { staged: true }), 'A restore is staged already. Cancel it to stage again.');
  assert.equal(stageFault(plan(), {}, { running: true }), 'A restore is being staged.');
  assert.equal(STAGED_WORDS, 'Nothing changes until this machine restarts, and you can cancel until then.');
});

test('a typed key is in the arguments of no function of the model but argsOf', () => {
  const p = plan();
  const picks = defaultPicks(p);
  const everything = JSON.stringify([
    showArgs({ source: '/s', how: 'typed', cards: cardLines(p, picks) }), comesAcross(p, picks), hardwareOf(p, picks), afterOf(p, picks), stepsOf({ plan: p, key: { how: 'typed', title: 'x' } }),
  ]);
  assert.equal(everything.includes(TYPED), false);
  assert.deepEqual(argsOf({ source: '/s', typed: TYPED }), { source: '/s', key: TYPED });
});
