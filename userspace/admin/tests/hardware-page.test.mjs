// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Hardware page on a fake document: what each card says for an
// administrator, a locked one and a reader, which action each button runs,
// and what the page does with a check that runs, a restart and a shut down.

import assert from 'node:assert/strict';
import test, { afterEach, beforeEach } from 'node:test';

import { ApiError } from '../api.js';
import {
  about, broken, clean, inventory, last, NOW, show, skipped, task,
} from './hardware-fixtures.mjs';
import {
  buttonOf, openHardware, refusal, settle,
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
  assert.deepEqual(world.calls.slice(0, 6).sort(), ['/api/1/console/about', '/api/1/hardware', '/api/1/hardware/check', '/api/1/hardware/history', '/api/1/storage', '/api/1/tasks']);
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
  assert.match(lead, /diskhealth also reads every disk's health once a day; sdb has a warning\.$/);
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
  assert.equal(dialog.querySelectorAll('input').filter((one) => one.getAttribute('type') === 'checkbox').length, 7);
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
  boxes[6].checked = true;
  await world.submit();
  assert.deepEqual(world.ran('hardware.check'), [{ areas: 'memory disks sensors network', full: true }]);
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
  assert.match(card, /All of it, from the start menu\. Restart, choose Troubleshooting, then Test the memory \(memtest86\+\)\./);
  assert.match(card, /Last run: Nov 9 2:10 AM · passed, 1\.0G tested by 4 CPUs/);
  assert.match(card, /The test from the start menu leaves no record\./);
  assert.ok(!card.includes('Test at the next restart'), 'memtest86+ does not end by itself');
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
