// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Hardware page says, from the commands' own JSON and with no DOM:
// the machine's words, the last check's rows and the checks before it, the
// memory test's last run, and what the check dialog sends.

import assert from 'node:assert/strict';
import test from 'node:test';

import { AREAS, checkArgs } from '../pages/hardware/check-model.js';
import { healthWords } from '../lib/disk-model.js';
import {
  disksOf, pillOf, testArgs, testsOf,
} from '../pages/hardware/disks-model.js';
import {
  degrees, fanRows, noRecordWords, startTimes, tempRows, unitOf,
} from '../pages/hardware/sensors-model.js';
import {
  amountWords, checkRows, dateWords, diskNotes, earlierRows, endedCheck, firmwareWords, hasResult, memoryRunWords, memoryWords, MEMORY_TEST_ARGS,
  modelWords, moduleWords, processorWords, stateOf, summaryWords, tookBackWords, upWords,
} from '../pages/hardware/model.js';
import {
  about, broken, clean, diskHealth, highest, history, inventory, last, NOW, sensors, show, skipped, storage, task, time, updates,
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
    lead: 'diskhealth reads each disk\'s health every 10 minutes, but a sleeping one', notes: [{ words: 'sdb has a warning', href: '/storage/sdb' }],
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

// ---- Temperatures and fans ----------------------------------------------------

const seconds = Math.floor(NOW / 1000);
const rowsOf = (answer = sensors(), more = {}) => tempRows(answer, highest(), {
  unit: 'C', time, now: seconds, starts: startTimes(updates()), ...more,
});
const rowNamed = (found, name) => found.rows.find((row) => row.name.startsWith(name));

test('a temperature is whole degrees in Celsius or Fahrenheit, and Celsius is the unit unless Fahrenheit was kept', () => {
  assert.equal(degrees(41.4, 'C'), '41 °C');
  assert.equal(degrees(41, 'F'), '106 °F');
  assert.equal(degrees(-3.5, 'F'), '26 °F');
  assert.deepEqual([unitOf('F'), unitOf('C'), unitOf(null), unitOf('K'), unitOf(undefined)], ['F', 'C', 'C', 'C', 'C']);
});

test('the rows are the processor, then each disk, then the other sensors and the thermal zones, each with its limit', () => {
  const found = rowsOf();
  assert.deepEqual(found.rows.map((row) => row.name), ['Processor (Package id 0)', 'Disk sda · WDC WD40EFRX-68N', 'Disk nvme0 · Samsung SSD 980', 'Disk sdb · WDC WD40EFRX-68N', 'Zone acpitz']);
  assert.equal(found.none, false);
  const [processor] = found.rows;
  assert.deepEqual(processor.now, { text: '41 °C' });
  assert.deepEqual(processor.limit, { text: '100 °C', note: 'critical' });
  assert.deepEqual(found.rows.at(-1).limit, { text: '97 °C', note: 'critical' });
  const board = tempRows({ sensors: [{ chip: 'nct6775-isa-0a30', label: 'SYSTIN', kind: 'temp', series: 'temp{nct6775-isa-0a30/SYSTIN}', value: 30, high: 80, crit: null }], zones: [], disks: [] }, [], { unit: 'C', time, now: seconds });
  assert.equal(board.rows[0].name, 'SYSTIN');
  assert.equal(board.rows[0].small, 'nct6775-isa-0a30');
  assert.deepEqual(board.rows[0].limit, { text: '80 °C', note: 'high' });
});

test('the highest says when it was, and that an update was on when a change of the system is within half an hour of it', () => {
  const [processor, sda, nvme] = rowsOf().rows;
  assert.deepEqual(processor.highest, { text: '63 °C', note: 'Yesterday 3:02 AM, during the update' });
  assert.deepEqual(sda.highest, { text: '41 °C', note: 'Yesterday 3:10 AM, during the update' });
  assert.deepEqual(nvme.highest, { text: '58 °C', note: 'Nov 12 2:14 PM' });
  assert.deepEqual(rowsOf(sensors(), { starts: [] }).rows[0].highest.note, 'Yesterday 3:02 AM');
  assert.deepEqual(rowsOf(sensors(), { starts: [seconds - 40 * 60 - 7 * 3600] }).rows[0].highest.note, 'Yesterday 3:02 AM');
  const none = rowsOf(sensors(), { starts: [] });
  assert.deepEqual(rowNamed(none, 'Zone').highest, { text: '—', muted: true });
  assert.deepEqual(tempRows(sensors(), null, { unit: 'C', time, now: seconds }).rows[0].highest, { text: '—', muted: true });
});

test('a highest from before the minute files reach is the hour it was in', () => {
  const old = [{ series: 'temp{coretemp-isa-0000/Package id 0}', highest: [Date.UTC(2026, 9, 20, 3, 0, 0) / 1000, 71] }];
  const [processor] = tempRows(sensors(), old, { unit: 'C', time, now: seconds }).rows;
  assert.deepEqual(processor.highest, { text: '71 °C', note: 'in the hour from Oct 20 3:00 AM' });
});

test('every temperature, the highest and the limit change with the unit', () => {
  const [processor, sda, nvme] = rowsOf(sensors(), { unit: 'F' }).rows;
  assert.equal(processor.now.text, '106 °F');
  assert.equal(processor.highest.text, '145 °F');
  assert.equal(processor.limit.text, '212 °F');
  assert.equal(sda.limit.text, '122 °F, critical 140 °F');
  assert.equal(nvme.limit.text, '158 °F, critical 185 °F');
});

test('a disk says when it was read, and why it is not, whether it sleeps or diskhealth has stopped', () => {
  const [, sda, nvme, sdb] = rowsOf().rows;
  assert.deepEqual(sda.now, { text: '36 °C', note: 'read at 3:10 PM' });
  assert.deepEqual(sda.limit, { text: '50 °C, critical 60 °C', note: 'for a hard disk' });
  assert.deepEqual(nvme.limit, { text: '70 °C, critical 85 °C', note: 'the disk\'s own' });
  assert.deepEqual(sdb.now, { text: 'Asleep since 2:10 AM: not read, so as not to wake it', muted: true });
  const stale = sensors();
  stale.disks[0].checked_ms = (seconds - 40 * 60) * 1000;
  assert.deepEqual(rowsOf(stale).rows[1].now, { tone: 'warn', words: 'Last read at 2:32 PM' });
  const old = sensors();
  old.disks[0].checked_ms = Date.UTC(2026, 10, 13, 1, 40, 0);
  assert.deepEqual(rowsOf(old).rows[1].now, { tone: 'warn', words: 'Last read yesterday 1:40 AM' });
  const nothing = sensors();
  nothing.disks[0].temperature_c = null;
  nothing.disks[0].warning_c = null;
  nothing.disks[0].critical_c = null;
  assert.deepEqual(rowsOf(nothing).rows[1].now, { text: 'The disk gives no temperature', muted: true });
  assert.deepEqual(rowsOf(nothing).rows[1].limit, { text: '—', muted: true });
  const flash = sensors();
  flash.disks[0].kind = 'ssd';
  assert.equal(rowsOf(flash).rows[1].limit.note, 'for a solid-state disk');
});

test('a sensor within 5 degrees of its limit is in the warning colour, and at it in the error colour, with the words', () => {
  const at = (value, more = {}) => rowsOf({ sensors: [{ chip: 'coretemp-isa-0000', label: 'Package id 0', kind: 'temp', series: 's', value, high: 84, crit: 100, ...more }], zones: [], disks: [] }).rows[0].now;
  assert.deepEqual(at(94), { text: '94 °C' });
  assert.deepEqual(at(95), { text: '95 °C', tone: 'warn', words: 'near its limit' });
  assert.deepEqual(at(100), { text: '100 °C', tone: 'bad', words: 'at its limit' });
  assert.deepEqual(at(80, { crit: null }), { text: '80 °C', tone: 'warn', words: 'near its limit' });
  assert.deepEqual(at(60, { crit: null, high: null }), { text: '60 °C' });
  assert.deepEqual(at(null), { text: '—', muted: true });
});

test('a disk at its warning limit is in the warning colour and at its critical limit in the error colour, with the words', () => {
  const at = (value) => {
    const answer = sensors();
    answer.disks[0].temperature_c = value;
    return rowsOf(answer).rows[1].now;
  };
  assert.deepEqual(at(49), { text: '49 °C', note: 'read at 3:10 PM' });
  assert.deepEqual(at(50), { text: '50 °C', note: 'read at 3:10 PM', tone: 'warn', words: 'over its warning limit' });
  assert.deepEqual(at(60), { text: '60 °C', note: 'read at 3:10 PM', tone: 'bad', words: 'over its critical limit' });
});

test('a machine with no sensor says so and keeps the disks\' rows, and one with nothing at all has no rows', () => {
  const found = rowsOf({ format: 1, sensors: [], zones: [], disks: sensors().disks });
  assert.equal(found.none, true);
  assert.deepEqual(found.rows.map((row) => row.name.split(' ')[1]), ['sda', 'nvme0', 'sdb']);
  assert.deepEqual(rowsOf({ format: 1, sensors: [], zones: [], disks: [] }), { rows: [], none: true });
  assert.deepEqual(rowsOf(null), { rows: [], none: true });
});

test('a fan says its speed, stopped when it has an alarm, a fault or no speed under a minimum, and not judged with no minimum', () => {
  assert.deepEqual(fanRows(sensors()).map((fan) => [fan.name, fan.words, fan.tone]), [
    ['fan1', '1,190 RPM', 'ok'],
    ['fan2', 'not judged: no fan on this header', 'off'],
  ]);
  const fan = (more) => fanRows({ sensors: [{ chip: 'c', label: 'fan1', kind: 'fan', value: 1000, min: null, alarm: false, fault: false, ...more }] })[0];
  assert.deepEqual([fan({ alarm: true }).words, fan({ alarm: true }).tone], ['stopped', 'bad']);
  assert.equal(fan({ alarm: 1 }).words, 'stopped');
  assert.equal(fan({ fault: true }).words, 'stopped');
  assert.equal(fan({ value: 0, min: 600 }).words, 'stopped');
  assert.equal(fan({ value: 0, min: 0 }).words, 'not judged: no fan on this header');
  assert.deepEqual([fan({ value: 400, min: 600 }).words, fan({ value: 400, min: 600 }).tone], ['400 RPM, under its minimum of 600 RPM', 'bad']);
  assert.deepEqual(fanRows({ sensors: [] }), []);
  assert.deepEqual(fanRows(null), []);
});

test('the disks that keep no health record are counted from the storage answer', () => {
  const none = storage();
  assert.equal(noRecordWords(none), null);
  none.disks.push({ name: 'sdc', model: 'Expansion', health: null });
  assert.equal(noRecordWords(none), '1 disk keeps no health record (USB or virtual) and is not read.');
  none.disks.push({ name: 'vda', model: null, health: null });
  assert.equal(noRecordWords(none), '2 disks keep no health record (USB or virtual) and are not read.');
  assert.equal(noRecordWords(null), null);
});

test('the times of the changes of the system come from the update history, and only those that are times', () => {
  assert.deepEqual(startTimes(updates()), [Date.UTC(2026, 10, 13, 3, 0, 0) / 1000]);
  assert.deepEqual(startTimes({ changes: [{ time: 'soon' }] }), []);
  assert.deepEqual(startTimes(null), []);
});

// ---- Disks ---------------------------------------------------------------------

test('a disk\'s pill is the one Storage\'s list gives it', () => {
  const healths = [null, { verdict: 'passed', rose: null }, { verdict: 'passed', rose: { title: 'x' } }, { verdict: 'warning', rose: null }, { verdict: 'failing', rose: null }];
  for (const health of healths) {
    const { word, tone } = healthWords(health);
    assert.deepEqual(pillOf(health), { word, tone }, JSON.stringify(health));
  }
  assert.deepEqual(pillOf({ verdict: 'passed' }), { word: 'Healthy', tone: 'ok' });
});

test('the disks are the whole disks of the storage answer with their model and pill, and none when it was not read', () => {
  assert.deepEqual(disksOf(storage()), [
    { name: 'sda', model: 'WD Red', pill: { word: 'Healthy', tone: 'ok' }, hasHealth: true },
    { name: 'sdb', model: 'WD Red', pill: { word: 'Wearing', tone: 'warn' }, hasHealth: true },
  ]);
  assert.deepEqual(disksOf({ disks: [{ name: 'vda', model: '', health: null }] }), [{ name: 'vda', model: null, pill: { word: 'Not checked', tone: 'off' }, hasHealth: false }]);
  assert.equal(disksOf(null), null);
  assert.equal(disksOf({}), null);
});

test('a test of one disk is the disks area with its name, and a full one is an extended test', () => {
  assert.deepEqual(testArgs('sda', false), { areas: 'disks', disk: 'sda' });
  assert.deepEqual(testArgs('nvme0n1', true), { areas: 'disks', disk: 'nvme0n1', full: true });
});

test('the last short and extended tests have the day, found from the hours the disk has been on, and the hour they were at', () => {
  assert.deepEqual(testsOf(diskHealth(), time, Math.floor(NOW / 1000)), [
    { words: 'Short test Nov 14, passed (at hour 21,800)', tone: null },
    { words: 'Extended test Nov 6, passed (at hour 21,604)', tone: null },
  ]);
  const failed = diskHealth('sda', { self_tests: [{ kind: 'extended', result: 'failed', at_hours: 21000 }, { kind: 'short', result: 'aborted', at_hours: 20000 }] });
  assert.deepEqual(testsOf(failed, time, Math.floor(NOW / 1000)).map((one) => [one.words, one.tone]), [
    ['Short test Aug 31, aborted (at hour 20,000)', null],
    ['Extended test Oct 12, failed (at hour 21,000)', 'bad'],
  ]);
  const newest = diskHealth('sda', { self_tests: [{ kind: 'short', result: 'passed', at_hours: 21800 }, { kind: 'short', result: 'failed', at_hours: 100 }] });
  assert.equal(testsOf(newest, time, Math.floor(NOW / 1000)).length, 1, 'the log is newest first, and each kind is said once');
});

test('a test with no hours, a disk with no hours and a test that runs are said as far as they are known', () => {
  const now = Math.floor(NOW / 1000);
  assert.equal(testsOf(diskHealth('sda', { hours: null }), time, now)[0].words, 'Short test passed (at hour 21,800)');
  assert.equal(testsOf(diskHealth('sda', { self_tests: [{ kind: 'extended', result: 'passed', at_hours: null }] }), time, now)[0].words, 'Extended test passed');
  assert.equal(testsOf(diskHealth('sda', { self_tests: [{ kind: 'short', result: 'running', at_hours: null }] }), time, now)[0].words, 'Short test running');
  assert.equal(testsOf(diskHealth('sda', { hours: 21000 }), time, now)[0].words.startsWith('Short test Nov 14, passed'), true, 'a test logged after the hours is today\'s');
});

test('a log that cannot be read, a disk that keeps none and a log with neither test say so', () => {
  const now = Math.floor(NOW / 1000);
  assert.deepEqual(testsOf(null, time, now), [{ words: 'Its own tests could not be read' }]);
  assert.deepEqual(testsOf(diskHealth('sda', { self_tests: null }), time, now), [{ words: 'This disk keeps no log of its own tests' }]);
  assert.deepEqual(testsOf(diskHealth('sda', { self_tests: [] }), time, now), [{ words: 'No short or extended test is in its log' }]);
});
