// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the RAID part of the Storage page says (raid-model.js), from fixtures of the
// `storage.raid` answer: the banner, the controllers' rows, the bays, the volumes with
// their task bars and the physical disks with the health the controller reports, and the
// words the disk cards use for a disk that is a RAID volume.

import assert from 'node:assert/strict';
import test from 'node:test';

import { makeTime } from '../lib/time.js';
import {
  baysWords, batteryWords, bayGroups, bayState, cacheSize, controllerRows, controllersOf, diskHealth, diskTables, failedAt, headPill, isAnswer,
  kindWords, levelWords, listWords, raidBanners, raidVolumes, replaceSteps, stackOf, timeLeftWords, volumeCards, volumeNameOf, volumeTask, WRITES_STRAIGHT,
} from '../pages/storage/raid-model.js';
import { show } from './storage-fixtures.mjs';
import {
  battery, controller, entry, failedBay, healthy, NOW, noBattery, noController, offlineVolume, raidDisk, raidVolume, ssd, writeThrough,
} from './storage-raid-fixtures.mjs';

const time = makeTime('UTC', () => NOW * 1000);
const duration = time.duration;
const first = (raid) => raid.controllers[0];
const row = (rows, label) => rows.find((one) => one.label === label);

test('an answer with no controller says nothing, and what is not an answer is no controller', () => {
  assert.equal(isAnswer(noController()), true);
  assert.equal(isAnswer(null), false);
  assert.equal(isAnswer({}), false);
  assert.deepEqual(controllersOf(undefined), []);
  assert.deepEqual(raidBanners(noController(), { time }), []);
  assert.deepEqual(controllerRows(noController()), []);
  assert.deepEqual(diskTables(noController(), time), []);
  assert.deepEqual(volumeCards(noController(), { duration }), []);
  assert.equal(headPill(noController()), null);
  assert.equal(raidVolumes(noController()).size, 0);
});

test('lists, bays and levels are said in plain words', () => {
  assert.equal(listWords([]), '');
  assert.equal(listWords([4]), '4');
  assert.equal(listWords([0, 1]), '0 and 1');
  assert.equal(listWords([2, 3, 5, 6]), '2, 3, 5 and 6');
  assert.equal(listWords([2, 3, 5, 6], 'or'), '2, 3, 5 or 6');
  const disks = first(failedBay()).disks;
  assert.equal(baysWords([disks[4]]), 'Bay 4');
  assert.equal(baysWords([disks[0], disks[1]]), 'Bays 0 and 1');
  assert.equal(baysWords([disks[2], disks[3], disks[5], disks[6]], 'or'), 'Bays 2, 3, 5 or 6');
  assert.equal(baysWords([]), 'No disks');
  assert.equal(baysWords([raidDisk(1, 3, { enclosure: null, index: null, bay: 'Slot 3' }), raidDisk(2, 4, { enclosure: null, index: null, bay: 'Slot 4' })]), 'Slots 3 and 4');
  assert.equal(baysWords([raidDisk(1, 0, { bay: 'Bay 0, backplane 1' }), raidDisk(2, 0, { bay: 'Bay 0, backplane 2', index: 1 })]), 'Bay 0, backplane 1 and Bay 0, backplane 2');
  assert.equal(levelWords('raid10'), 'RAID 10');
  assert.equal(levelWords('raid1e'), 'RAID 1E');
  assert.equal(levelWords('other'), 'RAID');
  assert.equal(volumeNameOf(raidVolume(0, 'boot', 'raid1', [])), 'boot');
  assert.equal(volumeNameOf(raidVolume(3, '', 'raid1', [])), 'volume 3');
  assert.equal(volumeNameOf(raidVolume(3, null, 'raid1', [])), 'volume 3');
});

test('the time left comes from the percent done and the time it took, and says so when there is no pace yet', () => {
  assert.equal(timeLeftWords(42, 3030, duration), 'about 1 h 10 min left');
  assert.equal(timeLeftWords(50, 600, duration), 'about 10 min left');
  assert.equal(timeLeftWords(99, 60, duration), 'less than a minute left');
  assert.equal(timeLeftWords(100, 60, duration), 'finishing');
  assert.equal(timeLeftWords(0, 0, duration), 'working out the time left');
  assert.equal(timeLeftWords(null, 20, duration), 'working out the time left');
  assert.equal(timeLeftWords(10, null, duration), 'working out the time left');
});

test('the cache is said as its label says it, in powers of two', () => {
  assert.equal(cacheSize(8_589_934_592), '8 GB');
  assert.equal(cacheSize(1_073_741_824), '1 GB');
  assert.equal(cacheSize(536_870_912), '512 MB');
  assert.equal(cacheSize(1_500_000_000), '1.5 GB');
  assert.equal(cacheSize(null), null);
  assert.equal(cacheSize(0), null);
});

test('a healthy controller has its mode, chip, firmware, cache, battery and temperature as rows', () => {
  const [one] = controllerRows(healthy());
  assert.equal(one.title, 'PERC H740P Mini · 5.16.0');
  assert.deepEqual(one.state, { word: 'Working', tone: 'ok', note: null });
  assert.deepEqual(one.rows.map((r) => r.label), ['Mode', 'Chip', 'Firmware', 'Cache', 'Battery', 'Temperature']);
  assert.equal(row(one.rows, 'Mode').value, 'RAID volumes and pass-through disks');
  assert.equal(row(one.rows, 'Chip').value, 'SAS3508');
  assert.equal(row(one.rows, 'Firmware').value, '5.16.0 (package 51.16.0-5150)');
  assert.equal(row(one.rows, 'Cache').value, '8 GB, writes held in the cache (write back)');
  assert.deepEqual(row(one.rows, 'Battery'), {
    label: 'Battery', value: 'Charged', tone: 'ok', note: 'Writes in the cache are kept if the power goes.', pill: true,
  });
  assert.equal(row(one.rows, 'Temperature').value, '61 °C');
});

test('the modes, an unknown chip and a controller with no firmware version are said as far as they are known', () => {
  const rows = (parts) => controllerRows({ controllers: [entry({ controller: controller(parts) })] })[0].rows;
  assert.equal(row(rows({ mode: 'raid' }), 'Mode').value, 'RAID: its disks are in volumes');
  assert.equal(row(rows({ mode: 'hba' }), 'Mode').value, 'Pass-through: its disks are used as they are');
  assert.equal(row(rows({ mode: 'weird' }), 'Mode').value, 'Not known');
  assert.equal(row(rows({ chip: null }), 'Chip').value, 'Not known');
  assert.equal(row(rows({ firmware: null }), 'Firmware').value, 'package 51.16.0-5150');
  assert.equal(row(rows({ firmware: null, package: null }), 'Firmware').value, 'Not known');
  assert.equal(row(rows({ temperature: null }), 'Temperature'), undefined);
  assert.equal(row(rows({ cache: null }), 'Cache').value, 'No cache memory');
  assert.equal(controllerRows({ controllers: [entry({ controller: controller({ model: '', firmware: null, package: null }) })] })[0].title, 'RAID controller');
});

test('a controller in fault says so, with the firmware\'s code, and the page says it in a banner', () => {
  const raid = { controllers: [entry({ controller: controller({ state: 'fault', fault: '0x0f41' }) })] };
  const [one] = controllerRows(raid);
  assert.deepEqual(one.state, { word: 'Fault', tone: 'bad', note: 'The firmware reports the fault 0x0f41.' });
  assert.deepEqual(one.rows[0], { label: 'State', value: 'Fault', tone: 'bad', note: 'The firmware reports the fault 0x0f41.', pill: true });
  const [banner] = raidBanners(raid, { time });
  assert.equal(banner.tone, 'bad');
  assert.equal(banner.lead, 'The RAID controller PERC H740P Mini reports a fault.');
  assert.match(banner.detail, /fault 0x0f41/);
  const unnamed = { controllers: [entry({ controller: controller({ state: 'fault', fault: null }) })] };
  assert.equal(controllerRows(unnamed)[0].state.note, 'The firmware reports a fault.');
});

test('the cache says whether writes are held in it from the policy of the volumes', () => {
  const cache = (volumes) => row(controllerRows({ controllers: [entry({ volumes })] })[0].rows, 'Cache').value;
  assert.equal(cache([]), '8 GB');
  assert.equal(cache([raidVolume(0, 'a', 'raid1', [], { write: 'through' })]), '8 GB, writes go straight to disk (write through)');
  assert.equal(cache([raidVolume(0, 'a', 'raid1', []), raidVolume(1, 'b', 'raid1', [], { write: 'through' })]), '8 GB, writes held in the cache for some volumes and not for others');
});

test('the battery says its state, and when it makes writes go straight to disk it says until when', () => {
  const words = (state, more = {}, volumes = [raidVolume(0, 'a', 'raid1', [])]) => batteryWords(battery({ state, ...more }), volumes);
  assert.deepEqual(words('charging', { charge: 40, write_through: true }), {
    present: true, word: 'Charging, 40%', tone: 'warn', note: `${WRITES_STRAIGHT}.`, writeThrough: true,
  });
  assert.equal(words('learning', { write_through: true }).note, 'Writes go straight to disk until the battery charges.');
  assert.equal(words('failed', { write_through: true }).note, 'Writes go straight to disk until the battery is replaced.');
  assert.equal(words('missing', { write_through: true }).note, 'Writes go straight to disk until the battery is replaced.');
  assert.deepEqual([words('failed').word, words('failed').tone], ['Failed', 'bad']);
  assert.deepEqual([words('missing').word, words('missing').tone], ['Not found', 'bad']);
  assert.deepEqual([words('degraded').word, words('degraded').tone], ['Worn', 'warn']);
  assert.deepEqual([words('discharging').word, words('discharging').tone], ['Running on the battery', 'warn']);
  assert.deepEqual([words('learning').word, words('learning').tone], ['Learning', 'off']);
  assert.deepEqual([words('optimal', { write_through: true }).tone], ['warn']);
  assert.equal(words('strange').word, 'strange');
  assert.equal(words('charging', { charge: null }).word, 'Charging');
});

test('a controller with no battery says so, and warns when a volume holds writes in the cache', () => {
  const [one] = controllerRows(noBattery());
  assert.deepEqual(row(one.rows, 'Battery'), {
    label: 'Battery', value: 'None', tone: 'warn', note: 'Writes held in the cache are lost if the power goes.', pill: false,
  });
  const quiet = batteryWords(null, [raidVolume(0, 'a', 'raid1', [], { write: 'through' })]);
  assert.deepEqual([quiet.tone, quiet.note], ['off', 'No battery or cache vault is fitted.']);
});

test('a battery that forces write through shows in the battery row and the cache row', () => {
  const [one] = controllerRows(writeThrough());
  assert.equal(row(one.rows, 'Battery').value, 'Charging, 40%');
  assert.equal(row(one.rows, 'Battery').note, 'Writes go straight to disk until the battery charges.');
  assert.equal(row(one.rows, 'Cache').value, '8 GB, writes go straight to disk (write through)');
  const [volume] = volumeCards(writeThrough(), { duration })[0].rows;
  assert.equal(volume.cache, '64 KB stripe · set to write back, writing straight to disk now because of the battery · reads ahead');
});

test('each state of a disk is a bay with a class, a word and what is under it', () => {
  const entryOf = failedBay().controllers[0];
  const state = (disk) => bayState(entryOf, disk);
  assert.deepEqual(state(entryOf.disks[0]), { kind: 'use', word: 'In use', sub: 'boot · RAID 1' });
  assert.deepEqual(state(entryOf.disks[4]), { kind: 'fail', word: 'Failed', sub: 'was in data' });
  assert.deepEqual(state(entryOf.disks[6]), { kind: 'rebuild', word: 'Rebuilding', sub: '42% · into data' });
  assert.deepEqual(state(entryOf.disks[7]), { kind: 'ready', word: 'Ready', sub: 'not in a volume' });
  assert.deepEqual(state(raidDisk(9, 9, { state: 'hot-spare', spare: 'global' })), { kind: 'spare', word: 'Hot spare', sub: 'for any volume' });
  assert.deepEqual(state(raidDisk(9, 9, { state: 'hot-spare', spare: 'dedicated' })), { kind: 'spare', word: 'Hot spare', sub: 'for one volume' });
  assert.deepEqual(state(raidDisk(9, 9, { state: 'jbod', os: 'sdc' })), { kind: 'direct', word: 'Used directly', sub: 'as sdc' });
  assert.deepEqual(state(raidDisk(9, 9, { state: 'jbod' })), { kind: 'direct', word: 'Used directly', sub: 'without RAID' });
  assert.deepEqual(state(raidDisk(9, 9, { state: 'unconfigured-bad' })), { kind: 'fail', word: 'Not usable', sub: 'the controller marked it bad' });
  assert.deepEqual(state(raidDisk(9, 9, { state: 'offline', volume: 1 })), { kind: 'fail', word: 'Offline', sub: 'out of data' });
  assert.deepEqual(state(raidDisk(9, 9, { state: 'offline' })), { kind: 'fail', word: 'Offline', sub: 'taken out of its volume' });
  assert.deepEqual(state(raidDisk(9, 9, { state: 'copyback', volume: 1, percent: 10 })), { kind: 'rebuild', word: 'Copying back', sub: '10% · into data' });
  assert.deepEqual(state(raidDisk(9, 9, { state: 'other' })), { kind: 'off', word: 'Other', sub: 'state not known' });
  assert.deepEqual(state(raidDisk(9, 9, { state: 'failed' })), { kind: 'fail', word: 'Failed', sub: 'stopped working' });
  assert.deepEqual(state(raidDisk(9, 9, { state: 'online' })), { kind: 'use', word: 'In use', sub: 'in a volume' });
  assert.deepEqual(state(raidDisk(9, 9, { state: 'rebuilding' })), { kind: 'rebuild', word: 'Rebuilding', sub: 'rebuilding' });
});

test('the bays are the front of the server: a bay for each slot, a failed one red, a slot with no disk empty', () => {
  const [group] = bayGroups(first(failedBay())).groups;
  assert.equal(group.title, null);
  assert.deepEqual(group.bays.map((bay) => [bay.name, bay.kind, bay.word]), [
    ['Bay 0', 'use', 'In use'], ['Bay 1', 'use', 'In use'], ['Bay 2', 'use', 'In use'], ['Bay 3', 'use', 'In use'],
    ['Bay 4', 'fail', 'Failed'], ['Bay 5', 'use', 'In use'], ['Bay 6', 'rebuild', 'Rebuilding'], ['Bay 7', 'ready', 'Ready'],
  ]);
  assert.equal(group.bays[0].size, '480 GB SSD');
  assert.equal(group.bays[2].size, '1.2 TB SAS');
  assert.equal(group.bays[4].label, 'Bay 4: failed, was in data, 1.2 TB SAS');
  const gap = { controllers: [entry({ disks: [ssd(0, 0, { volume: 0 }), ssd(1, 3, { volume: 0 })], volumes: [raidVolume(0, 'boot', 'raid1', [0, 1])] })] };
  const bays = bayGroups(first(gap)).groups[0].bays;
  assert.deepEqual(bays.map((bay) => [bay.name, bay.empty, bay.word]), [['Bay 0', false, 'In use'], ['Bay 1', true, 'Empty'], ['Bay 2', true, 'Empty'], ['Bay 3', false, 'In use']]);
  assert.equal(bays[1].label, 'Bay 1: empty');
  assert.equal(bays[1].sub, null);
});

test('with two backplanes each has its row, and an empty bay is named as the others are; direct-attached disks are slots', () => {
  const two = {
    controllers: [entry({
      disks: [
        ssd(0, 0, { bay: 'Bay 0, backplane 1', index: 0, enclosure: 32 }),
        ssd(1, 2, { bay: 'Bay 2, backplane 1', index: 0, enclosure: 32 }),
        ssd(2, 0, { bay: 'Bay 0, backplane 2', index: 1, enclosure: 33 }),
      ],
    })],
  };
  const groups = bayGroups(first(two)).groups;
  assert.deepEqual(groups.map((group) => group.title), ['Backplane 1', 'Backplane 2']);
  assert.deepEqual(groups[0].bays.map((bay) => bay.name), ['Bay 0, backplane 1', 'Bay 1, backplane 1', 'Bay 2, backplane 1']);
  assert.equal(groups[0].bays[1].empty, true);
  const direct = { controllers: [entry({ disks: [ssd(0, 0, { enclosure: null, index: null, bay: 'Slot 0' }), ssd(1, 2, { enclosure: null, index: null, bay: 'Slot 2' })] })] };
  const slots = bayGroups(first(direct)).groups[0].bays;
  assert.deepEqual(slots.map((bay) => bay.name), ['Slot 0', 'Slot 1', 'Slot 2']);
  assert.deepEqual(bayGroups(first({ controllers: [entry()] })).groups, []);
});

test('a kind is the media and the connection', () => {
  assert.equal(kindWords(raidDisk(1, 1)), 'HDD · SAS');
  assert.equal(kindWords(ssd(1, 1)), 'SSD · SATA');
  assert.equal(kindWords(raidDisk(1, 1, { media: null, interface: null })), 'Not known');
  assert.equal(kindWords(raidDisk(1, 1, { media: 'ssd', interface: 'nvme' })), 'SSD · NVME');
});

test('a disk\'s health is what the controller reports, the worst sign first', () => {
  assert.deepEqual(diskHealth(raidDisk(1, 1)), { word: 'No errors', tone: 'ok', line: '35 °C · no media errors' });
  assert.deepEqual(diskHealth(raidDisk(1, 1, { media_errors: 2, other_errors: 1 })), { word: 'Media errors', tone: 'warn', line: '35 °C · 2 media errors · 1 other error' });
  assert.deepEqual(diskHealth(raidDisk(1, 1, { predictive: 3 })), { word: 'Predicted to fail', tone: 'bad', line: '35 °C · no media errors · 3 predictive failures' });
  assert.deepEqual(diskHealth(raidDisk(1, 1, { smart_alert: true })), { word: 'Expects to fail', tone: 'bad', line: 'The drive\'s own warning is on · 35 °C · no media errors' });
  assert.deepEqual(diskHealth(raidDisk(1, 1, { state: 'failed', temperature: null })), { word: 'Not answering', tone: 'bad', line: '' });
  assert.equal(diskHealth(raidDisk(1, 1, { temperature: null })).line, 'no media errors');
});

test('the failure time is the newest critical event that names the disk', () => {
  const entryOf = first(failedBay());
  assert.equal(failedAt(entryOf, entryOf.disks[4]), Date.parse('2026-11-14T09:14:00Z') / 1000);
  assert.equal(failedAt(entryOf, entryOf.disks[3]), null);
  const counted = { ...entryOf, events: [{ ...entryOf.events[1], time: null, boot_relative: 500 }] };
  assert.equal(failedAt(counted, entryOf.disks[4]), null);
});

test('the physical disks are listed by bay with the model, serial, kind, state and health', () => {
  const [table] = diskTables(failedBay(), time);
  assert.equal(table.rows.length, 8);
  const [bay0, , , bay3, bay4, , bay6, bay7] = table.rows;
  assert.deepEqual([bay0.bay, bay0.disk, bay0.details, bay0.kind], ['Bay 0', 'INTEL SSDSC2KB480G8, 480 GB', 'Serial PHYG0 · firmware XCV10132', 'SSD · SATA']);
  assert.deepEqual(bay0.state, { word: 'In use', tone: 'ok', note: 'RAID 1 boot' });
  assert.deepEqual(bay3.health, { word: 'Media errors', tone: 'warn', line: '36 °C · 2 media errors' });
  assert.deepEqual(bay4.state, { word: 'Failed', tone: 'bad', note: 'Stopped answering at 9:14 AM' });
  assert.equal(bay4.health.word, 'Not answering');
  assert.deepEqual(bay6.state, { word: 'Rebuilding, 42%', tone: 'warn', note: 'Now in RAID 5 data' });
  assert.deepEqual(bay7.state, { word: 'Ready', tone: 'off', note: 'In no volume' });
  const bare = diskTables({ controllers: [entry({ disks: [raidDisk(1, 1, { vendor: null, model: null, serial: null, revision: null, state: 'hot-spare', spare: 'dedicated' }), raidDisk(2, 2, { state: 'jbod', os: 'sdc' }), raidDisk(3, 3, { state: 'offline', volume: 0 })], volumes: [raidVolume(0, 'a', 'raid1', [])] })] }, time)[0].rows;
  assert.equal(bare[0].disk, '1.2 TB');
  assert.equal(bare[0].details, '');
  assert.equal(bare[0].state.note, 'Takes over for one volume');
  assert.equal(bare[1].state.note, 'Used directly as sdc');
  assert.equal(bare[2].state.note, 'Taken out of RAID 1 a');
});

test('the volumes have their level, size, state, bays, cache and the stack of what sits on them', () => {
  const [card] = volumeCards(healthy(), { storage: show(), duration });
  const [boot] = card.rows;
  assert.deepEqual([boot.name, boot.head, boot.state, boot.task, boot.note, boot.bays], ['boot', 'RAID 1 · 480 GB', { word: 'Optimal', tone: 'ok' }, null, null, 'Bays 0 and 1']);
  assert.equal(boot.cache, '64 KB stripe · writes held in the cache · reads ahead');
  assert.deepEqual(boot.stack, ['volume data', 'sda', 'RAID 1', 'Bays 0 and 1']);
  assert.deepEqual(stackOf(raidVolume(0, 'b', 'raid1', [], { os: 'sdq' }), [ssd(0, 0), ssd(1, 1)]), ['sdq', 'RAID 1', 'Bays 0 and 1']);
  assert.deepEqual(stackOf(raidVolume(0, 'b', 'raid1', [], { os: null }), [ssd(0, 0)]), ['not shown to the system', 'RAID 1', 'Bay 0']);
});

test('a volume that rebuilds says Rebuilding with the percent and the time left, and what one more failure does', () => {
  const [, data] = volumeCards(failedBay(), { storage: show(), duration })[0].rows;
  assert.deepEqual(data.state, { word: 'Rebuilding', tone: 'warn' });
  assert.deepEqual(data.task, { words: 'Rebuilding', percent: 42, left: 'about 1 h 10 min left', line: 'Rebuilding, 42%, about 1 h 10 min left' });
  assert.equal(data.note, 'Degraded until the rebuild ends: one more failure in bays 2, 3, 5 or 6 can lose the data.');
  assert.equal(data.bays, 'Bays 2, 3, 5 and 6 · bay 6, the hot spare, is taking the place of bay 4');
  assert.equal(data.cache, '256 KB stripe · writes held in the cache · reads ahead');
});

test('a degraded volume that nothing rebuilds says so, a RAID 6 may take more failures, an offline one is out of reach', () => {
  const alone = failedBay();
  const [one] = alone.controllers;
  one.disks[6] = raidDisk(6, 6, { volume: 1 });
  let data = volumeCards(alone, { duration })[0].rows[1];
  assert.deepEqual(data.state, { word: 'Degraded', tone: 'warn' });
  assert.equal(data.task, null);
  assert.equal(data.note, 'The volume has lost a disk, and nothing is rebuilding it: one more failure in bays 2, 3, 5 or 6 can lose the data.');
  one.volumes[1].level = 'raid6';
  one.volumes[1].state = 'partly-degraded';
  data = volumeCards(alone, { duration })[0].rows[1];
  assert.deepEqual(data.state, { word: 'Partly degraded', tone: 'warn' });
  assert.equal(data.note, 'Part of the volume has lost a disk, and nothing is rebuilding it: more failures in bays 2, 3, 5 or 6 can lose the data.');
  data = volumeCards(offlineVolume(), { duration })[0].rows[1];
  assert.deepEqual(data.state, { word: 'Offline', tone: 'bad' });
  assert.match(data.note, /^The controller cannot read this volume/);
  const odd = failedBay();
  odd.controllers[0].volumes[1].state = 'strange';
  assert.deepEqual(volumeCards(odd, { duration })[0].rows[1].state.word, 'Rebuilding');
});

test('a volume\'s own task is a bar with its percent: a check, a start-up, a reconstruction; one with no percent yet has no bar fill', () => {
  const task = (parts) => volumeTask(entry({ volumes: [] }), raidVolume(0, 'a', 'raid5', [], parts), duration);
  assert.equal(task({ task: 'none' }), null);
  assert.deepEqual(task({ task: 'check', percent: 30, elapsed: 600 }), { words: 'Checking consistency', percent: 30, left: 'about 23 min left', line: 'Checking consistency, 30%, about 23 min left' });
  assert.equal(task({ task: 'init', percent: 5, elapsed: 60 }).words, 'Preparing the volume');
  assert.equal(task({ task: 'background-init', percent: 5, elapsed: 60 }).words, 'Preparing the volume in the background');
  assert.equal(task({ task: 'reconstruct', percent: 5, elapsed: 60 }).words, 'Reconstructing');
  assert.deepEqual(task({ task: 'check', percent: null, elapsed: null }), { words: 'Checking consistency', percent: null, left: null, line: 'Checking consistency' });
  assert.equal(task({ task: 'check', percent: 250, elapsed: 60 }).percent, 100);
  assert.equal(task({ task: 'mystery', percent: 5, elapsed: 60 }).words, 'Working');
  const withCopyback = entry({ disks: [raidDisk(1, 1, { volume: 0, task: 'copyback', percent: 20, elapsed: 100 })], volumes: [raidVolume(0, 'a', 'raid1', [1])] });
  assert.equal(volumeTask(withCopyback, withCopyback.volumes[0], duration).words, 'Copying back');
});

test('the banner for a failed disk is as the design words it, with the hot spare that took its place', () => {
  const [banner] = raidBanners(failedBay(), { time });
  assert.equal(banner.tone, 'bad');
  assert.equal(banner.lead, 'The disk in bay 4 failed at 9:14 AM.');
  assert.equal(banner.detail, 'The hot spare in bay 6 took its place, and the volume data is rebuilding onto it: 42%, about 1 h 10 min left. Until the rebuild ends, one more failure in data loses it.');
  assert.deepEqual(banner.replace, { n: 0, bays: ['Bay 4'] });
});

test('the banner does not say hot spare when the answer does not, and names no time when no event does', () => {
  const raid = failedBay();
  raid.controllers[0].disks[6].spare = null;
  raid.controllers[0].events = [];
  const [banner] = raidBanners(raid, { time });
  assert.equal(banner.lead, 'The disk in bay 4 failed.');
  assert.match(banner.detail, /^The disk in bay 6 took its place, and the volume data is rebuilding onto it: 42%/);
});

test('a failed disk that the volume no longer names still has its banner, and a failed disk in no volume puts no data at risk', () => {
  const raid = failedBay();
  raid.controllers[0].disks[4].volume = null;
  assert.match(raidBanners(raid, { time })[0].detail, /^The hot spare in bay 6 took its place/);
  const none = { controllers: [entry({ disks: [raidDisk(1, 1, { state: 'failed' })] })] };
  assert.equal(raidBanners(none, { time })[0].detail, 'It was in no volume, so no data is at risk. Replace it when you can.');
});

test('a failed disk with no spare says the volume stays degraded; two failed disks are said together', () => {
  const raid = failedBay();
  const [one] = raid.controllers;
  one.disks[6] = raidDisk(6, 6, { volume: 1 });
  assert.equal(raidBanners(raid, { time })[0].detail, 'No spare took its place, so the volume data stays degraded until the disk is replaced. One more failure in it can lose the data.');
  one.disks[7] = raidDisk(7, 7, { state: 'failed', volume: 1 });
  const [banner] = raidBanners(raid, { time });
  assert.equal(banner.lead, '2 disks failed: bay 4 and bay 7.');
  assert.deepEqual(banner.replace.bays, ['Bay 4', 'Bay 7']);
});

test('an offline volume is the worst banner, a degraded one with no failed disk has its own, and a healthy machine has none', () => {
  const [offline, failed] = raidBanners(offlineVolume(), { time });
  assert.equal(offline.lead, 'The volume data is offline.');
  assert.equal(offline.tone, 'bad');
  assert.match(offline.detail, /out of reach until enough of its disks are back\. Its disks are in bays 2, 3, 5 and 6\.$/);
  assert.equal(failed.lead, '2 disks failed: bay 4 and bay 6.');
  const degraded = healthy();
  degraded.controllers[0].volumes[0].state = 'degraded';
  const [banner] = raidBanners(degraded, { time });
  assert.equal(banner.tone, 'warn');
  assert.equal(banner.lead, 'The volume boot is degraded.');
  assert.equal(banner.detail, 'It has lost a disk and nothing is rebuilding it, so one more failure in bays 0 or 1 can lose the data. Look at the disks below for the one that is missing.');
  const checking = healthy();
  checking.controllers[0].volumes[0].state = 'degraded';
  checking.controllers[0].volumes[0].task = 'reconstruct';
  checking.controllers[0].volumes[0].percent = 10;
  checking.controllers[0].volumes[0].elapsed = 100;
  assert.match(raidBanners(checking, { time })[0].detail, /^Reconstructing, 10%, about 15 min left\. Until it ends, one more failure in bays 0 or 1 can lose the data\.$/);
  assert.deepEqual(raidBanners(healthy(), { time }), []);
  assert.deepEqual(raidBanners(writeThrough(), { time }), []);
  assert.deepEqual(raidBanners(noBattery(), { time }), []);
});

test('the pill by the title is the failed bay, else the volume that is not in order', () => {
  assert.deepEqual(headPill(failedBay()), { words: 'Bay 4 failed', kind: 'bad' });
  const two = failedBay();
  two.controllers[0].disks[7] = raidDisk(7, 7, { state: 'failed' });
  assert.deepEqual(headPill(two), { words: '2 disks failed', kind: 'bad' });
  const degraded = healthy();
  degraded.controllers[0].volumes[0].state = 'degraded';
  assert.deepEqual(headPill(degraded), { words: 'Volume degraded', kind: 'warn' });
  degraded.controllers[0].volumes[0].state = 'offline';
  assert.deepEqual(headPill(degraded), { words: 'Volume offline', kind: 'bad' });
  assert.equal(headPill(healthy()), null);
});

test('a disk that is a RAID volume is known by the name the system gives it, with where its disks are', () => {
  const found = raidVolumes(failedBay());
  assert.deepEqual([...found.keys()], ['sda', 'sdb']);
  const data = found.get('sdb');
  assert.deepEqual(data.health, { word: 'Degraded', tone: 'warn', note: 'A RAID 5 volume on PERC H740P Mini. Its health is the controller\'s: degraded.' });
  assert.equal(data.bays, 'Bays 2, 3, 5 and 6');
  assert.equal(data.line, 'A RAID 5 volume on PERC H740P Mini, made of bays 2, 3, 5 and 6');
  assert.deepEqual(data.failedBays, ['Bay 4']);
  assert.equal(data.replace, true);
  assert.equal(found.get('sda').replace, false);
  const passed = raidVolumes(healthy());
  assert.equal(passed.has('sdb'), false);
  assert.equal(raidVolumes({ controllers: [entry({ volumes: [raidVolume(0, 'x', 'raid1', [], { os: null })] })] }).size, 0);
});

test('the steps to replace a failed disk are at the bay, name it, and promise nothing the page cannot do', () => {
  const volume = raidVolumes(failedBay()).get('sdb');
  const { intro, steps } = replaceSteps({ bays: ['Bay 4'], volume });
  assert.match(intro, /^This disk is a RAID volume, so you replace the failed disk in its bay, not the volume\. Its disks are in bays 2, 3, 5 and 6\./);
  assert.equal(steps.length, 5);
  assert.match(steps[2], /^Pull the failed disk from bay 4\. Do not pull a disk from a bay that shows In use/);
  assert.match(steps[4], /which this page cannot do yet\.$/);
  assert.match(replaceSteps({ volume }).steps[2], /from bay 4\./);
  assert.match(replaceSteps({}).steps[2], /from the bay that shows as failed\./);
  assert.match(replaceSteps({ bays: ['Bay 4', 'Bay 7'] }).steps[2], /from bay 4 and bay 7\./);
  assert.doesNotMatch(replaceSteps({}).intro, /undefined|Its disks/);
});
