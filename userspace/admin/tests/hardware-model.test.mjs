// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Hardware page says, from the commands' own JSON and with no DOM:
// the machine's words, the last check's rows and the checks before it, the
// memory test's last run, and what the check dialog sends.

import assert from 'node:assert/strict';
import test from 'node:test';

import { AREAS, checkArgs } from '../pages/hardware/check-model.js';
import {
  amountWords, checkRows, dateWords, diskNotes, earlierRows, endedCheck, firmwareWords, hasResult, memoryRunWords, memoryWords, MEMORY_TEST_ARGS,
  modelWords, moduleWords, processorWords, stateOf, summaryWords, tookBackWords, upWords,
} from '../pages/hardware/model.js';
import {
  about, broken, clean, history, inventory, last, NOW, show, skipped, storage, task, time,
} from './hardware-fixtures.mjs';

test('amounts of memory are in GiB from one and in MiB below it, and none for no memory', () => {
  assert.equal(amountWords(16_318_464 * 1024), '15.6 GiB');
  assert.equal(amountWords(8 * 2 ** 30), '8 GiB');
  assert.equal(amountWords(512 * 2 ** 20), '512 MiB');
  for (const none of [null, undefined, 0, -1, Number.NaN]) assert.equal(amountWords(none), null);
});

test('the model is the maker and the model, and the kind of case in lower case', () => {
  assert.equal(modelWords(show()), 'Dell Inc. OptiPlex 7040, mini tower');
  assert.equal(modelWords(show({ chassis: null })), 'Dell Inc. OptiPlex 7040');
  assert.equal(modelWords(show({ maker: null })), 'OptiPlex 7040, mini tower');
  assert.equal(modelWords(show({ maker: ' ', model: '' })), null);
  assert.equal(modelWords(null), null);
});

test('the processor says its cores, its threads when they differ, and its clock', () => {
  assert.equal(processorWords(show().processor), 'Intel Core i5-6500 · 4 cores, 3.2 GHz, up to 3.6 GHz');
  assert.equal(processorWords({ model: 'Intel Xeon E-2236', cores: 6, threads: 12, base_mhz: 3400, max_mhz: 4800 }), 'Intel Xeon E-2236 · 6 cores, 12 threads, 3.4 GHz, up to 4.8 GHz');
  assert.equal(processorWords({ model: 'ARM Cortex-A72', cores: 1, threads: 1, base_mhz: null, max_mhz: 1500 }), 'ARM Cortex-A72 · 1 core, 1.5 GHz');
  assert.equal(processorWords({ model: 'QEMU Virtual CPU', cores: 2, threads: 2, base_mhz: 2000, max_mhz: 2000 }), 'QEMU Virtual CPU · 2 cores, 2 GHz');
  assert.equal(processorWords({ model: null, cores: null, threads: null, base_mhz: null, max_mhz: null }), null);
  assert.equal(processorWords(undefined), null);
});

test('the memory says its slots, its kinds and its error correction only when the tables were read', () => {
  assert.equal(memoryWords(show().memory, 'locked'), '15.6 GiB');
  assert.equal(memoryWords(show().memory, 'reader'), '15.6 GiB · An administrator sees the slots');
  assert.equal(memoryWords(inventory().memory, 'ok'), '15.6 GiB · in 2 of 4 slots · DDR4-2133 · no error correction');
  assert.equal(memoryWords({ ...inventory().memory, ecc: true }, 'ok'), '15.6 GiB · in 2 of 4 slots · DDR4-2133 · error correction');
  assert.equal(memoryWords({ ...inventory().memory, ecc: null }, 'ok'), '15.6 GiB · in 2 of 4 slots · DDR4-2133');
  assert.equal(memoryWords(show().memory, 'ok'), '15.6 GiB · The firmware does not list the slots');
  assert.equal(memoryWords({ bytes: null, ecc: null, slots: null, modules: null }, 'locked'), null);
  const mixed = inventory().memory;
  mixed.modules[1] = { ...mixed.modules[1], kind: 'DDR4', speed: 2400 };
  assert.equal(memoryWords(mixed, 'ok'), '15.6 GiB · in 2 of 4 slots · DDR4-2133 and DDR4-2400 · no error correction');
});

test('a module is told by its slot, its size, its kind and speed, and who made it', () => {
  assert.equal(moduleWords(inventory().memory.modules[0]), 'DIMM A1 · 8 GiB · DDR4-2133 · Samsung M378A1K43BB2-CRC');
  assert.equal(moduleWords({ slot: 'DIMM A2', bytes: null, kind: 'DDR3', speed: null, maker: null, part: null }), 'DIMM A2 · DDR3');
});

test('the firmware is told by its version and its date in words', () => {
  assert.equal(firmwareWords(show().firmware), 'BIOS 1.24.0, Jul 12, 2023');
  assert.equal(firmwareWords({ vendor: null, version: '2.1', date: null }), 'BIOS 2.1');
  assert.equal(firmwareWords({ vendor: null, version: null, date: '07/12/2023' }), null);
  assert.equal(firmwareWords(undefined), null);
  assert.equal(dateWords('07/12/2023'), 'Jul 12, 2023');
  assert.equal(dateWords('2023-07-12'), '2023-07-12');
  assert.equal(dateWords('13/45/2023'), '13/45/2023');
  assert.equal(dateWords(null), null);
});

test('how long the machine has run is told in two units at most', () => {
  assert.equal(upWords(4 * 3600 + 52 * 60), '4 hours 52 minutes');
  assert.equal(upWords(3600), '1 hour');
  assert.equal(upWords(61), '1 minute');
  assert.equal(upWords(59), 'less than a minute');
  assert.equal(upWords(3 * 86_400 + 4 * 3600 + 5 * 60), '3 days 4 hours');
  assert.equal(upWords(86_400), '1 day');
  for (const none of [null, undefined, -1, Number.NaN]) assert.equal(upWords(none), null);
});

test('a start that was a fallback says when the update was taken back', () => {
  assert.equal(tookBackWords(about({ fallback: true, up: 3600 }), time, Math.floor(NOW / 1000)), 'since the update was taken back (2:12 PM)');
  assert.equal(tookBackWords(about(), time, Math.floor(NOW / 1000)), null);
  assert.equal(tookBackWords(about({ fallback: true, up: null }), time, 0), null);
  assert.equal(tookBackWords(null, time, 0), null);
});

test('a result is a result when it has a time, and null is none', () => {
  assert.equal(hasResult(last()), true);
  for (const none of [null, undefined, {}, 'x']) assert.equal(hasResult(none), false);
});

test('the summary counts each result and names the IDs of the warnings and the failures', () => {
  assert.equal(summaryWords(last(), time), 'Last check Nov 9 2:10 AM (a full check): 6 passed, 1 warning (HW-115), 2 skipped.');
  assert.equal(summaryWords(clean(), time), 'Last check Nov 9 2:10 AM (a quick check): 2 passed.');
  assert.equal(summaryWords(broken(), time), 'Last check Nov 9 2:10 AM (a full check): 1 passed, 1 warning (HW-114), 2 failed (HW-113, HW-103).');
  assert.equal(summaryWords(last({ checks: [] }), time), 'Last check Nov 9 2:10 AM (a full check): nothing could be checked.');
  const two = last({ checks: [{ area: 'disks', check: 'a', result: 'warning', detail: '', id: 'HW-115' }, { area: 'disks', check: 'b', result: 'warning', detail: '', id: 'HW-115' }] });
  assert.equal(summaryWords(two, time), 'Last check Nov 9 2:10 AM (a full check): 2 warnings (HW-115).');
});

test('the card is as bad as the worst check', () => {
  assert.deepEqual(stateOf(broken()), { word: 'Failed', tone: 'bad' });
  assert.deepEqual(stateOf(last()), { word: 'Warning', tone: 'warn' });
  assert.deepEqual(stateOf(clean()), { word: 'Passed', tone: 'ok' });
  assert.deepEqual(stateOf(skipped()), { word: 'Skipped', tone: 'off' });
  assert.deepEqual(stateOf(last({ checks: [] })), { word: 'Skipped', tone: 'off' });
});

test('the rows put a failure first, then warnings, passes and what was skipped, each group in the order the command ran', () => {
  const rows = checkRows(broken());
  assert.deepEqual(rows.map((row) => `${row.word} ${row.name}`), ['Failed sda read scan', 'Failed memory test', 'Warning sdb read scan', 'Passed processor load']);
  assert.deepEqual(rows[0], {
    key: 'disks/sda read scan', area: 'disks', name: 'sda read scan', result: 'failed', word: 'Failed', tone: 'bad', detail: 'sector 7 cannot be read', id: 'HW-113', advice: 'Replace the disk.',
  });
  assert.equal(rows[3].id, null);
  assert.equal(rows[3].advice, null);
  const all = checkRows(last());
  assert.deepEqual(all.map((row) => row.result), ['warning', 'passed', 'passed', 'passed', 'passed', 'passed', 'passed', 'skipped', 'skipped']);
  assert.deepEqual(all.at(-1), {
    key: 'network/net1 link', area: 'network', name: 'net1 link', result: 'skipped', word: 'Skipped', tone: 'off', detail: 'no cable', id: null, advice: null,
  });
  assert.deepEqual(checkRows(null), []);
});

test('the checks before the last one are the newest first, and the last one itself is left out', () => {
  const rows = earlierRows(history(), last(), time);
  assert.deepEqual(rows.map((row) => row.words), [
    'Nov 5 7:00 AM · a quick check · 7 passed, 1 warning, 1 skipped · HW-115',
    'Nov 2 7:00 AM · a quick check · 8 passed, 1 skipped',
  ]);
  assert.deepEqual(rows.map((row) => row.key), [String(history()[1].at_ms), String(history()[0].at_ms)]);
  assert.equal(earlierRows(history(), null, time).length, 3, 'with no last result every line is an earlier one');
  assert.deepEqual(earlierRows(null, last(), time), []);
  assert.deepEqual(earlierRows([{ passed: 1 }], last(), time), [], 'a line with no time is left out');
});

test('no more than five earlier checks are listed, and one with nothing to count says so', () => {
  const many = Array.from({ length: 9 }, (_, at) => ({
    at_ms: Date.UTC(2026, 9, 1 + at), full: at % 2 === 0, status: 0, passed: 0, warnings: 0, failed: 0, skipped: 0, ids: [],
  }));
  const rows = earlierRows(many, null, time);
  assert.equal(rows.length, 5);
  assert.match(rows[0].words, /^Oct 9 12:00 AM · a full check · nothing could be checked$/);
  const failing = earlierRows([{ at_ms: Date.UTC(2026, 9, 3), full: false, passed: 1, warnings: 0, failed: 2, skipped: 0, ids: ['HW-113', 'HW-103'] }], null, time);
  assert.match(failing[0].words, /1 passed, 2 failed · HW-113, HW-103$/);
});

test('the disks that need a look are named with a link to each, and none reporting health says nothing', () => {
  assert.deepEqual(diskNotes(storage()), {
    lead: 'diskhealth also reads every disk\'s health once a day', notes: [{ words: 'sdb has a warning', href: '/storage/sdb' }],
  });
  const failing = storage();
  failing.disks[0].health = { verdict: 'failing', rose: null };
  failing.disks[1].health = { verdict: 'passed', rose: { title: 'Sectors are being moved' } };
  assert.deepEqual(diskNotes(failing).notes.map((note) => note.words), ['sda is failing', 'sdb has a warning']);
  assert.deepEqual(diskNotes({ disks: [{ name: 'sda', health: { verdict: 'passed', rose: null } }] }).notes, []);
  assert.equal(diskNotes({ disks: [{ name: 'sda', health: null }] }), null);
  assert.equal(diskNotes(null), null);
});

test('a check that failed or was stopped after the last result is told, and an older one is not', () => {
  const failed = task({ state: 'failed', ended: 1_800_000_000, note: 'hardware ended with status 3' });
  assert.equal(endedCheck([failed], last({ time: 1_700_000_000 })), 'Failed: hardware ended with status 3');
  assert.equal(endedCheck([failed], last({ time: 1_800_000_001 })), null);
  assert.equal(endedCheck([failed], null), 'Failed: hardware ended with status 3');
  assert.equal(endedCheck([task({ state: 'stopped', ended: 1_800_000_000, note: 'stopped by chase' })], null), 'Stopped: stopped by chase');
  assert.equal(endedCheck([task({ state: 'done', ended: 1_800_000_000 }), { ...failed, action: 'updates.apply' }], null), null);
  const newest = [failed, task({ id: 43, state: 'stopped', ended: 1_800_000_500, note: 'stopped by alice' })];
  assert.equal(endedCheck(newest, null), 'Stopped: stopped by alice');
  assert.equal(endedCheck(null, null), null);
});

test('the memory test\'s last run is its memory row, a failure or a warning first', () => {
  assert.equal(memoryRunWords(last(), time), 'Nov 9 2:10 AM · passed, 1.0G tested by 4 CPUs');
  assert.equal(memoryRunWords(broken(), time), 'Nov 9 2:10 AM · failed, 2 errors in 1.0G (HW-103)');
  assert.equal(memoryRunWords(null, time), 'never');
  assert.equal(memoryRunWords(skipped(), time), 'not in the last check, Nov 9 2:10 AM');
  const warned = last({ checks: [{ area: 'memory', check: 'memory errors', result: 'warning', detail: '3 corrected errors counted since the start', id: 'HW-101' }, { area: 'memory', check: 'memory test', result: 'passed', detail: '1.0G tested by 4 CPUs' }] });
  assert.equal(memoryRunWords(warned, time), 'Nov 9 2:10 AM · warning, 3 corrected errors counted since the start (HW-101)');
  const only = last({ checks: [{ area: 'memory', check: 'memory errors', result: 'passed', detail: 'none counted since the start' }] });
  assert.equal(memoryRunWords(only, time), 'Nov 9 2:10 AM · passed, none counted since the start');
});

test('the memory test runs the memory area alone', () => {
  assert.deepEqual(MEMORY_TEST_ARGS, { areas: 'memory' });
  assert.equal(Object.isFrozen(MEMORY_TEST_ARGS), true);
});

test('the areas of the dialog are the six the action takes, and all of them are left out of the arguments', () => {
  const ids = AREAS.map((area) => area.id);
  assert.deepEqual(ids, ['memory', 'disks', 'cpu', 'sensors', 'battery', 'network']);
  assert.deepEqual(checkArgs(ids, false), {});
  assert.deepEqual(checkArgs(ids, true), { full: true });
  assert.deepEqual(checkArgs(['network', 'memory'], false), { areas: 'memory network' }, 'in the order the command checks them');
  assert.deepEqual(checkArgs(['disks'], true), { areas: 'disks', full: true });
  assert.equal(checkArgs([], false), null);
  assert.equal(checkArgs(['gpu'], true), null);
});
