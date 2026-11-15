// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Disks, partitions and volumes in words (design 3.5), over the objects of
// `storage show --json`: the parts of a bar at least 2 percent wide, what a
// partition is for, the words of a mirror in each state, and the words of a
// disk's health for SATA and NVMe.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  disksOf, healthWords, isMirror, layoutWords, listWords, mirrorWords, roleOf, scaled, usedFor, volumeName, volumeTitle,
} from '../lib/disk-model.js';
import {
  disk, GB, MIRROR, partition, passed, ROSE, show, showAlone, showBehind, showConverting, showDup, showMissing, showSpread, showSystemAndData, showWearing, stick, TB, volume,
} from './storage-fixtures.mjs';

const sum = (parts) => parts.reduce((total, part) => total + part.percent, 0);
const near = (actual, expected) => assert.ok(Math.abs(actual - expected) < 0.01, `${actual} is not ${expected}`);

// ---- disks to scale ----------------------------------------------------------

test('a disk with a data partition and room after it is drawn as the two', () => {
  const big = disk('sda', 'Disk', 1000, [partition('sda1', 1, 0, 800)], { free_after: 200 });
  const [one] = scaled([big]);
  assert.deepEqual(one.parts.map((part) => [part.id, part.free]), [['sda1', false], ['free', true]]);
  near(one.parts[0].percent, 80);
  near(one.parts[1].percent, 20);
});

test('a 1 MB start-up partition on a 4 TB disk is still 2 percent', () => {
  const huge = disk('sdd', 'Disk', 4 * TB, [
    partition('sdd1', 1, 1_048_576, 1_000_000, { type: 'efi' }),
    partition('sdd2', 2, 2_097_152, 3.999 * TB),
  ]);
  const [one] = scaled([huge]);
  near(one.parts[0].percent, 2);
  near(one.parts[1].percent, 98);
  near(sum(one.parts), 100);
});

test('the parts add up to 100 however many are small', () => {
  const sizes = [1e6, 2e6, 5e6, 500e9, 300e9, 1e6];
  const crowded = disk('sdd', 'Disk', 1 * TB, sizes.map((size, at) => partition(`sdd${at + 1}`, at + 1, at * 1e6, size)));
  const [one] = scaled([crowded]);
  near(sum(one.parts), 100);
  assert.ok(one.parts.every((part) => part.percent >= 2 - 1e-9));
  assert.ok(one.parts[3].percent > one.parts[4].percent);
});

test('so many partitions that 2 percent each would not fit share the bar equally', () => {
  const many = disk('sdd', 'Disk', 1 * TB, Array.from({ length: 60 }, (_, at) => partition(`sdd${at + 1}`, at + 1, at * 1e6, 1e6)));
  const [one] = scaled([many]);
  near(sum(one.parts), 100);
  near(one.parts[0].percent, one.parts[59].percent);
});

test('partitions are drawn in the order they lie on the disk', () => {
  const swapped = disk('sdd', 'Disk', 1000, [partition('sdd2', 2, 500, 500), partition('sdd1', 1, 0, 400)], { free_after: 0 });
  assert.deepEqual(scaled([swapped])[0].parts.map((part) => part.id), ['sdd1', 'sdd2']);
});

test('a disk with nothing on it is one free part', () => {
  const [one] = scaled([stick()]);
  assert.equal(one.name, 'sdc');
  assert.deepEqual(one.parts.map((part) => [part.id, part.free, Math.round(part.percent)]), [['free', true, 100]]);
});

test('room after the last partition that is under 1 percent is not drawn', () => {
  const tight = disk('sdd', 'Disk', 1000, [partition('sdd1', 1, 0, 995)], { free_after: 5 });
  assert.deepEqual(scaled([tight])[0].parts.map((part) => part.id), ['sdd1']);
  const room = disk('sdd', 'Disk', 1000, [partition('sdd1', 1, 0, 990)], { free_after: 10 });
  assert.deepEqual(scaled([room])[0].parts.map((part) => part.id), ['sdd1', 'free']);
});

test('a disk that does not give its size or partitions does not break the bar', () => {
  assert.deepEqual(scaled([{ name: 'vda', size: 0, partitions: [] }])[0].parts, []);
  assert.deepEqual(scaled(null), []);
  assert.equal(scaled([{ name: 'vda', size: 100 }])[0].parts.length, 1);
});

// ---- volumes -----------------------------------------------------------------

test('a volume is the system, the data, both, or neither', () => {
  const answer = show();
  const [root, mirror] = answer.volumes;
  assert.equal(roleOf(root), 'system');
  assert.equal(volumeTitle(root), 'System');
  assert.equal(volumeName(root), 'the system');
  assert.equal(roleOf(mirror), 'data');
  assert.equal(volumeTitle(mirror), 'Data');
  assert.equal(volumeName(mirror), 'the data');
  const both = showSystemAndData().volumes[0];
  assert.equal(roleOf(both), 'system-data');
  assert.equal(volumeTitle(both), 'System and data');
  assert.equal(volumeName(both), 'the system and data');
  const photos = volume('cccc', 'Photos', '/volumes/photos', ['sdd1']);
  assert.equal(roleOf(photos), 'other');
  assert.equal(volumeTitle(photos), 'Photos');
  assert.equal(volumeName(photos), 'the Photos volume');
  assert.equal(volumeTitle(volume('dddd', '', '/volumes/x', ['sdd1'])), 'Volume');
  assert.equal(volumeName(volume('dddd', '', '/volumes/x', ['sdd1'])), 'the volume');
});

test('a volume that holds only the shared folder is data too', () => {
  assert.equal(roleOf(volume('e', 'files', '/volumes/files', ['sdd1'], { holds: ['/shared'] })), 'data');
  assert.equal(roleOf(volume('e', 'apps', '/volumes/apps', ['sdd1'], { holds: ['/applications'] })), 'other');
});

test('the disks of a volume are named, each once, from its partitions', () => {
  const answer = show();
  assert.deepEqual(disksOf(answer.volumes[1], answer.disks), ['sda', 'sdb']);
  assert.deepEqual(disksOf(answer.volumes[0], answer.disks), ['nvme0']);
  assert.deepEqual(disksOf(volume('x', 'x', '/x', ['sdz1', 'sdz2']), []), ['sdz1', 'sdz2']);
  assert.deepEqual(disksOf(volume('x', 'x', '/x', ['sda1', 'sda2']), [disk('sda', 'D', 10, [partition('sda1', 1, 0, 5), partition('sda2', 2, 5, 5)])]), ['sda']);
});

test('names are listed with and', () => {
  assert.equal(listWords([]), '');
  assert.equal(listWords(['sda']), 'sda');
  assert.equal(listWords(['sda', 'sdb']), 'sda and sdb');
  assert.equal(listWords(['sda', 'sdb', 'sdc']), 'sda, sdb and sdc');
});

test('the layout of the data is named', () => {
  const answer = show();
  assert.equal(layoutWords(answer.volumes[1]), 'Btrfs RAID1');
  assert.equal(layoutWords(answer.volumes[0]), 'Btrfs single');
  assert.equal(layoutWords(showConverting().volumes[1]), 'Btrfs single and RAID1');
  assert.equal(layoutWords({ profiles: null }), 'Btrfs');
});

// ---- the mirror's words ------------------------------------------------------

test('a mirror with every disk and every copy is healthy', () => {
  const answer = show();
  const words = mirrorWords(answer.volumes[1], answer.disks);
  assert.deepEqual(words, { kind: 'healthy', tone: 'ok', word: 'Mirror healthy', where: 'on sda and sdb, each a full copy', detail: null });
  assert.equal(isMirror(answer.volumes[1]), true);
});

test('a mirror that has lost a disk says every file is still there', () => {
  const answer = showMissing();
  const words = mirrorWords(answer.volumes[1], answer.disks);
  assert.equal(words.kind, 'missing');
  assert.equal(words.tone, 'bad');
  assert.equal(words.word, 'Mirror missing a disk');
  assert.equal(words.where, 'on sda');
  assert.equal(words.detail, '1 of 2 disks is missing. Every file is still there, with no second copy.');
});

test('a volume with no second copy that has lost a disk does not promise its files', () => {
  const answer = showMissing();
  const lost = { ...answer.volumes[1], profiles: { data: ['single'], metadata: ['single'], system: ['single'] }, missing: 1 };
  const words = mirrorWords(lost, answer.disks);
  assert.equal(words.word, 'Missing a disk');
  assert.equal(words.detail, '1 of 2 disks is missing.');
});

test('a missing disk comes before one that is behind', () => {
  const answer = showMissing();
  const both = { ...answer.volumes[1], behind: 1 };
  assert.equal(mirrorWords(both, answer.disks).kind, 'missing');
});

test('a disk that is behind needs a check of every block, and is catching up while one runs', () => {
  const answer = showBehind();
  const idle = mirrorWords(answer.volumes[1], answer.disks);
  assert.deepEqual([idle.kind, idle.tone, idle.word], ['behind', 'warn', 'Mirror behind']);
  assert.equal(idle.detail, '1 of 2 disks missed changes. Checking every block copies them across.');
  const running = mirrorWords(answer.volumes[1], answer.disks, true);
  assert.equal(running.word, 'Mirror catching up');
  assert.equal(running.detail, '1 of 2 disks missed changes. The check is copying them across.');
});

test('data that is single over two disks is spread with no second copy', () => {
  const answer = showSpread();
  assert.equal(isMirror(answer.volumes[1]), false);
  const words = mirrorWords(answer.volumes[1], answer.disks);
  assert.deepEqual([words.kind, words.tone, words.word], ['partial', 'warn', 'Partly mirrored']);
  assert.equal(words.detail, 'Its data has no second copy.');
  const all = { ...answer.volumes[1], profiles: { data: ['single'], metadata: ['single'], system: ['single'] } };
  const spread = mirrorWords(all, answer.disks);
  assert.deepEqual([spread.kind, spread.tone, spread.word], ['spread', 'warn', 'Spread over sda and sdb']);
  assert.equal(spread.detail, 'With no second copy: if one disk fails, files are lost.');
  const striped = { ...answer.volumes[1], profiles: { data: ['raid0'], metadata: ['raid1'], system: ['raid1'] } };
  assert.equal(mirrorWords(striped, answer.disks).kind, 'partial');
});

test('data kept twice on one disk is not a mirror', () => {
  const answer = showDup();
  const words = mirrorWords(answer.volumes[0], answer.disks);
  assert.deepEqual([words.kind, words.tone, words.word], ['dup', 'info', 'Two copies on one disk']);
  assert.equal(words.detail, 'This protects against a damaged block, not against the disk failing.');
});

test('a balance that is changing the layout says so', () => {
  const answer = showConverting();
  const words = mirrorWords(answer.volumes[1], answer.disks);
  assert.deepEqual([words.kind, words.tone, words.word], ['converting', 'info', 'Changing layout']);
  assert.equal(isMirror(answer.volumes[1]), false);
});

test('one disk with its data single has nothing to say about copies', () => {
  const answer = showAlone();
  const words = mirrorWords(answer.volumes[0], answer.disks);
  assert.deepEqual([words.kind, words.word, words.detail], ['none', null, null]);
  assert.equal(words.where, 'on nvme0');
});

test('a volume whose profiles cannot be read is told by its disks alone', () => {
  const answer = show();
  const unread = { ...answer.volumes[1], profiles: null };
  const words = mirrorWords(unread, answer.disks);
  assert.deepEqual([words.kind, words.word, words.where], ['disks', 'On 2 disks', 'on sda and sdb']);
  const one = { ...answer.volumes[0], profiles: null };
  assert.equal(mirrorWords(one, answer.disks).kind, 'none');
  assert.equal(mirrorWords(unread, answer.disks, false).detail, null);
});

// ---- what a partition is for -------------------------------------------------

test('the partitions of the system disk say what they hold', () => {
  const answer = show();
  const [esp, root] = answer.disks[0].partitions;
  assert.equal(usedFor(esp, answer.volumes), 'Start-up files (the ESP)');
  assert.equal(usedFor(root, answer.volumes), 'The system');
});

test('the root partition says it holds the people\'s folders when it does', () => {
  const answer = showSystemAndData();
  assert.equal(usedFor(answer.disks[0].partitions[1], answer.volumes), 'The system and data');
});

test('each partition of a mirror is half of it', () => {
  const answer = show();
  assert.equal(usedFor(answer.disks[1].partitions[0], answer.volumes), 'Half of the data mirror');
  const photos = volume('cccc', 'Photos', '/volumes/photos', ['sdd1', 'sde1'], { profiles: MIRROR, disks: 2 });
  assert.equal(usedFor(partition('sdd1', 1, 0, GB), [photos]), 'Half of the Photos mirror');
  assert.equal(usedFor(answer.disks[2].partitions[0], answer.volumes), 'Half of the data mirror');
});

test('a mirror of three disks says one of three', () => {
  const answer = show();
  const three = volume('bbbb-data', 'data', '/volumes/data', ['sda1', 'sdb1', 'sdc1'], { profiles: { data: ['raid1c3'], metadata: ['raid1c3'], system: ['raid1c3'] }, holds: ['/users'] });
  assert.equal(usedFor(answer.disks[1].partitions[0], [three]), 'One of 3 disks of the data mirror');
});

test('data that is spread says so and does not call itself a mirror', () => {
  const answer = showSpread();
  const spread = { ...answer.volumes[1], profiles: { data: ['single'], metadata: ['single'], system: ['single'] } };
  assert.equal(usedFor(answer.disks[1].partitions[0], [spread]), 'Part of the data, spread over 2 disks');
});

test('a partition of a mirror that lost its other disk is still half of it', () => {
  const answer = showMissing();
  assert.equal(usedFor(answer.disks[1].partitions[0], answer.volumes), 'Half of the data mirror');
});

test('swap, boot code and other filesystems are named', () => {
  assert.equal(usedFor(partition('sdd1', 1, 0, 5, { type: 'swap', filesystem: 'swap' }), []), 'Swap');
  assert.equal(usedFor(partition('sdd1', 1, 0, 5, { type: 'bios-boot' }), []), 'Start-up code for older machines');
  assert.equal(usedFor(partition('sdd1', 1, 0, 5, { filesystem: 'vfat', label: 'BACKUP' }), []), 'Files in the FAT format, called BACKUP');
  assert.equal(usedFor(partition('sdd1', 1, 0, 5, { filesystem: 'ext4' }), []), 'Files in the ext4 format');
  assert.equal(usedFor(partition('sdd1', 1, 0, 5, { filesystem: 'f2fs' }), []), 'Files in the f2fs format');
});

test('a partition with no filesystem, and no partition at all, are unused', () => {
  assert.equal(usedFor(partition('sdd1', 1, 0, 5, { type: 'linux-raid' }), []), 'Unused');
  assert.equal(usedFor(null, []), 'Unused');
  assert.equal(usedFor(partition('sdd1', 1, 0, 5), undefined), 'Unused');
});

test('a Btrfs partition that no volume lists is named by its filesystem', () => {
  assert.equal(usedFor(partition('sdd1', 1, 0, 5, { filesystem: 'btrfs', label: 'old' }), []), 'Files in the Btrfs format, called old');
});

// ---- health ------------------------------------------------------------------

test('a SATA disk that passed is healthy', () => {
  assert.deepEqual(healthWords(passed()), { word: 'Healthy', tone: 'ok', note: '' });
});

test('a SATA disk with moved sectors is wearing, and says how many', () => {
  const words = healthWords(passed({ verdict: 'warning', moved: 8, waiting: 2 }));
  assert.deepEqual([words.word, words.tone], ['Wearing', 'warn']);
  assert.equal(words.note, '8 sectors moved to spares, 2 sectors waiting to be moved.');
});

test('a disk whose count rose lately is wearing even when it passed', () => {
  const answer = showWearing();
  assert.equal(healthWords(answer.disks[2].health).word, 'Wearing');
  assert.equal(healthWords(passed({ rose: ROSE })).word, 'Wearing');
  assert.equal(healthWords(passed({ rose: ROSE })).note, 'The disk warns that it may fail.');
});

test('a disk that says it is failing is failing', () => {
  const words = healthWords(passed({ verdict: 'failing', moved: 1 }));
  assert.deepEqual([words.word, words.tone, words.note], ['Failing', 'bad', '1 sector moved to spares.']);
  assert.equal(healthWords({ verdict: 'failing' }).note, 'The disk says it is failing.');
});

test('an NVMe drive is read by its media errors and the writes it has used', () => {
  const fine = show().disks[0].health;
  assert.deepEqual(healthWords(fine), { word: 'Healthy', tone: 'ok', note: '' });
  const worn = healthWords({ ...fine, verdict: 'warning', media_errors: 3, used_percent: 93 });
  assert.equal(worn.word, 'Wearing');
  assert.equal(worn.note, '3 media errors, 93 percent of its rated writes used.');
  assert.equal(healthWords({ ...fine, verdict: 'warning', media_errors: 1 }).note, '1 media error.');
});

test('a disk that gives no health is not checked', () => {
  const words = healthWords(null);
  assert.deepEqual(words, { word: 'Not checked', tone: 'off', note: 'This disk does not report its health.' });
  assert.deepEqual(healthWords(undefined), words);
});

test('the words of a disk that could be anything are not thrown by missing fields', () => {
  assert.equal(healthWords({}).word, 'Healthy');
  assert.equal(healthWords({ verdict: 'warning' }).note, 'The disk warns that it may fail.');
});
