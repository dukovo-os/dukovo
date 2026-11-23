// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Storage page with a RAID controller, on a fake document: the banner, the Controllers,
// Bays, RAID volumes and Physical disks cards drawn from the `storage.raid` answer, the page as
// it was when there is no controller or the answer cannot be read, and what the cards of the
// disks say of a disk that is a RAID volume. The page has no button that changes a controller.

import assert from 'node:assert/strict';
import test from 'node:test';

import { buttonOf, openStorage, refusal, settle, text } from './storage-raid-view.mjs';
import {
  failedBay, healthy, noBattery, noController, offlineVolume, writeThrough,
} from './storage-raid-fixtures.mjs';

const RAID = '/api/1/storage/raid';
const raidWorld = (answer) => ({ reads: { [RAID]: answer } });
const CARDS = ['controllers', 'bays', 'volumes', 'physical'];
/** The words of buttons that change a controller, which come in later steps. */
const CHANGES = /Create|Delete|hot spare|Blink|locate|Import|Rename|Cache policy|Check consistency|Take offline|Bring online|Use without RAID/;

test('a machine with no controller shows nothing of RAID, and the page is as it was', async (context) => {
  const world = await openStorage(context, { world: raidWorld(noController()) });
  for (const id of CARDS) assert.equal(world.card(id), null, id);
  assert.equal(world.view.main.querySelector('.sto-banner .banner'), null);
  assert.equal(world.head().pill, null);
  assert.deepEqual(world.problems, []);
  assert.ok(world.calls.includes(RAID));
  assert.match(text(world.view.main), /Disks, drawn to scale/);
  assert.doesNotMatch(text(world.view.main), /Controllers|Physical disks|RAID volumes/);
});

test('a route that admind does not have, or that fails, leaves the page as it was and says nothing', async (context) => {
  for (const failure of [refusal('ADM-404', 'no route'), new Error('down')]) {
    const world = await openStorage(context, { world: raidWorld(failure) });
    assert.equal(world.card('controllers'), null);
    assert.deepEqual(world.problems, []);
    assert.match(text(world.view.main), /Disks, drawn to scale/);
  }
});

test('a healthy controller draws its four cards, with no banner and no pill, and none of them can change the machine', async (context) => {
  const world = await openStorage(context, { world: raidWorld(healthy()) });
  assert.deepEqual(world.problems, []);
  const cards = world.view.main.querySelectorAll('section.sto-raid-card');
  assert.deepEqual(cards.map((one) => one.id), CARDS);
  assert.deepEqual(cards.map((one) => one.querySelector('h2').textContent), ['Controllers', 'Bays', 'RAID volumes', 'Physical disks']);
  assert.equal(world.view.main.querySelector('.sto-banner .banner'), null);
  assert.equal(world.head().pill, null);
  const root = world.view.main.querySelector('.sto-raid');
  assert.deepEqual(root.querySelectorAll('button'), []);
  assert.deepEqual(root.querySelectorAll('a'), []);
  assert.doesNotMatch(text(root), CHANGES);
  assert.match(text(world.card('controllers')), /PERC H740P Mini · 5\.16\.0 Working Mode RAID volumes and pass-through disks Chip SAS3508 Firmware 5\.16\.0 \(package 51\.16\.0-5150\) Cache 8 GB, writes held in the cache \(write back\) Battery Charged Writes in the cache are kept if the power goes\. Temperature 61 °C/);
  assert.match(text(world.card('physical')), /Health read through the controller/);
});

test('the bays are drawn from the slots: eight bays, the failed one red, the rebuilding one dashed, and each says its state in words', async (context) => {
  const world = await openStorage(context, { world: raidWorld(failedBay()) });
  const bays = world.card('bays').querySelectorAll('li.bay');
  assert.equal(bays.length, 8);
  assert.deepEqual(bays.map((one) => [...one.classes].filter((name) => name !== 'bay')[0]), ['use', 'use', 'use', 'use', 'fail', 'use', 'rebuild', 'ready']);
  const failed = bays[4];
  assert.equal(failed.getAttribute('data-bay'), 'Bay 4');
  assert.match(text(failed), /^Bay 4 Failed was in data 1\.2 TB SAS/);
  assert.match(text(bays[6]), /^Bay 6 Rebuilding 42% · into data/);
  assert.equal(world.card('bays').querySelector('ul.sto-bays').style.values.get('--bays'), '8');
  assert.equal(world.card('bays').querySelector('ul.sto-bays').getAttribute('aria-label'), 'Drive bays, front of the server');
  assert.match(text(world.card('bays').querySelector('.sto-legend')), /In use Rebuilding Failed Hot spare Ready Empty/);
});

test('a slot with no disk is drawn empty', async (context) => {
  const raid = healthy();
  raid.controllers[0].disks.splice(1, 1);
  const world = await openStorage(context, { world: raidWorld(raid) });
  const bays = world.card('bays').querySelectorAll('li.bay');
  assert.deepEqual(bays.map((one) => text(one)), ['Bay 0 In use boot · RAID 1 480 GB SSD', 'Bay 1 Empty', 'Bay 2 Used directly as sdb 480 GB SSD']);
  assert.ok(bays[1].classes.has('empty'));
});

test('a failed disk puts the banner at the top with the pill by the title, as the design words them', async (context) => {
  const world = await openStorage(context, { world: raidWorld(failedBay()) });
  const banner = world.view.main.querySelector('.sto-banner .banner');
  assert.ok(banner.classes.has('bad'));
  assert.equal(text(banner).replace(' How to replace the failed disk', ''), 'The disk in bay 4 failed at 9:14 AM. The hot spare in bay 6 took its place, and the volume data is rebuilding onto it: 42%, about 1 h 10 min left. Until the rebuild ends, one more failure in data loses it.');
  assert.deepEqual(world.head().pill, { words: 'Bay 4 failed', kind: 'bad' });
  assert.deepEqual(world.heads.map((one) => one.pill?.words ?? null), [null, 'Bay 4 failed']);
  assert.deepEqual(banner.querySelectorAll('button').map((one) => one.textContent), ['How to replace the failed disk']);
});

test('the volumes card has the task bar with its percent and time left, the state in words and the stack line', async (context) => {
  const world = await openStorage(context, { world: raidWorld(failedBay()) });
  const volumes = world.card('volumes').querySelectorAll('.sto-volume');
  assert.deepEqual(volumes.map((one) => one.getAttribute('data-volume')), ['boot', 'data']);
  const data = volumes[1];
  assert.match(text(data), /^data RAID 5 · 3\.6 TB Rebuilding Rebuilding, 42%, about 1 h 10 min left Degraded until the rebuild ends: one more failure in bays 2, 3, 5 or 6 can lose the data\./);
  const bar = data.querySelector('[role="progressbar"]');
  assert.equal(bar.getAttribute('aria-valuenow'), '42');
  assert.equal(bar.getAttribute('aria-valuemax'), '100');
  assert.equal(bar.querySelector('i').style.values.get('--v'), '42');
  assert.ok(bar.getAttribute('aria-labelledby'));
  assert.match(text(data.querySelector('.sto-stack')), /^volume data →, then sdb →, then RAID 5 →, then Bays 2, 3, 5 and 6$/);
  assert.match(text(data), /Stripe and cache 256 KB stripe · writes held in the cache · reads ahead/);
  assert.equal(volumes[0].querySelector('[role="progressbar"]'), null);
  assert.match(text(volumes[0]), /Optimal/);
});

test('a task with no percent yet is a bar that waits, with no value', async (context) => {
  const raid = healthy();
  Object.assign(raid.controllers[0].volumes[0], { task: 'check', percent: null, elapsed: null });
  const world = await openStorage(context, { world: raidWorld(raid) });
  const bar = world.card('volumes').querySelector('[role="progressbar"]');
  assert.ok(bar.classes.has('wait'));
  assert.equal(bar.getAttribute('aria-valuenow'), null);
  assert.match(text(world.card('volumes')), /Checking consistency/);
});

test('the physical disks are a table by bay with the health the controller reports', async (context) => {
  const world = await openStorage(context, { world: raidWorld(failedBay()) });
  const table = world.card('physical').querySelector('table');
  assert.deepEqual(table.querySelectorAll('thead th').map((one) => one.textContent), ['Bay', 'Disk', 'Kind', 'State', 'Health']);
  const rows = table.querySelectorAll('tbody tr');
  assert.equal(rows.length, 8);
  assert.match(text(rows[3]), /^Bay 3 SEAGATE ST1200MM0099, 1\.2 TB Serial WFK0A3 · firmware ST31 HDD · SAS In use RAID 5 data Media errors 36 °C · 2 media errors$/);
  assert.match(text(rows[4]), /^Bay 4 .* Failed Stopped answering at 9:14 AM Not answering$/);
  assert.equal(rows[0].querySelector('th').getAttribute('scope'), 'row');
  assert.ok(table.classes.has('stack'));
  assert.equal(rows[0].querySelectorAll('td').every((one) => one.getAttribute('data-label')), true);
});

test('a controller with no battery and one that forces write through each say so in the Controllers card', async (context) => {
  const none = await openStorage(context, { world: raidWorld(noBattery()) });
  assert.match(text(none.card('controllers')), /Battery None Writes held in the cache are lost if the power goes\./);
  const through = await openStorage(context, { world: raidWorld(writeThrough()) });
  assert.match(text(through.card('controllers')), /Cache 8 GB, writes go straight to disk \(write through\) Battery Charging, 40% Writes go straight to disk until the battery charges\./);
  assert.equal(through.view.main.querySelector('.sto-banner .banner'), null);
});

test('an offline volume is the banner and the pill', async (context) => {
  const world = await openStorage(context, { world: raidWorld(offlineVolume()) });
  assert.match(text(world.view.main.querySelector('.sto-banner')), /^The volume data is offline\./);
  assert.equal(world.head().pill.words, '2 disks failed');
});

test('a read that fails later leaves the cards, and a controller that is gone takes them away', async (context) => {
  let answer = failedBay();
  const world = await openStorage(context, { world: raidWorld(() => answer) });
  assert.equal(world.card('bays').querySelectorAll('li.bay').length, 8);
  answer = new Error('down');
  world.answers[RAID] = () => { throw answer; };
  await world.tick(30_000);
  assert.equal(world.card('bays').querySelectorAll('li.bay').length, 8);
  assert.deepEqual(world.problems, []);
  world.answers[RAID] = () => noController();
  await world.tick(30_000);
  assert.equal(world.card('bays'), null);
  assert.equal(world.view.main.querySelector('.sto-banner .banner'), null);
  assert.equal(world.head().pill, null);
});

test('a bay that changes is drawn again, and the page keeps what it drew when nothing changed', async (context) => {
  const world = await openStorage(context, { world: raidWorld(() => healthy()) });
  const before = world.card('bays').querySelector('li.bay');
  await world.tick(30_000);
  assert.equal(world.card('bays').querySelector('li.bay'), before);
  const next = healthy();
  next.controllers[0].disks[0].state = 'failed';
  world.answers[RAID] = () => next;
  await world.tick(30_000);
  assert.ok(world.card('bays').querySelector('li.bay').classes.has('fail'));
  assert.equal(world.head().pill.words, 'Bay 0 failed');
});

test('the steps from the banner are words at the bay, for a failed disk of a RAID volume', async (context) => {
  const world = await openStorage(context, { world: raidWorld(failedBay()) });
  buttonOf(world.view.main.querySelector('.sto-banner'), 'How to replace the failed disk').click();
  await settle();
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Replace a failed disk');
  assert.match(text(dialog), /Pull the failed disk from bay 4\./);
  assert.doesNotMatch(text(dialog), /elevate|mirror/);
});

test('a reader sees the cards and the banner, and the steps, with no button that changes anything', async (context) => {
  const world = await openStorage(context, { session: { admin: false }, world: raidWorld(failedBay()) });
  assert.equal(world.card('bays').querySelectorAll('li.bay').length, 8);
  assert.deepEqual(world.view.main.querySelector('.sto-banner').querySelectorAll('button').map((one) => one.textContent), ['How to replace the failed disk']);
  assert.deepEqual(world.view.main.querySelector('.sto-raid').querySelectorAll('button'), []);
});

// ---- the cards of the disks ----

/** A machine whose RAID volume `sdc` is the stick of the page's fixtures, which keeps no health record of its own. */
const volumeSdc = (raid) => {
  raid.controllers[0].volumes[0].os = 'sdc';
  return raid;
};

test('a disk that is a RAID volume says so in the list, with the volume\'s state and not "Not checked"', async (context) => {
  const world = await openStorage(context, { world: raidWorld(volumeSdc(healthy())) });
  const row = world.view.main.querySelector('li.disk[data-disk="sdc"]');
  assert.match(text(row), /sdc: Optimal\. A RAID 1 volume on PERC H740P Mini\. Its health is the controller's: optimal\./);
  assert.doesNotMatch(text(row), /Not checked|does not report its health/);
  const other = world.view.main.querySelector('li.disk[data-disk="sda"]');
  assert.match(text(other), /Healthy/);
  const degraded = await openStorage(context, { world: raidWorld(volumeSdc(failedBay())) });
  assert.match(text(degraded.view.main.querySelector('li.disk[data-disk="sdc"]')), /sdc: Optimal\./);
  const bad = failedBay();
  bad.controllers[0].volumes[1].os = 'sdc';
  const worse = await openStorage(context, { world: raidWorld(bad) });
  assert.match(text(worse.view.main.querySelector('li.disk[data-disk="sdc"] .st')), /Degraded/);
});

test('the panel of a RAID volume\'s disk says whose it is and where its member bays are, has no self-test button, and gives the steps at the bay', async (context) => {
  const raid = failedBay();
  raid.controllers[0].volumes[1].os = 'sdc';
  const world = await openStorage(context, { world: raidWorld(raid), rest: 'sdc' });
  const panel = world.card('disk');
  const words = text(panel);
  assert.doesNotMatch(words, /keeps no health record|Not checked|does not report its health|USB or virtual/);
  assert.match(words, /Health Worth a look: Degraded: the controller's word for the volume/);
  assert.match(words, /On the controller data, RAID 5, PERC H740P Mini/);
  assert.match(words, /Member disks Bays 2, 3, 5 and 6\. See the bays/);
  assert.equal(panel.querySelectorAll('a').find((one) => one.textContent === 'See the bays').getAttribute('href'), '/storage#bays');
  assert.equal(buttonOf(panel, 'Check now'), undefined);
  assert.equal(panel.querySelector('[data-action="check"]'), null);
  assert.doesNotMatch(words, /Run one on the Hardware page|Last full check/);
  assert.equal(buttonOf(panel, 'Replace this disk…'), undefined);
  const steps = buttonOf(panel, 'How to replace the failed disk');
  steps.click();
  await settle();
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Replace a failed disk');
  assert.match(text(dialog), /Its disks are in bays 2, 3, 5 and 6\./);
  assert.match(text(dialog), /Pull the failed disk from bay 4\./);
  assert.doesNotMatch(text(dialog), /elevate|mirror/);
});

test('the panel of a RAID volume in good order has no steps button, and the page has no RAID cards beside a disk', async (context) => {
  const world = await openStorage(context, { world: raidWorld(volumeSdc(healthy())), rest: 'sdc' });
  assert.equal(buttonOf(world.card('disk'), 'How to replace the failed disk'), undefined);
  assert.match(text(world.card('disk')), /Health Optimal/);
  assert.equal(world.card('bays'), null);
  assert.equal(world.view.main.querySelector('.sto-raid').querySelectorAll('section').length, 0);
});

test('a disk that is not a RAID volume still says it does not report its health, as before', async (context) => {
  const world = await openStorage(context, { world: raidWorld(healthy()), rest: 'sdc' });
  assert.match(text(world.card('disk')), /Health Not checked: this disk does not report its health/);
});
