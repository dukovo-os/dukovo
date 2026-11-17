// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the commands behind the Hardware page print, as the tests need it:
// `hardware show`, `hardware last` and `hardware history` as their manuals and
// Rust tests show them, and the answers of the routes around them. Not a
// test itself, and never installed.

import { makeTime } from '../lib/time.js';

/** 3:12 PM on Saturday 14 November 2026, UTC. */
export const NOW = Date.UTC(2026, 10, 14, 15, 12, 0);
export const time = makeTime('UTC', () => NOW);

/** 2:10 AM on Monday 9 November 2026, UTC, in seconds: when the last check ended. */
export const CHECKED = Date.UTC(2026, 10, 9, 2, 10, 0) / 1000;

const plain = (value) => JSON.parse(JSON.stringify(value));

/** `hardware show --json` as anyone gets it: the tables' members are null. */
export const show = (more = {}) => plain({
  format: 1,
  maker: 'Dell Inc.',
  model: 'OptiPlex 7040',
  chassis: 'Mini Tower',
  processor: {
    model: 'Intel Core i5-6500', cores: 4, threads: 4, base_mhz: 3200, max_mhz: 3600,
  },
  memory: {
    bytes: 16_318_464 * 1024, ecc: null, slots: null, modules: null,
  },
  firmware: { vendor: 'Dell Inc.', version: '1.24.0', date: '07/12/2023' },
  ...more,
});

/** The same, as an administrator gets it: with the memory's slots and modules. */
export const inventory = () => {
  const answer = show();
  answer.memory = {
    bytes: 16_318_464 * 1024,
    ecc: false,
    slots: 4,
    modules: [
      {
        slot: 'DIMM A1', bytes: 8 * 2 ** 30, kind: 'DDR4', speed: 2133, maker: 'Samsung', part: 'M378A1K43BB2-CRC',
      },
      {
        slot: 'DIMM B1', bytes: 8 * 2 ** 30, kind: 'DDR4', speed: 2133, maker: 'Samsung', part: 'M378A1K43BB2-CRC',
      },
    ],
  };
  return answer;
};

const line = (area, check, result, detail, more = {}) => ({
  area, check, result, detail, ...more,
});

/** `hardware last --json` of a full check with a warning, a pass in each area and two skipped checks. */
export const last = (more = {}) => plain({
  time: CHECKED,
  version: '0.14.0',
  full: true,
  status: 2,
  checks: [
    line('memory', 'memory test', 'passed', '1.0G tested by 4 CPUs; Test the memory in the start menu tests all of it'),
    line('memory', 'memory errors', 'passed', 'none counted since the start'),
    line('disks', 'sda health', 'passed', 'no reallocated sectors'),
    line('disks', 'sdb health', 'warning', '8 sectors were moved to spares', { id: 'HW-115', advice: 'Keep backups current and have a replacement ready.' }),
    line('cpu', 'processor load', 'passed', '4 CPUs, 10 minutes, no wrong result'),
    line('sensors', 'temperatures', 'passed', 'the highest is 63 °C'),
    line('battery', 'battery', 'skipped', 'this machine has no battery'),
    line('network', 'net0 link', 'passed', '1000 Mbit/s, full duplex'),
    line('network', 'net1 link', 'skipped', 'no cable'),
  ],
  ...more,
});

/** A result whose checks all passed. */
export const clean = () => last({
  full: false,
  status: 0,
  checks: [
    line('memory', 'memory test', 'passed', '1.0G tested by 4 CPUs; Test the memory in the start menu tests all of it'),
    line('cpu', 'processor load', 'passed', '4 CPUs, 1 minute, no wrong result'),
  ],
});

/** A result with a failure and a warning. */
export const broken = () => last({
  status: 1,
  checks: [
    line('disks', 'sdb read scan', 'warning', 'slow sectors', { id: 'HW-114', advice: 'Look at the cable.' }),
    line('disks', 'sda read scan', 'failed', 'sector 7 cannot be read', { id: 'HW-113', advice: 'Replace the disk.' }),
    line('memory', 'memory test', 'failed', '2 errors in 1.0G; Test the memory in the start menu tests all of it', { id: 'HW-103', advice: 'Test the memory from the start menu.' }),
    line('cpu', 'processor load', 'passed', 'no wrong result'),
  ],
});

/** A result in which nothing could be checked. */
export const skipped = () => last({
  status: 3,
  checks: [line('sensors', 'temperatures', 'skipped', 'the kernel reads no sensor')],
});

/** `hardware history --json`: the checks kept, oldest first; the newest is the last check. */
export const history = () => plain([
  {
    at_ms: Date.UTC(2026, 10, 2, 7, 0, 0), full: false, status: 0, passed: 8, warnings: 0, failed: 0, skipped: 1, ids: [],
  },
  {
    at_ms: Date.UTC(2026, 10, 5, 7, 0, 0), full: false, status: 2, passed: 7, warnings: 1, failed: 0, skipped: 1, ids: ['HW-115'],
  },
  {
    at_ms: CHECKED * 1000, full: true, status: 2, passed: 6, warnings: 1, failed: 0, skipped: 2, ids: ['HW-115'],
  },
]);

/** `GET /api/1/console/about`: how long the machine has run, and whether its start was a fallback. */
export const about = (more = {}) => ({
  release: 'Dukovo Server 0.14.0', system: '0.14.0', up: 4 * 3600 + 52 * 60, fallback: false, ...more,
});

/** `GET /api/1/storage`: two disks, one with an early warning. */
export const storage = () => ({
  disks: [
    {
      name: 'sda', model: 'WD Red', health: { verdict: 'passed', rose: null },
    },
    {
      name: 'sdb', model: 'WD Red', health: { verdict: 'warning', rose: null },
    },
  ],
});

const READ = Math.floor(NOW / 1000) - 120;

/**
 * `hardware sensors --json` of a machine with a processor, a board zone, two
 * fans (one with no fan on its header) and three disks: a hard disk read at
 * 3:10 PM, an NVMe drive with limits of its own and a hard disk asleep since
 * 2:10 AM.
 */
export const sensors = (more = {}) => plain({
  format: 1,
  sensors: [
    {
      chip: 'coretemp-isa-0000', label: 'Package id 0', kind: 'temp', series: 'temp{coretemp-isa-0000/Package id 0}', value: 41, high: 84, crit: 100, min: null, alarm: false, fault: false,
    },
    {
      chip: 'nct6775-isa-0a30', label: 'fan1', kind: 'fan', series: 'fan{nct6775-isa-0a30/fan1}', value: 1190, high: null, crit: null, min: 600, alarm: false, fault: false,
    },
    {
      chip: 'nct6775-isa-0a30', label: 'fan2', kind: 'fan', series: 'fan{nct6775-isa-0a30/fan2}', value: 0, high: null, crit: null, min: null, alarm: false, fault: false,
    },
  ],
  zones: [{
    series: 'temp{zone/acpitz}', value: 32, crit: 97,
  }],
  disks: [
    {
      disk: 'sda', model: 'WDC WD40EFRX-68N', kind: 'hdd', temperature_c: 36, checked_ms: READ * 1000, asleep_ms: null, warning_c: 50, critical_c: 60, limits_from: 'kind', series: 'temp{disk/sda}',
    },
    {
      disk: 'nvme0', model: 'Samsung SSD 980', kind: 'nvme', temperature_c: 44, checked_ms: READ * 1000, asleep_ms: null, warning_c: 70, critical_c: 85, limits_from: 'disk', series: 'temp{disk/nvme0}',
    },
    {
      disk: 'sdb', model: 'WDC WD40EFRX-68N', kind: 'hdd', temperature_c: 35, checked_ms: Date.UTC(2026, 10, 14, 2, 0, 0), asleep_ms: Date.UTC(2026, 10, 14, 2, 10, 0), warning_c: 50, critical_c: 60, limits_from: 'kind', series: 'temp{disk/sdb}',
    },
  ],
  ...more,
});

/** `metrics show temp --highest --json` for those series: the highest of the last 30 days, with the time in seconds. */
export const highest = () => plain([
  { series: 'temp{coretemp-isa-0000/Package id 0}', highest: [Date.UTC(2026, 10, 13, 3, 2, 0) / 1000, 63] },
  { series: 'temp{disk/sda}', highest: [Date.UTC(2026, 10, 13, 3, 10, 0) / 1000, 41] },
  { series: 'temp{disk/nvme0}', highest: [Date.UTC(2026, 10, 12, 14, 14, 0) / 1000, 58] },
  { series: 'temp{zone/acpitz}', highest: null },
]);

/** `GET /api/1/updates/history`: a change of the system at 3:00 AM on November 13, half an hour before the processor's highest. */
export const updates = () => ({
  changes: [{
    start: 12, time: '2026-11-13T03:00:00Z', item: 'system', from: '0.13.0', to: '0.14.0', outcome: 'good', error: null,
  }],
  apps: [],
});

/** `GET /api/1/storage/disks/DISK/health` as an administrator gets it: the disk's summary and its log of its own tests, newest first. */
export const diskHealth = (disk = 'sda', more = {}) => plain({
  disk,
  model: 'WD Red',
  verdict: 'passed',
  hours: 21804,
  self_tests: [
    { kind: 'short', result: 'passed', at_hours: 21800 },
    { kind: 'extended', result: 'passed', at_hours: 21604 },
  ],
  ...more,
});

const device = (name, group, more = {}) => ({
  name, group, names: [], ids: '8086:1533', driver: 'igb', driver_told: true, state: 'working', where: 'PCI 0000:03:00.0', hidden: false, ...more,
});

/** `hardware devices --json`: two network cards (one with no cable), a controller, a USB disk and a serial adapter, a screen, a card the system cannot tell the driver of, one with no driver, and two bridges that work. */
export const devices = () => plain({
  format: 1,
  devices: [
    device('Intel I219-LM', 'network', { names: ['net0'], ids: '8086:15b8', driver: 'e1000e', where: 'PCI 0000:00:1f.6' }),
    device('Intel I210', 'network', { names: ['net1'], state: 'no-link' }),
    device('Intel Q170 SATA controller', 'storage', { names: ['sda', 'sdb'], ids: '8086:a102', driver: 'ahci', where: 'PCI 0000:00:17.0' }),
    device('Seagate Expansion', 'storage', { names: ['sdc'], ids: '0bc2:2312', driver: 'usb-storage', where: 'USB 2-1' }),
    device('FTDI FT232R serial adapter', 'usb', { ids: '0403:6001', driver: 'ftdi_sio', where: 'USB 1-2' }),
    device('Intel HD Graphics 530', 'display', { ids: '8086:1912', driver: 'efifb', where: 'PCI 0000:00:02.0' }),
    device('Intel Management Engine interface', 'other', { ids: '8086:a13a', driver: null, state: null, where: 'PCI 0000:00:16.0' }),
    device('Unknown device', 'other', { ids: '1234:5678', driver: null, driver_told: false, state: null, where: 'PCI 0000:04:00.0' }),
    device('Intel PCI bridge', 'other', { ids: '8086:a110', hidden: true, driver: 'pcieport', where: 'PCI 0000:00:1c.0' }),
    device('Intel host bridge', 'other', { ids: '8086:1910', hidden: true, driver: null, where: 'PCI 0000:00:00.0' }),
  ],
});

/** `GET /api/1/hardware/memtest`, which is `hardware memtest --status --json`: nothing asked and no test at a start kept, unless `more` says. */
export const memtest = (more = {}) => plain({
  format: 1, asked: null, last: null, history: [], ...more,
});

/** The test asked for at 3:10 PM on November 14, by chase. */
export const asked = () => ({ at_ms: Date.UTC(2026, 10, 14, 15, 10, 0), by: 'chase' });

/** The last test at a start, as `result` (`passed`, `errors`, `unfinished` or `not-taken`) gives it: at 3:06 AM on November 14, of a 16 GiB machine. */
export const startTest = (result = 'passed', more = {}) => {
  const base = {
    at_ms: Date.UTC(2026, 10, 14, 3, 6, 0), start: 212, result, complete: true, tested_bytes: Math.round(15.2 * 2 ** 30), total_bytes: 16 * 2 ** 30, errors: 0, bad: [], retired: 0, seconds: 240,
  };
  const found = {
    passed: {},
    errors: {
      errors: 3, retired: 2, bad: [{ from: '0x1000000', to: '0x1002fff', errors: 2 }, { from: '0x7f000000', to: '0x7f000fff', errors: 1 }],
    },
    unfinished: {
      complete: false, tested_bytes: 0, errors: 0, seconds: 0,
    },
    'not-taken': {
      complete: false, tested_bytes: 0, total_bytes: 0, seconds: 0,
    },
  }[result];
  return plain({ ...base, ...found, ...more });
};

/** A running `hardware.check` as `GET /api/1/tasks` lists it. */
export const task = (more = {}) => ({
  id: 41, action: 'hardware.check', args: {}, state: 'running', step: 4, steps: 11, words: 'disks sda self-test passed', started: Math.floor(NOW / 1000) - 120, ...more,
});
