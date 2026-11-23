// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Hardware page on a fake document: what each card says for an
// administrator, a locked one and a reader, which action each button runs,
// and what the page does with a check that runs, a restart and a shut down.

import assert from 'node:assert/strict';
import test, { afterEach, beforeEach } from 'node:test';

import { ApiError } from '../api.js';
import { setUnlockSeconds } from '../unlock.js';
import {
  about, asked, bmcEvents, bmcOf, bmcSensors, broken, clean, devices, diskHealth, inventory, last, memtest, NOW, sensors, show, skipped, startTest, storage, task,
} from './hardware-fixtures.mjs';
import {
  buttonOf, openHardware, refusal, settle, useStore,
} from './hardware-view.mjs';

const realObserver = globalThis.MutationObserver;
beforeEach(() => {
  globalThis.MutationObserver = class { observe() {} disconnect() {} };
});
afterEach(() => {
  if (realObserver === undefined) delete globalThis.MutationObserver;
  else globalThis.MutationObserver = realObserver;
});

const text = (root) => root.textContent.replace(/\s+/g, ' ');

// The dialog is a module of its own, which the page reads when its button is pressed.
await import('../pages/hardware/check-dialog.js');

test('an administrator sees each card, and a read for each thing the page says', async (context) => {
  const world = await openHardware(context);
  assert.equal(world.view.headed.title, 'Hardware');
  assert.equal(world.view.headed.commands, true);
  assert.equal(world.view.headed.buttons.length, 1);
  assert.equal(world.view.headed.buttons[0].textContent, 'Check the hardware');
  for (const id of ['machine', 'check', 'memory', 'power']) assert.ok(world.card(id), id);
  assert.deepEqual(world.calls.slice(0, 7).sort(), ['/api/1/console/about', '/api/1/hardware', '/api/1/hardware/check', '/api/1/hardware/history', '/api/1/storage', '/api/1/storage/raid', '/api/1/tasks']);
  assert.ok(world.calls.includes('/api/1/hardware/inventory'), 'an unlocked administrator reads the modules at once');
  assert.ok(world.calls.includes('/api/1/power'));
});

test('This machine says what the machine is, from the firmware, the processor and the memory', async (context) => {
  const world = await openHardware(context);
  const card = text(world.card('machine'));
  assert.match(card, /ModelDell Inc\. OptiPlex 7040, mini tower/);
  assert.match(card, /ProcessorIntel Core i5-6500 · 4 cores, 3\.2 GHz, up to 3\.6 GHz/);
  assert.match(card, /Memory15\.6 GiB · in 2 of 4 slots · DDR4-2133 · no error correction/);
  assert.match(card, /FirmwareBIOS 1\.24\.0, Jul 12, 2023 · Firmware sets on the Updates page/);
  assert.match(card, /Up for4 hours 52 minutes/);
  assert.equal(world.card('machine').querySelectorAll('a').find((one) => one.textContent === 'Firmware sets on the Updates page').getAttribute('href'), '/updates#firmware');
  assert.match(text(world.card('machine').querySelector('details')), /DIMM A1 · 8 GiB · DDR4-2133 · Samsung M378A1K43BB2-CRC/);
});

test('a start that fell back says so beside how long the machine has run, with a link to what happened', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/console/about': about({ fallback: true, up: 3600 }) } } });
  assert.match(text(world.card('machine')), /Up for1 hour, since the update was taken back \(2:12 PM\) What happened/);
  assert.equal(world.card('machine').querySelectorAll('a').find((one) => one.textContent === 'What happened').getAttribute('href'), '/updates#happened');
});

test('a locked administrator reads the modules after unlocking, and a reader is told who sees them', async (context) => {
  const locked = await openHardware(context, { unlocked: false });
  assert.ok(!locked.calls.includes('/api/1/hardware/inventory'));
  assert.match(text(locked.card('machine')), /Memory15\.6 GiB Show the modules/);
  const reader = await openHardware(context, { session: { admin: false } });
  assert.ok(!reader.calls.includes('/api/1/hardware/inventory'));
  assert.match(text(reader.card('machine')), /Memory15\.6 GiB · An administrator sees the slots/);
  assert.equal(buttonOf(reader.card('machine'), 'Show the modules'), undefined);
});

test('the modules are read when the console unlocks', async (context) => {
  const world = await openHardware(context, { unlocked: false });
  assert.ok(!world.calls.includes('/api/1/hardware/inventory'));
  const { setUnlockSeconds } = await import('../unlock.js');
  setUnlockSeconds(900);
  await world.emit('unlock');
  assert.ok(world.calls.includes('/api/1/hardware/inventory'));
  assert.match(text(world.card('machine')), /in 2 of 4 slots/);
});

test('a machine that says nothing is said so, and a machine that cannot be read is too', async (context) => {
  const nothing = show({
    maker: null, model: null, chassis: null, processor: { model: null, cores: null, threads: null, base_mhz: null, max_mhz: null }, memory: { bytes: null, ecc: null, slots: null, modules: null }, firmware: { vendor: null, version: null, date: null },
  });
  const world = await openHardware(context, { unlocked: false, world: { reads: { '/api/1/hardware': nothing, '/api/1/console/about': {} } } });
  assert.match(text(world.card('machine')), /The firmware and the kernel say nothing about this machine\./);
  const error = refusal('ADM-129', 'hardware is not installed', 501);
  const broke = await openHardware(context, { world: { reads: { '/api/1/hardware': error } } });
  assert.match(text(broke.card('machine')), /What this machine is could not be read\./);
  assert.equal(broke.problems.length, 1);
});

test('the sentence under the head gives the last check and what diskhealth says of the disks', async (context) => {
  const world = await openHardware(context);
  const lead = text(world.view.main.querySelector('#hw-lead'));
  assert.match(lead, /^Last check Nov 9 2:10 AM \(a full check\): 6 passed, 1 warning \(HW-115\), 2 skipped\./);
  assert.match(lead, /diskhealth reads each disk's health every 10 minutes, but a sleeping one; sdb has a warning\.$/);
  assert.equal(world.view.main.querySelector('#hw-lead').querySelector('a').getAttribute('href'), '/storage/sdb');
});

test('the Last check card lists a failure first, with its ID linked and what to do, and what was skipped last', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/check': broken() } } });
  const items = world.card('check').querySelectorAll('li').filter((one) => one.classes.has('hw-check'));
  assert.deepEqual(items.map((one) => text(one).split(' · ')[0]), ['Failed sda read scan', 'Failed memory test', 'Warning sdb read scan', 'Passed processor load']);
  assert.match(text(items[0]), /sector 7 cannot be readHW-113 Replace the disk\./);
  const link = items[0].querySelector('a');
  assert.equal(link.getAttribute('href'), '/help/errors/HW-113');
  assert.match(text(world.card('check').querySelector('h2')), /Last check/);
  assert.equal(world.card('check').querySelector('.pill').textContent, 'Failed');
  const all = await openHardware(context);
  const last2 = all.card('check').querySelectorAll('li').filter((one) => one.classes.has('hw-check'));
  assert.match(text(last2.at(-1)), /^Skipped net1 link · networkno cable/);
    assert.equal(all.card('check').querySelector('.pill').textContent, 'Warning');
});

test('the checks before the last one are in a list that opens, the newest first, without the last one itself', async (context) => {
  const world = await openHardware(context);
  const earlier = world.card('check').querySelector('details');
  assert.equal(earlier.querySelector('summary').textContent, 'Earlier checks');
  assert.deepEqual(earlier.querySelectorAll('li').map((one) => text(one)), [
    'Nov 5 7:00 AM · a quick check · 7 passed, 1 warning, 1 skipped · HW-115',
    'Nov 2 7:00 AM · a quick check · 8 passed, 1 skipped',
  ]);
  const only = await openHardware(context, { world: { reads: { '/api/1/hardware/history': [] } } });
  assert.equal(only.card('check').querySelector('details'), null);
});

test('no check kept says so, and a reader is told who may check', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/check': null, '/api/1/hardware/history': [] } } });
  assert.match(text(world.card('check')), /Not checked yet\./);
  assert.ok(!text(world.card('check')).includes('An administrator can check'));
  assert.equal(text(world.view.main.querySelector('#hw-lead')).startsWith('diskhealth'), true);
  const reader = await openHardware(context, { session: { admin: false }, world: { reads: { '/api/1/hardware/check': null, '/api/1/hardware/history': [] } } });
  assert.match(text(reader.card('check')), /Not checked yet\.An administrator can check the hardware\./);
  assert.deepEqual(reader.view.headed.buttons, []);
});

test('a result in which nothing could be checked says so', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/check': skipped() } } });
  assert.match(text(world.view.main.querySelector('#hw-lead')), /: 1 skipped\./);
  assert.equal(world.card('check').querySelector('.pill').textContent, 'Skipped');
  const none = await openHardware(context, { world: { reads: { '/api/1/hardware/check': last({ checks: [] }) } } });
  assert.match(text(none.view.main.querySelector('#hw-lead')), /nothing could be checked\./);
});

test('a last check that cannot be read says so, and is never "Not checked yet"', async (context) => {
  const error = refusal('ADM-129', 'hardware is not installed', 501);
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/check': error } } });
  assert.match(text(world.card('check')), /The last check could not be read\./);
  assert.ok(!text(world.card('check')).includes('Not checked yet'));
  assert.match(text(world.card('memory')), /Last run: could not be read/);
  assert.equal(world.problems.length, 1);
});

test('Check the hardware asks which areas, and a quick check is all of them', async (context) => {
  const world = await openHardware(context, { world: { answers: { 'hardware.check': { ok: true, task: 41, result: { task: 41 } } } } });
  world.view.headed.buttons[0].click();
  await settle();
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Check the hardware');
  assert.match(dialog.textContent, /A quick check takes about 3 minutes/);
  assert.equal(dialog.querySelectorAll('input').filter((one) => one.getAttribute('type') === 'checkbox').length, 9, 'eight areas and the full check');
  assert.ok(buttonOf(dialog, 'Check now'));
  await world.submit();
  assert.deepEqual(world.ran('hardware.check'), [{}]);
  assert.equal(dialog.open, false);
  assert.match(text(world.view.main.querySelector('#hw-lead')), /^Checking the hardware\. Running/);
});

test('a check of some areas names them, and a full check says so', async (context) => {
  const world = await openHardware(context, { world: { answers: { 'hardware.check': { ok: true, task: 41, result: { task: 41 } } } } });
  world.view.headed.buttons[0].click();
  await settle();
  const boxes = world.dialog().querySelectorAll('input').filter((one) => one.getAttribute('type') === 'checkbox');
  boxes[2].checked = false;
  boxes[4].checked = false;
  boxes[8].checked = true;
  await world.submit();
  assert.deepEqual(world.ran('hardware.check'), [{ areas: 'memory disks sensors network errors raid', full: true }]);
});

test('a dialog with no area chosen sends nothing and says so', async (context) => {
  const world = await openHardware(context);
  world.view.headed.buttons[0].click();
  await settle();
  for (const box of world.dialog().querySelectorAll('input')) box.checked = false;
  await world.submit();
  assert.deepEqual(world.ran('hardware.check'), []);
  assert.match(text(world.dialog()), /Choose at least one thing to check\./);
  assert.equal(world.dialog().open, true);
});

test('a full check shows its words when its box is on', async (context) => {
  const world = await openHardware(context);
  world.view.headed.buttons[0].click();
  await settle();
  const words = world.dialog().querySelectorAll('p').find((one) => one.textContent.startsWith('A full check takes hours'));
  assert.equal(words.hidden, true);
  const full = world.dialog().querySelectorAll('input').at(-1);
  full.checked = true;
  full.fire('change');
  assert.equal(words.hidden, false);
});

test('while a check runs its step is under the head and the buttons that start one are off', async (context) => {
  const running = task();
  const world = await openHardware(context, { world: { reads: { '/api/1/tasks': { tasks: [running] } } } });
  assert.equal(text(world.view.main.querySelector('#hw-lead')), 'Checking the hardware. Step 4 of 11: disks sda self-test passed');
  assert.equal(world.view.headed.buttons[0].getAttribute('aria-disabled'), 'true');
  assert.equal(buttonOf(world.card('memory'), 'Test part of the memory').getAttribute('aria-disabled'), 'true');
  world.view.headed.buttons[0].click();
  await settle();
  assert.equal(world.dialog(), null, 'no second check is offered');
  await world.press(world.card('memory'), 'Test part of the memory');
  assert.deepEqual(world.ran('hardware.check'), []);
});

test('the steps of the task come from its events, and the page reads the last check again when it ends', async (context) => {
  const running = task();
  const world = await openHardware(context, { world: { reads: { '/api/1/tasks': { tasks: [running] } } } });
  await world.emit('task', { id: 41, action: 'hardware.check', state: 'running', step: 5, steps: 11, words: 'disks sdb self-test passed' });
  assert.match(text(world.view.main.querySelector('#hw-lead')), /Step 5 of 11: disks sdb self-test passed/);
  const before = world.calls.filter((path) => path === '/api/1/hardware/check').length;
  world.answers['/api/1/tasks'] = { tasks: [] };
  world.answers['/api/1/hardware/check'] = clean();
  await world.emit('task', { id: 41, action: 'hardware.check', state: 'done' });
  assert.equal(world.calls.filter((path) => path === '/api/1/hardware/check').length, before + 1);
  assert.match(text(world.view.main.querySelector('#hw-lead')), /^Last check Nov 9 2:10 AM \(a quick check\): 2 passed\./);
  assert.equal(world.view.headed.buttons[0].getAttribute('aria-disabled'), null);
  assert.match(text(world.view.main.querySelector('[role="status"]')), /The check is done\./);
});

test('a check that did not finish says so in the Last check card, and a task of another action is left alone', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/tasks': { tasks: [task()] } } } });
  await world.emit('task', { id: 99, action: 'updates.apply', state: 'running' });
  assert.match(text(world.view.main.querySelector('#hw-lead')), /Step 4 of 11/);
  world.answers['/api/1/tasks'] = { tasks: [task({ state: 'failed', ended: Math.floor(NOW / 1000), note: 'hardware ended with status 3', status: 3 })] };
  await world.emit('task', { id: 41, action: 'hardware.check', state: 'failed' });
  assert.match(text(world.card('check')), /The check did not finish\. Failed: hardware ended with status 3/);
  assert.match(text(world.view.main.querySelector('[role="status"]')), /The check did not finish\./);
});

test('a check that was started by another person shows its steps too', async (context) => {
  const world = await openHardware(context);
  world.answers['/api/1/tasks/42'] = task({ id: 42 });
  await world.emit('task', { id: 42, action: 'hardware.check', state: 'running', step: 1, steps: 11, words: 'memory memory test passed' });
  assert.match(text(world.view.main.querySelector('#hw-lead')), /Step 1 of 11: memory memory test passed/);
});

test('the memory test card says why, what runs and what does not, and when it last ran', async (context) => {
  const world = await openHardware(context);
  const card = text(world.card('memory'));
  assert.match(card, /Memory can fail without a clear sign/);
  assert.match(card, /While the system runs\. The memory area of a quick check tests up to 1 GiB of the free memory, in about a minute\./);
  assert.match(card, /All of it, at the next restart\. The next start tests the free memory before the system starts, about a minute for each 4 GiB \(about 4 minutes here\), takes any bad part out of use, then starts as usual\. The server is offline meanwhile, and nobody needs to be at it\./);
  assert.match(card, /For the fullest test, from the start menu\. Restart, choose Troubleshooting, then Test the memory \(memtest86\+\)\./);
  assert.match(card, /Last run: Nov 9 2:10 AM · passed, 1\.0G tested by 4 CPUs/);
  assert.match(card, /The test from the start menu leaves no record\./);
  assert.ok(buttonOf(world.card('memory'), 'Test at the next restart…'));
  assert.ok(card.indexOf('All of it, at the next restart') < card.indexOf('For the fullest test'), 'the start menu comes second');
});

test('Test part of the memory runs the memory area as a task', async (context) => {
  const world = await openHardware(context, { world: { answers: { 'hardware.check': { ok: true, task: 41, result: { task: 41 } } } } });
  await world.press(world.card('memory'), 'Test part of the memory');
  assert.deepEqual(world.ran('hardware.check'), [{ areas: 'memory' }]);
  assert.deepEqual(world.inlines.find(([action]) => action === 'hardware.check'), ['hardware.check', { areas: 'memory' }]);
  assert.match(text(world.view.main.querySelector('#hw-lead')), /^Checking the hardware\./);
  assert.equal(world.card('memory').querySelector('#memory-why').hidden, false);
});

test('a refused memory test is said in its card', async (context) => {
  const error = refusal('ADM-106', 'The console is locked', 403);
  const world = await openHardware(context, { world: { answers: { 'hardware.check': error } } });
  await world.press(world.card('memory'), 'Test part of the memory');
  assert.equal(world.problems.length, 1);
  assert.equal(world.problems[0][1].slot, world.card('memory').querySelector('.hw-errors'));
});

test('a reader has no button on the memory test, and the card says who has', async (context) => {
  const world = await openHardware(context, { session: { admin: false } });
  assert.equal(world.card('memory').querySelectorAll('button').length, 0);
  assert.match(text(world.card('memory')), /An administrator can test the memory\./);
  assert.match(text(world.card('memory')), /Last run: Nov 9 2:10 AM/);
});

test('the memory rows of a result with a failure come first in the last run', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/check': broken() } } });
  assert.match(text(world.card('memory')), /Last run: Nov 9 2:10 AM · failed, 2 errors in 1\.0G \(HW-103\)/);
  const none = await openHardware(context, { world: { reads: { '/api/1/hardware/check': null } } });
  assert.match(text(none.card('memory')), /Last run: never/);
});

/** The dialog's choices, as the words on their buttons. */
const choicesOf = (world) => world.dialog().querySelectorAll('button').map((one) => one.textContent);

test('Test at the next restart asks in a dialog with three ways out, and Cancel runs nothing', async (context) => {
  const world = await openHardware(context);
  await world.press(world.card('memory'), 'Test at the next restart…');
  assert.equal(world.dialog().querySelector('h2').textContent, 'Test the memory at the next restart?');
  assert.match(world.dialog().textContent, /The next start tests the free memory first \(about 4 minutes here\), then starts the system as usual\. The server is offline meanwhile, and nobody needs to be at it\./);
  assert.match(world.dialog().textContent, /Restart now and test restarts duke-nas in a minute/);
  assert.deepEqual(choicesOf(world), ['Restart now and test', 'At the next restart', 'Cancel']);
  assert.equal(world.document.activeElement, buttonOf(world.dialog(), 'At the next restart'), 'the choice that changes nothing now has the focus');
  await world.press(world.dialog(), 'Cancel');
  assert.deepEqual(world.ran('hardware.memtest'), []);
  assert.deepEqual(world.ran('power.restart'), []);
  assert.equal(world.dialog(), null);
  assert.ok(buttonOf(world.card('memory'), 'Test at the next restart…'), 'nothing was asked');
});

test('At the next restart asks for the test only, then shows it asked with Don\'t test and Restart now', async (context) => {
  const world = await openHardware(context);
  const card = world.card('memory');
  world.answers['/api/1/hardware/memtest'] = memtest({ asked: asked() });
  await world.press(card, 'Test at the next restart…');
  await world.press(world.dialog(), 'At the next restart');
  assert.deepEqual(world.ran('hardware.memtest'), [{}]);
  assert.deepEqual(world.ran('power.restart'), [], 'it restarts nothing');
  assert.equal(world.dialog(), null);
  assert.match(text(card), /The memory is tested at the next restart \(asked Nov 14, 3:10 PM by chase\)/);
  assert.equal(buttonOf(card, 'Test at the next restart…'), undefined, 'it is asked already');
  assert.ok(buttonOf(card, 'Don\'t test'));
  assert.ok(buttonOf(card, 'Restart now…'));
  assert.equal(buttonOf(card, 'Restart now…').parent, buttonOf(card, 'Don\'t test').parent, 'Restart now stands beside Don\'t test');
  assert.equal(card.querySelector('.hw-menu').hidden, true, 'the start menu\'s own restart is out of the way');
  assert.equal(world.document.activeElement, buttonOf(card, 'Don\'t test'), 'the focus is on the button that undoes it');
  assert.equal(world.ranWith('hardware.memtest')[0][2].where.slot, card.querySelector('.hw-errors'));
});

test('Restart now and test asks, then goes on to the restart\'s own question with the longer wait', async (context) => {
  const world = await openHardware(context);
  world.answers['/api/1/hardware/memtest'] = memtest({ asked: asked() });
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 0, at_ms: NOW } };
  await world.press(world.card('memory'), 'Test at the next restart…');
  await world.press(world.dialog(), 'Restart now and test');
  assert.deepEqual(world.ran('hardware.memtest'), [{}]);
  assert.equal(world.dialog().querySelector('h2').textContent, 'Restart duke-nas?');
  assert.match(world.dialog().textContent, /To test the memory before the system starts\./);
  await world.submit();
  assert.deepEqual(world.ran('power.restart'), [{ at: null }]);
  assert.equal(world.expected.length, 1);
  assert.match(world.expected[0][0], /^duke-nas is restarting and tests its memory first, which can take up to an hour\./);
  assert.equal(world.expected[0][1].quietMs, 14 * 60_000, '10 minutes and 1 for each 4 GiB of 15.6 GiB');
});

test('a test that was asked is shown asked, and Don\'t test cancels it and gives the focus back to the ask', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/memtest': memtest({ asked: asked() }) } } });
  const card = world.card('memory');
  assert.match(text(card), /The memory is tested at the next restart \(asked Nov 14, 3:10 PM by chase\)/);
  assert.equal(buttonOf(card, 'Test at the next restart…'), undefined);
  world.answers['/api/1/hardware/memtest'] = memtest();
  await world.press(card, 'Don\'t test');
  assert.deepEqual(world.ran('hardware.memtest-cancel'), [{}]);
  assert.equal(world.ranWith('hardware.memtest-cancel')[0][2].where.slot, card.querySelector('.hw-errors'));
  assert.ok(!text(card).includes('The memory is tested at the next restart ('), 'nothing is asked now');
  assert.equal(buttonOf(card, 'Don\'t test'), undefined);
  assert.equal(world.document.activeElement, buttonOf(card, 'Test at the next restart…'));
});

test('Restart now beside Don\'t test restarts for the test, and the start menu\'s own restart is out of the way', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/memtest': memtest({ asked: asked() }) } } });
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 0, at_ms: NOW } };
  assert.equal(world.card('memory').querySelector('.hw-menu').hidden, true);
  await world.press(world.card('memory'), 'Restart now…');
  assert.match(world.dialog().textContent, /To test the memory before the system starts\./);
  await world.submit();
  assert.deepEqual(world.ran('power.restart'), [{ at: null }]);
  assert.equal(world.expected[0][1].quietMs, 14 * 60_000);
  assert.deepEqual(world.ran('hardware.memtest'), [], 'it is asked already');
});

test('a refusal of the test is said in the memory card, and Restart now and test restarts nothing then', async (context) => {
  const error = refusal('HW-105', 'The next start is taken already');
  const world = await openHardware(context, { world: { answers: { 'hardware.memtest': error } } });
  await world.press(world.card('memory'), 'Test at the next restart…');
  await world.press(world.dialog(), 'Restart now and test');
  assert.equal(world.problems.length, 1);
  assert.equal(world.problems[0][0], error);
  assert.equal(world.problems[0][1].slot, world.card('memory').querySelector('.hw-errors'));
  assert.equal(world.dialog(), null, 'no restart is offered');
  assert.deepEqual(world.ran('power.restart'), []);
  assert.ok(buttonOf(world.card('memory'), 'Test at the next restart…'));
});

test('a reader sees that the test is asked, and has no button on it', async (context) => {
  const world = await openHardware(context, { session: { admin: false }, world: { reads: { '/api/1/hardware/memtest': memtest({ asked: asked() }) } } });
  assert.match(text(world.card('memory')), /The memory is tested at the next restart \(asked Nov 14, 3:10 PM by chase\)/);
  assert.equal(world.card('memory').querySelectorAll('button').length, 0);
  assert.match(text(world.card('memory')), /All of it, at the next restart\./);
});

test('the last run at a start comes beside the check\'s, in the colour of what it found', async (context) => {
  const startOf = (world) => world.card('memory').querySelectorAll('p').find((one) => /^Nov 14.*(at the start|the test at the start|the start menu did not)/.test(one.textContent) && !one.hidden);
  const reportOf = (world) => world.card('memory').querySelectorAll('a').find((one) => one.textContent === 'Report a problem');
  const opened = (last) => openHardware(context, { world: { reads: { '/api/1/hardware/memtest': memtest({ last }) } } });

  const none = await openHardware(context);
  assert.equal(startOf(none), undefined, 'a machine that never tested at a start says nothing of it');
  assert.equal(reportOf(none).parent.hidden, true);

  const passed = await opened(startTest('passed'));
  assert.equal(startOf(passed).textContent, 'Nov 14, at the start: passed · 15.2 of 16 GiB tested in 4 minutes');
  assert.equal(startOf(passed).getAttribute('class'), 'hw-start');
  assert.equal(reportOf(passed).parent.hidden, true);
  assert.match(text(passed.card('memory')), /Last run: Nov 9 2:10 AM · passed, 1\.0G tested by 4 CPUs/, 'the check\'s own line stays');

  const errors = await opened(startTest('errors'));
  assert.equal(startOf(errors).textContent, 'Nov 14, at the start: 3 errors in 2 places. Those parts are out of use until the next restart. The memory should be replaced (HW-103).');
  assert.equal(startOf(errors).getAttribute('class'), 'hw-start bad');
  assert.equal(reportOf(errors).parent.hidden, false);
  assert.equal(reportOf(errors).getAttribute('href'), '/report?about=hardware');

  const unfinished = await opened(startTest('unfinished'));
  assert.equal(startOf(unfinished).textContent, 'Nov 14: the test at the start did not finish (HW-104). The power went or the machine stopped during it, which failing memory can cause.');
  assert.equal(startOf(unfinished).getAttribute('class'), 'hw-start warn');
  assert.equal(reportOf(unfinished).parent.hidden, true);

  const taken = await opened(startTest('not-taken'));
  assert.equal(startOf(taken).textContent, 'Nov 14: the start menu did not run the test: an update was tried first. Ask again.');
  assert.equal(startOf(taken).getAttribute('class'), 'muted');
});

test('a test at a start that cannot be read is said, and the ask is still there', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/memtest': refusal('ADM-500', 'It broke', 500) } } });
  assert.match(text(world.card('memory')), /The memory test at the start could not be read\./);
  assert.ok(buttonOf(world.card('memory'), 'Test at the next restart…'));
  assert.match(text(world.card('memory')), /Last run: Nov 9 2:10 AM/);
});

test('a machine whose memory is not known says no time for the test, and waits the longest for it', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware': show({ memory: { bytes: null, ecc: null, slots: null, modules: null } }), '/api/1/hardware/memtest': memtest({ asked: asked() }) } } });
  assert.doesNotMatch(text(world.card('memory')), /about \d+ minutes? here/);
  assert.match(text(world.card('memory')), /about a minute for each 4 GiB, takes any bad part out of use/);
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 0, at_ms: NOW } };
  await world.press(world.card('memory'), 'Restart now…');
  await world.submit();
  assert.equal(world.expected[0][1].quietMs, 70 * 60_000);
});

test('Restart now from the memory card gives the start menu as the reason', async (context) => {
  const world = await openHardware(context);
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 60, at_ms: NOW + 60_000 } };
  await world.press(world.card('memory'), 'Restart now…');
  assert.equal(world.dialog().querySelector('h2').textContent, 'Restart duke-nas?');
  assert.match(world.dialog().textContent, /To test all of the memory from the start menu\./);
  await world.submit();
  assert.deepEqual(world.ran('power.restart'), [{ at: null }]);
  assert.match(text(world.card('power')), /duke-nas restarts in 60 seconds/);
  assert.ok(buttonOf(world.card('power'), 'Cancel'));
});

test('Restart or shut down says what the minute is for, and a reader has no button', async (context) => {
  const world = await openHardware(context);
  const card = text(world.card('power'));
  assert.match(card, /A restart or a shut down begins a minute after you ask, and you can cancel until then\./);
  assert.ok(buttonOf(world.card('power'), 'Restart…'));
  assert.ok(buttonOf(world.card('power'), 'Shut down…'));
  assert.ok(buttonOf(world.card('power'), 'Restart at that time'));
  const reader = await openHardware(context, { session: { admin: false } });
  assert.equal(reader.card('power').querySelectorAll('button').length, 0);
  assert.match(text(reader.card('power')), /An administrator can restart or shut down this machine\./);
});

test('Restart asks first, then runs the restart and counts the minute in its own card', async (context) => {
  const world = await openHardware(context);
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 60, at_ms: NOW + 60_000 } };
  await world.press(world.card('power'), 'Restart…');
  assert.equal(world.dialog().querySelector('h2').textContent, 'Restart duke-nas?');
  assert.ok(!world.dialog().textContent.includes('To finish'), 'no reason is given from here');
  await world.submit();
  assert.deepEqual(world.ran('power.restart'), [{ at: null }]);
  assert.match(text(world.card('power')), /duke-nas restarts in 60 seconds/);
});

test('Restart at that time sets a restart for the time typed, and a time that is not one runs nothing', async (context) => {
  const world = await openHardware(context);
  const field = world.card('power').querySelector('input');
  field.value = '03:00';
  field.fire('input');
  assert.deepEqual(world.inlines.at(-1), ['power.restart', { at: '03:00' }]);
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 42_480, at_ms: NOW + 42_480_000 } };
  await world.press(world.card('power'), 'Restart at that time');
  assert.deepEqual(world.ran('power.restart'), [{ at: '03:00' }]);
  assert.match(text(world.card('power')), /duke-nas restarts at 3:00 AM/);
  const again = await openHardware(context);
  await again.press(again.card('power'), 'Restart at that time');
  assert.deepEqual(again.ran('power.restart'), []);
  assert.match(again.notes[0][0], /A time is written HH:MM/);
});

test('Shut down asks for the machine\'s name, and runs the shut down only when it is typed', async (context) => {
  const world = await openHardware(context);
  world.answers['/api/1/power'] = { shutdown: { end: 'poweroff', in_seconds: 60, at_ms: NOW + 60_000 } };
  await world.press(world.card('power'), 'Shut down…');
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Shut down duke-nas?');
  assert.match(dialog.textContent, /does not come back until someone presses its power button/);
  const confirm = buttonOf(dialog, 'Shut down in a minute');
  assert.equal(confirm.disabled, true);
  const typed = dialog.querySelector('input');
  typed.value = 'duke-nas';
  typed.fire('input');
  assert.equal(confirm.disabled, false);
  await world.submit();
  assert.deepEqual(world.ran('power.shutdown'), [{}]);
  assert.match(text(world.card('power')), /duke-nas shuts down in 60 seconds/);
});

test('Cancel stops the restart or shut down that is set', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/power': { shutdown: { end: 'reboot', in_seconds: 5400, at_ms: NOW + 5_400_000 } } } } });
  assert.match(text(world.card('power')), /duke-nas restarts at 4:42 PM/);
  world.answers['/api/1/power'] = { shutdown: null };
  await world.press(world.card('power'), 'Cancel');
  assert.deepEqual(world.ran('power.cancel'), [{}]);
  assert.equal(world.card('power').querySelector('[role="timer"]').closest('.banner').hidden, true);
});

test('a reader sees the restart that is set, and no Cancel to press', async (context) => {
  const world = await openHardware(context, { session: { admin: false }, world: { reads: { '/api/1/power': { shutdown: { end: 'reboot', in_seconds: 5400, at_ms: NOW + 5_400_000 } } } } });
  assert.match(text(world.card('power')), /duke-nas restarts at 4:42 PM/);
  assert.equal(world.card('power').querySelector('.btns').hidden, true);
});

test('a restart that another person sets shows its line when the shell tells of it', async (context) => {
  const world = await openHardware(context);
  assert.equal(world.card('power').querySelector('.banner'), null);
  await world.emit('power', { shutdown: { end: 'reboot', in_seconds: 60, at_ms: NOW + 60_000 } });
  assert.match(text(world.card('power')), /duke-nas restarts in 60 seconds/);
});

test('the page honours the card its address names', async (context) => {
  let focused = null;
  const world = await openHardware(context, { hash: '#power' });
  focused = world.document.activeElement;
  assert.equal(focused.id, 'power-h');
});

test('the cards come in after the first read, and the page reads again when the server answers again', async (context) => {
  const world = await openHardware(context);
  const reads = world.calls.filter((path) => path === '/api/1/hardware').length;
  world.answers['/api/1/hardware'] = show({ model: 'OptiPlex 9020' });
  await world.emit('online');
  assert.equal(world.calls.filter((path) => path === '/api/1/hardware').length, reads + 1);
  assert.match(text(world.card('machine')), /OptiPlex 9020/);
});

test('a read that fails is said by the shell, and the page still draws what it has', async (context) => {
  const error = new ApiError({ id: 'ADM-129', title: 'hardware is not installed', detail: '', fix: '', status: 501 });
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/history': error, '/api/1/storage': error, '/api/1/console/about': error } } });
  assert.ok(world.card('machine'));
  assert.ok(!text(world.card('check')).includes('Earlier checks'));
  assert.doesNotMatch(text(world.view.main.querySelector('#hw-lead')), /diskhealth/);
  assert.doesNotMatch(text(world.card('machine')), /Up for/);
});

test('an administrator whose read of the modules is refused as locked sees the button, not a problem', async (context) => {
  const locked = refusal('ADM-106', 'The console is locked', 403);
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/inventory': locked } } });
  assert.equal(world.problems.length, 0);
  assert.ok(buttonOf(world.card('machine'), 'Show the modules'));
});

test('the inventory of the modules is used for the memory, not the reader\'s', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/inventory': inventory() } } });
  assert.ok(world.calls.includes('/api/1/hardware/inventory'));
  assert.match(text(world.card('machine')), /DDR4-2133 · no error correction/);
});

test('leaving the page stops what it waits for', async (context) => {
  const world = await openHardware(context);
  const reads = world.calls.length;
  world.view.signal.dispatchEvent?.(new Event('abort'));
  await settle();
  assert.equal(world.calls.length, reads);
});

// ---- Temperatures and fans ----------------------------------------------------

const rowsOf = (world) => world.card('temps').querySelectorAll('tbody tr').map(text);

test('Temperatures and fans is a table of the processor, the disks and the zones, a line for each fan, and the line about alerts', async (context) => {
  const world = await openHardware(context);
  for (const read of ['/api/1/hardware/sensors', '/api/1/hardware/highest', '/api/1/updates/history']) assert.ok(world.calls.includes(read), read);
  assert.deepEqual(world.card('temps').querySelectorAll('thead th').map(text), ['Sensor', 'Now', 'Highest, 30 days', 'Limit']);
  assert.deepEqual(rowsOf(world), [
    'Processor (Package id 0)41 °C63 °CYesterday 3:02 AM, during the update100 °Ccritical',
    'Disk sda · WDC WD40EFRX-68N36 °Cread at 3:10 PM41 °CYesterday 3:10 AM, during the update50 °C, critical 60 °Cfor a hard disk',
    'Disk nvme0 · Samsung SSD 98044 °Cread at 3:10 PM58 °CNov 12 2:14 PM70 °C, critical 85 °Cthe disk\'s own',
    'Disk sdb · WDC WD40EFRX-68NAsleep since 2:10 AM: not read, so as not to wake it—50 °C, critical 60 °Cfor a hard disk',
    'Zone acpitz32 °C—97 °Ccritical',
  ]);
  const card = text(world.card('temps'));
  assert.match(card, /Disks are read every 10 minutes, but a sleeping one\. An alert goes out when a sensor stays within 5 °C of its critical limit, or a disk at its warning limit, for 5 minutes \(metricsd\)\./);
  assert.match(card, /fan11,190 RPMfan2not judged: no fan on this header$/);
  assert.equal(world.card('temps').querySelectorAll('td').find((cell) => cell.textContent.startsWith('41 °C')).getAttribute('data-label'), 'Now');
  assert.equal(world.card('temps').getAttribute('aria-labelledby'), 'temps-h');
});

test('the cards come in the order of the phone, with the temperatures, the devices and the restart across both columns', async (context) => {
  const world = await openHardware(context);
  const cards = world.view.main.querySelector('.hw').children.filter((one) => one.tag === 'section');
  assert.deepEqual(cards.map((one) => one.id), ['machine', 'check', 'errors', 'temps', 'disks', 'memory', 'devices', 'power']);
  assert.deepEqual(cards.filter((one) => one.classes.has('hw-wide')).map((one) => one.id), ['temps', 'devices', 'power']);
});

test('the choice of Fahrenheit changes every temperature at once, is kept, and is the unit the page opens with next time', async (context) => {
  const store = useStore(context);
  const world = await openHardware(context);
  const [celsius, fahrenheit] = world.card('temps').querySelectorAll('.seg button');
  assert.deepEqual([celsius.textContent, celsius.getAttribute('aria-pressed'), fahrenheit.getAttribute('aria-pressed')], ['°C', 'true', 'false']);
  const calls = world.calls.length;
  fahrenheit.click();
  assert.equal(world.calls.length, calls, 'no read is made for a change of unit');
  assert.deepEqual([celsius.getAttribute('aria-pressed'), fahrenheit.getAttribute('aria-pressed')], ['false', 'true']);
  assert.match(rowsOf(world)[0], /^Processor \(Package id 0\)106 °F145 °F.*212 °Fcritical$/);
  assert.equal(JSON.parse(store.get('dukovo-admin.hardware.temps.unit')), 'F');
  const again = await openHardware(context);
  assert.match(rowsOf(again)[0], /106 °F/);
  assert.equal(again.card('temps').querySelectorAll('.seg button')[1].getAttribute('aria-pressed'), 'true');
});

test('a machine whose kernel reads no sensor says so, keeps its disks\' rows and says that no fan can be read', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/sensors': sensors({ sensors: [], zones: [] }) } } });
  const card = text(world.card('temps'));
  assert.match(card, /The kernel reads no temperature on this machine\./);
  assert.deepEqual(rowsOf(world).map((row) => row.slice(0, 9)), ['Disk sda ', 'Disk nvme', 'Disk sdb ']);
  assert.match(card, /No fan speed can be read on this machine\.$/);
  const bare = await openHardware(context, { world: { reads: { '/api/1/hardware/sensors': sensors({ sensors: [], zones: [], disks: [] }) } } });
  assert.equal(bare.card('temps').querySelector('table'), null);
  assert.match(text(bare.card('temps')), /The kernel reads no temperature on this machine\.No fan speed can be read on this machine\.$/);
  assert.ok(!/Disks are read every 10 minutes/.test(text(bare.card('temps'))));
});

test('the disks that keep no health record are one muted line under the table', async (context) => {
  const answer = storage();
  answer.disks.push({ name: 'sdc', model: 'Expansion', health: null }, { name: 'vda', model: null, health: null });
  const world = await openHardware(context, { world: { reads: { '/api/1/storage': answer } } });
  assert.match(text(world.card('temps')), /2 disks keep no health record \(USB or virtual\) and are not read\./);
  assert.equal(world.card('temps').querySelectorAll('p').find((one) => one.textContent.startsWith('2 disks')).classes.has('muted'), true);
});

test('a fan that stopped is in the error colour, and a temperature near its limit has its words', async (context) => {
  const hot = sensors();
  hot.sensors[0].value = 96;
  hot.sensors[1].alarm = true;
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/sensors': hot } } });
  const near = world.card('temps').querySelectorAll('td .st').find((one) => one.textContent === 'near its limit');
  assert.ok(near.classes.has('warn'));
  const stopped = world.card('temps').querySelectorAll('dd .st').find((one) => one.textContent === 'stopped');
  assert.ok(stopped.classes.has('bad'));
});

test('the temperatures are read again each minute, without the highest, and a read that fails leaves what was read', async (context) => {
  const world = await openHardware(context);
  const reads = () => world.calls.filter((call) => call === '/api/1/hardware/sensors').length;
  assert.equal(reads(), 1);
  const warmer = sensors();
  warmer.sensors[0].value = 55;
  world.answers['/api/1/hardware/sensors'] = warmer;
  await world.tick(60_000);
  assert.equal(reads(), 2);
  assert.equal(world.calls.filter((call) => call === '/api/1/hardware/highest').length, 1);
  assert.match(rowsOf(world)[0], /^Processor \(Package id 0\)55 °C/);
  world.answers['/api/1/hardware/sensors'] = refusal('ADM-500', 'down', 500);
  await world.tick(60_000);
  assert.match(rowsOf(world)[0], /^Processor \(Package id 0\)55 °C/);
  assert.equal(world.problems.length, 0, 'a later failure says nothing, as the page says when it is offline');
});

test('temperatures that cannot be read are said, with the refusal where the card shows it, and the rest of the page stands', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/sensors': refusal('ADM-500', 'down', 500) } } });
  assert.match(text(world.card('temps')), /The temperatures and fans could not be read\./);
  assert.equal(world.problems.length, 1);
  assert.equal(world.problems[0][1].slot, world.card('temps').querySelector('.hw-errors'));
  assert.match(text(world.card('machine')), /Dell Inc\. OptiPlex 7040/);
});

test('without the highest or the update history the rows are still there, with a dash for the highest', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/highest': refusal('ADM-500', 'down', 500), '/api/1/updates/history': refusal('ADM-500', 'down', 500) } } });
  assert.match(rowsOf(world)[0], /^Processor \(Package id 0\)41 °C—100 °Ccritical$/);
  assert.equal(world.problems.length, 0);
});

test('a reader sees the temperatures too', async (context) => {
  const world = await openHardware(context, { session: { admin: false } });
  assert.equal(rowsOf(world).length, 5);
});

test('the card is reached by its address, and says it is reading while the first answer is awaited', async (context) => {
  const world = await openHardware(context, { hash: '#temps', world: { reads: { '/api/1/hardware/sensors': () => new Promise(() => {}) } } });
  assert.match(text(world.card('temps')), /Reading the temperatures…/);
});

// ---- Disks -------------------------------------------------------------------

const DISKS_NOTE = 'Unlock to see the disks\' own tests';
/** A disk's row as three parts, `head | tests | buttons`, since the blocks of a page have a break between them. */
const lineOf = (world, disk) => world.card('disks').querySelectorAll('li').find((one) => one.textContent.startsWith(disk))
  .children.map((part) => (part.classes.has('btns') ? part.children.map(text).join(' + ') : text(part))).join(' | ');

test('Disks lists each whole disk with its model and health, its last short and extended tests, and the two buttons that run one', async (context) => {
  const world = await openHardware(context);
  assert.ok(world.calls.includes('/api/1/storage/disks/sda/health'));
  assert.ok(world.calls.includes('/api/1/storage/disks/sdb/health'));
  assert.equal(lineOf(world, 'sda'), 'sda · WD Red Healthy | Short test Nov 14, passed (at hour 21,800) · Extended test Nov 6, passed (at hour 21,604) | Short test + Extended test');
  assert.match(lineOf(world, 'sdb'), /^sdb · WD Red Wearing \| /);
  const link = world.card('disks').querySelector('a');
  assert.deepEqual([link.textContent, link.getAttribute('href')], ['sda', '/storage/sda']);
  assert.equal(world.card('disks').querySelector('.pill.ok').textContent, 'Healthy');
  assert.equal(world.card('disks').querySelector('.pill.warn').textContent, 'Wearing');
  assert.match(text(world.card('disks')), /A short test is the disk's health, its short self-test and a read of 256 places, about 3 minutes\. An extended test is its long self-test and a read of all of it: hours\./);
  assert.ok(buttonOf(world.card('disks'), DISKS_NOTE).hidden, 'an unlocked console has nothing to unlock');
  assert.equal(world.card('disks').getAttribute('aria-labelledby'), 'disks-h');
});

test('the Short test button runs the disks area on that disk, and the Extended test one a full check of it', async (context) => {
  const open = () => openHardware(context, { world: { answers: { 'hardware.check': { ok: true, task: 77 } } } });
  const world = await open();
  await world.press(world.card('disks').querySelectorAll('li')[1], 'Short test');
  const more = await open();
  await more.press(more.card('disks').querySelectorAll('li')[1], 'Extended test');
  assert.deepEqual(world.ran('hardware.check'), [{ areas: 'disks', disk: 'sdb' }]);
  assert.deepEqual(more.ran('hardware.check'), [{ areas: 'disks', disk: 'sdb', full: true }]);
  const [first] = world.ranWith('hardware.check');
  assert.equal(first[2].where.slot, world.card('disks').querySelector('.hw-errors'));
  assert.equal(first[2].words.running, 'Checking the hardware');
  assert.equal(world.dialog(), null, 'no dialog asks first: a test is a button');
});

test('a disk with a test running says how far it is, and every button that starts a check is off', async (context) => {
  const running = task({ args: { areas: 'disks', disk: 'sdb' }, percent: 40, step: 2, steps: 3 });
  const world = await openHardware(context, { world: { reads: { '/api/1/tasks': { tasks: [running] } } } });
  assert.equal(lineOf(world, 'sdb'), 'sdb · WD Red Wearing | Testing: 40% | Short test + Extended test');
  assert.match(lineOf(world, 'sda'), /\| Short test Nov 14, passed/);
  for (const one of world.card('disks').querySelectorAll('.btns button')) assert.equal(one.getAttribute('aria-disabled'), 'true');
  assert.equal(world.card('disks').querySelector('#disks-why').hidden, false);
  await world.press(world.card('disks'), 'Short test');
  assert.deepEqual(world.ran('hardware.check'), []);
  await world.emit('task', { id: 41, action: 'hardware.check', state: 'running', percent: 70 });
  assert.match(lineOf(world, 'sdb'), /Testing: 70%/);
  const unknown = task({ args: { areas: 'disks', disk: 'sdb' } });
  const bare = await openHardware(context, { world: { reads: { '/api/1/tasks': { tasks: [unknown] } } } });
  assert.match(lineOf(bare, 'sdb'), /Testing…/);
});

test('when a test ends the page reads the logs again, and a button keeps its focus while its test runs', async (context) => {
  const world = await openHardware(context, { world: { answers: { 'hardware.check': { ok: true, task: 77 } } } });
  const button = buttonOf(world.card('disks').querySelectorAll('li')[0], 'Short test');
  button.focus();
  await world.press(world.card('disks').querySelectorAll('li')[0], 'Short test');
  assert.equal(world.document.activeElement, button, 'the row is not made again');
  assert.equal(button.getAttribute('aria-disabled'), 'true');
  const reads = world.calls.filter((call) => call === '/api/1/storage/disks/sda/health').length;
  world.answers['/api/1/tasks'] = { tasks: [] };
  world.answers['/api/1/storage/disks/sda/health'] = diskHealth('sda', { self_tests: [{ kind: 'short', result: 'failed', at_hours: 21804 }] });
  await world.emit('task', { id: 77, action: 'hardware.check', state: 'done' });
  assert.equal(world.calls.filter((call) => call === '/api/1/storage/disks/sda/health').length, reads + 1);
  assert.equal(lineOf(world, 'sda'), 'sda · WD Red Healthy | Short test Nov 14, failed (at hour 21,804) | Short test + Extended test');
  assert.ok(world.card('disks').querySelector('.bad'));
  assert.equal(button.getAttribute('aria-disabled'), null);
});

test('a locked administrator is offered the tests and sees them once the console unlocks, and a reader is told who sees them', async (context) => {
  const locked = await openHardware(context, { unlocked: false });
  assert.ok(!locked.calls.some((call) => call.includes('/health')));
  assert.equal(lineOf(locked, 'sda'), 'sda · WD Red Healthy |  | Short test + Extended test');
  assert.equal(buttonOf(locked.card('disks'), DISKS_NOTE).hidden, false);
  const { setUnlockSeconds } = await import('../unlock.js');
  setUnlockSeconds(900);
  await locked.emit('unlock');
  assert.match(lineOf(locked, 'sda'), /Short test Nov 14, passed/);
  assert.equal(buttonOf(locked.card('disks'), DISKS_NOTE).hidden, true);
  const reader = await openHardware(context, { session: { admin: false } });
  assert.ok(!reader.calls.some((call) => call.includes('/health')));
  assert.equal(lineOf(reader, 'sda'), 'sda · WD Red Healthy | ');
  assert.equal(reader.card('disks').querySelectorAll('button').length, 0);
  assert.match(text(reader.card('disks')), /Administrators see the disks' own tests$/);
});

test('a disk that does not report its health says so and may still be tested, and a log that cannot be read is said', async (context) => {
  const answer = storage();
  answer.disks.push({ name: 'sdc', model: 'Expansion', health: null });
  const world = await openHardware(context, { world: { reads: { '/api/1/storage': answer, '/api/1/storage/disks/sdb/health': refusal('ADM-500', 'down', 500) } } });
  assert.equal(lineOf(world, 'sdc'), 'sdc · Expansion Not checked | This disk does not report its health or its tests. | Short test + Extended test');
  assert.ok(!world.calls.includes('/api/1/storage/disks/sdc/health'));
  assert.match(lineOf(world, 'sdb'), /Its own tests could not be read/);
  assert.equal(world.problems.length, 0);
});

test('a disk with no log of its own tests says so, and so does one whose log has no short or extended test', async (context) => {
  const world = await openHardware(context, { world: { reads: {
    '/api/1/storage/disks/sda/health': diskHealth('sda', { self_tests: null }),
    '/api/1/storage/disks/sdb/health': diskHealth('sdb', { self_tests: [{ kind: 'conveyance', result: 'passed', at_hours: 21000 }] }),
  } } });
  assert.match(lineOf(world, 'sda'), /Healthy \| This disk keeps no log of its own tests \|/);
  assert.match(lineOf(world, 'sdb'), /Wearing \| No short or extended test is in its log \|/);
});

test('the disks that cannot be read, and a machine with none, are said', async (context) => {
  const down = await openHardware(context, { world: { reads: { '/api/1/storage': refusal('ADM-500', 'down', 500) } } });
  assert.match(text(down.card('disks')), /The disks could not be read\./);
  const none = await openHardware(context, { world: { reads: { '/api/1/storage': { disks: [] } } } });
  assert.match(text(none.card('disks')), /No disk was found\./);
});

// ---- Devices -------------------------------------------------------------------

const tablesOf = (world) => world.card('devices').querySelectorAll('table').map((table) => [text(table.querySelector('caption')), table.querySelectorAll('tbody tr').map(text)]);

test('the devices are not read until the card is asked, and then come in a table for each group with the hidden ones folded away', async (context) => {
  const world = await openHardware(context);
  assert.ok(!world.calls.includes('/api/1/hardware/devices'));
  assert.match(text(world.card('devices')), /Every card and port the system can tell of/);
  await world.press(world.card('devices'), 'Show the devices');
  assert.ok(world.calls.includes('/api/1/hardware/devices'));
  assert.deepEqual(tablesOf(world), [
    ['Network · 2', [
      'Intel I219-LMnet0 · 8086:15b8e1000ePCI 0000:00:1f.6Working'.replace('e1000ePCI 0000:00:1f.6Working', 'e1000eWorkingPCI 0000:00:1f.6'),
      'Intel I210net1 · 8086:1533igbNo cablePCI 0000:03:00.0',
    ]],
    ['Storage · 2', [
      'Intel Q170 SATA controllersda, sdb · 8086:a102ahciWorkingPCI 0000:00:17.0',
      'Seagate Expansionsdc · 0bc2:2312usb-storageWorkingUSB 2-1',
    ]],
    ['USB · 1', ['FTDI FT232R serial adapter0403:6001ftdi_sioWorkingUSB 1-2']],
    ['Display · 1', ['Intel HD Graphics 5308086:1912efifbWorkingPCI 0000:00:02.0']],
    ['Other · 2', [
      'Intel Management Engine interface8086:a13anoneNo driverPCI 0000:00:16.0',
      'Unknown device1234:5678Not told—PCI 0000:04:00.0',
    ]],
    ['Hidden devices', [
      'Intel PCI bridge8086:a110pcieportWorkingPCI 0000:00:1c.0',
      'Intel host bridge8086:1910noneWorkingPCI 0000:00:00.0',
    ]],
  ]);
  const hidden = world.card('devices').querySelector('details');
  assert.equal(text(hidden.querySelector('summary')), 'Hidden: 2 devices that work and need nothing, such as USB hubs and bridges');
  assert.equal(hidden.querySelectorAll('table').length, 1);
  assert.equal(world.card('devices').querySelectorAll('table').length - 1, 5, 'the hidden ones are in a table of their own, inside the fold');
});

test('a device with no cable has a warning state, and one no driver took is not "Not told"', async (context) => {
  const world = await openHardware(context);
  await world.press(world.card('devices'), 'Show the devices');
  const states = world.card('devices').querySelectorAll('.st');
  assert.ok(states.find((one) => one.textContent === 'No cable').classes.has('warn'));
  assert.ok(states.find((one) => one.textContent === 'No driver').classes.has('off'));
  assert.ok(states.find((one) => one.textContent === 'Working').classes.has('ok'));
  assert.equal(states.filter((one) => one.textContent === 'Not told').length, 0, '"Not told" is the driver\'s, which the system could not tell');
  assert.equal(world.card('devices').querySelectorAll('td').filter((cell) => cell.textContent === 'Not told').length, 1);
  assert.equal(world.card('devices').querySelectorAll('td').find((cell) => cell.textContent === 'Not told').getAttribute('data-label'), 'Driver');
});

test('a failed device has a danger badge, and the counts and the owner are under its name', async (context) => {
  const answer = devices();
  answer.devices[0].pcie_errors = { owner: 'kernel', corrected: 2, nonfatal: 1, fatal: 1, failed: true };
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/devices': answer } } });
  await world.press(world.card('devices'), 'Show the devices');
  const failed = world.card('devices').querySelectorAll('.st').find((one) => one.textContent === 'Failed');
  assert.ok(failed.classes.has('bad'));
  const row = text(world.card('devices').querySelector('tbody tr'));
  assert.match(row, /PCIe errors: 2 corrected, 1 non-fatal, 1 fatal/);
  assert.match(row, /Errors handled by: the kernel/);
});

test('the card says how to report a device that needs a driver, and moves focus to its heading when it has drawn', async (context) => {
  const world = await openHardware(context);
  await world.press(world.card('devices'), 'Show the devices');
  const link = world.card('devices').querySelectorAll('a').find((one) => one.textContent === 'Report it');
  assert.equal(link.getAttribute('href'), '/report?about=hardware');
  assert.match(text(world.card('devices')), /A device with no driver that you need\? Report it; the report carries its IDs \(vendor and device numbers, never a serial\)\./);
  assert.equal(world.document.activeElement, world.card('devices').querySelector('h2'));
  assert.equal(buttonOf(world.card('devices'), 'Show the devices'), undefined);
});

test('a reader may open the devices, and a list that cannot be read leaves the button to try again', async (context) => {
  const reader = await openHardware(context, { session: { admin: false } });
  await reader.press(reader.card('devices'), 'Show the devices');
  assert.equal(tablesOf(reader).length, 6);
  const down = await openHardware(context, { world: { reads: { '/api/1/hardware/devices': refusal('ADM-500', 'down', 500) } } });
  await down.press(down.card('devices'), 'Show the devices');
  assert.equal(down.problems.length, 1);
  assert.equal(down.problems[0][1].slot, down.card('devices').querySelector('.hw-errors'));
  const again = buttonOf(down.card('devices'), 'Show the devices');
  assert.equal(again.getAttribute('aria-disabled'), null);
  down.answers['/api/1/hardware/devices'] = devices();
  await down.press(down.card('devices'), 'Show the devices');
  assert.equal(tablesOf(down).length, 6);
});

test('a machine whose list is empty says so', async (context) => {
  const world = await openHardware(context, { world: { reads: { '/api/1/hardware/devices': { format: 1, devices: [] } } } });
  await world.press(world.card('devices'), 'Show the devices');
  assert.match(text(world.card('devices')), /The system lists no device\./);
  assert.equal(world.card('devices').querySelectorAll('table').length, 0);
});

/** A server: its BMC is named by `hardware show` as an administrator gets it, and the sensors and log are the BMC's. */
const server = (more = {}) => ({
  reads: {
    '/api/1/hardware/inventory': { ...inventory(), bmc: bmcOf() },
    '/api/1/hardware/bmc/sensors': bmcSensors(),
    '/api/1/hardware/bmc/events': bmcEvents(14),
    ...more,
  },
});

test('a machine with no BMC has no BMC card, and asks for none of its reads', async (context) => {
  const world = await openHardware(context);
  assert.equal(world.card('bmc'), null);
  assert.ok(!world.calls.some((path) => path.includes('/bmc/')), world.calls.join(' '));
});

test('a server has a BMC card after the temperatures, with the BMC, its address, what is not well, the rest to open and the last ten entries', async (context) => {
  const world = await openHardware(context, { world: server() });
  const cards = world.view.main.querySelector('.hw').children.filter((one) => one.tag === 'section').map((one) => one.id);
  assert.deepEqual(cards, ['machine', 'check', 'errors', 'temps', 'bmc', 'disks', 'memory', 'devices', 'power']);
  const card = world.card('bmc');
  assert.match(text(card), /Maker\s*Dell/);
  assert.match(text(card), /LAN address\s*10\.1\.2\.3/);
  assert.match(text(card), /Voltages: Voltage 31 · unavailable/);
  // The rest is under a list that opens, with each kind apart: 3 power, 2 currents and 31 voltages.
  const all = card.querySelector('details');
  assert.equal(all.querySelector('summary').textContent, 'Show all 36 sensors');
  assert.deepEqual(all.querySelectorAll('h3').map((one) => one.textContent), ['Power', 'Currents', 'Voltages']);
  assert.equal(all.querySelectorAll('li').length, 36);
  // Ten entries of the 14, the newest first.
  const entries = card.querySelectorAll('tbody tr');
  assert.equal(entries.length, 10);
  assert.match(entries[0].textContent, /^Yesterday 8:30 AM/);
  // The BMC's temperatures and fans are in the temperatures card, and its voltages are not.
  const temps = text(world.card('temps'));
  assert.match(temps, /Inlet Temp/);
  assert.match(temps, /Fan1A RPM \(BMC\)/);
  assert.ok(!temps.includes('Voltage 1'), temps);
  assert.ok(world.calls.includes('/api/1/hardware/bmc/sensors'));
});

test('the Identify button lights the light and shows the answer in the card', async (context) => {
  const world = await openHardware(context, { world: server() });
  const card = world.card('bmc');
  await world.press(card, 'Identify');
  assert.deepEqual(world.ran('hardware.identify'), [{}]);
  assert.equal(world.ranWith('hardware.identify')[0][2].where.slot, card.querySelector('.hw-errors'));
});

test('a reader and a locked administrator are shown no BMC', async (context) => {
  for (const options of [{ session: { admin: false } }, { unlocked: false }]) {
    const world = await openHardware(context, { ...options, world: server() });
    assert.equal(world.card('bmc'), null);
    assert.ok(!world.calls.some((path) => path.includes('/bmc/')), world.calls.join(' '));
  }
});

test('the BMC card comes in when the console unlocks, and a log that cannot be read is said so', async (context) => {
  const world = await openHardware(context, { unlocked: false, world: server({ '/api/1/hardware/bmc/events': refusal('ADM-500', 'the BMC does not answer', 500) }) });
  assert.equal(world.card('bmc'), null);
  setUnlockSeconds(900);
  await world.emit('unlock');
  const card = world.card('bmc');
  assert.ok(card, 'the card is there');
  assert.match(text(card), /The event log could not be read\./);
  assert.match(text(card), /Show all 36 sensors/);
});
