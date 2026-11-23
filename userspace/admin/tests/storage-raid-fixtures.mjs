// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What admind's `storage.raid` route answers (`storage raid --json`), as the tests of
// the RAID part of the Storage page need it: a healthy Dell PERC H740P with a RAID 1 volume
// and a pass-through SSD, a RAID 5 volume that lost a disk and rebuilds onto a spare, a
// controller with no battery, a battery that forces write through, and a machine with no
// controller. The shape is the one in plan/phase19/step5-kernel-raid.md ("JSON"). Not a test
// itself, and never installed.

import { GB, NOW } from './storage-fixtures.mjs';

const plain = (value) => JSON.parse(JSON.stringify(value));

/** A physical disk as the answer gives it; `slot` names the bay. */
export const raidDisk = (id, slot, more = {}) => ({
  id, enclosure: 32, index: 0, slot, bay: `Bay ${slot}`, state: 'online', interface: 'sas', media: 'hdd', vendor: 'SEAGATE', model: 'ST1200MM0099',
  serial: `WFK0A${id}`, revision: 'ST31', size: 1_200_243_695_616, raw_size: 1_200_243_695_616, media_errors: 0, other_errors: 0, predictive: 0,
  smart_alert: false, temperature: 35, volume: null, spare: null, os: null, task: 'none', percent: null, elapsed: null, ...more,
});

/** A 480 GB SATA SSD. */
export const ssd = (id, slot, more = {}) => raidDisk(id, slot, {
  interface: 'sata', media: 'ssd', vendor: 'INTEL', model: 'SSDSC2KB480G8', serial: `PHYG${id}`, revision: 'XCV10132', size: 480_103_981_056, temperature: 31, ...more,
});

/** A volume as the answer gives it. */
export const raidVolume = (target, name, level, disks, more = {}) => ({
  target, name, level, size: 599_550_590_976, stripe: 65536, state: 'optimal', write: 'back', default_write: 'back', read: 'ahead', disk_cache: 'default',
  disks, os: `sd${'abcdef'[target]}`, task: 'none', percent: null, elapsed: null, ...more,
});

/** The controller's part of the answer. */
export const controller = (more = {}) => ({
  model: 'PERC H740P Mini', serial: 'SERIAL0', package: '51.16.0-5150', firmware: '5.16.0', interface: 'fusion', chip: 'SAS3508', pci: '0000:3b:00.0', numa: 0,
  cache: 8_589_934_592, mode: 'mixed', battery: 'cachevault', temperature: 61, state: 'operational', fault: null, volumes: 1, degraded: 0, offline: 0, disks: 3, critical: 0, failed: 0, commands: 256, ...more,
});

/** A battery that is charged. */
export const battery = (more = {}) => ({
  type: 'cachevault', state: 'optimal', charge: 100, temperature: 28, voltage: 9450, current: 0, learn: 'none', write_through: false, flags: [], ...more,
});

/** One controller of the answer. */
export const entry = (parts = {}) => ({
  n: 0, controller: controller(), volumes: [], disks: [], battery: battery(), events: [], ...parts,
});

/** The answer for `controllers`. */
export const answerOf = (...controllers) => ({ controllers });

/** The time of the failure in the events, 09:14 UTC on the fixtures' day. */
export const FAILED_AT = '2026-11-14T09:14:00Z';
export { NOW };

/** A machine with no RAID controller. */
export const noController = () => ({ controllers: [] });

/** A healthy H740P Mini: boot is RAID 1 on bays 0 and 1 (it is sda), and an SSD in bay 2 is passed through as sdb. */
export const healthy = () => plain(answerOf(entry({
  volumes: [raidVolume(0, 'boot', 'raid1', [0, 1], { os: 'sda', size: 479_559_942_144 })],
  disks: [
    ssd(0, 0, { volume: 0, size: 480_103_981_056 }),
    ssd(1, 1, { volume: 0, size: 480_103_981_056, temperature: 30 }),
    ssd(2, 2, { state: 'jbod', os: 'sdb', serial: 'PHYG2' }),
  ],
})));

/**
 * A server of eight bays that has lost the disk in bay 4: the data volume (RAID 5 of bays 2, 3, 5 and 6) rebuilds onto the spare in
 * bay 6, 42% in after 3030 seconds, and bay 7 is a disk in no volume.
 */
export function failedBay(more = {}) {
  return plain(answerOf(entry({
    controller: controller({ volumes: 2, degraded: 1, disks: 8, failed: 1 }),
    volumes: [
      raidVolume(0, 'boot', 'raid1', [0, 1], { os: 'sda', size: 479_559_942_144 }),
      raidVolume(1, 'data', 'raid5', [2, 3, 5, 6], { os: 'sdb', size: 3_600_731_087_872, stripe: 262_144, state: 'degraded' }),
    ],
    disks: [
      ssd(0, 0, { volume: 0 }),
      ssd(1, 1, { volume: 0, temperature: 30 }),
      raidDisk(2, 2, { volume: 1 }),
      raidDisk(3, 3, { volume: 1, temperature: 36, media_errors: 2 }),
      raidDisk(4, 4, { state: 'failed', volume: 1, temperature: null }),
      raidDisk(5, 5, { volume: 1 }),
      raidDisk(6, 6, {
        state: 'rebuilding', volume: 1, spare: 'global', temperature: 41, task: 'rebuild', percent: 42, elapsed: 3030,
      }),
      raidDisk(7, 7, { state: 'unconfigured-good', temperature: 33 }),
    ],
    events: [
      {
        seq: 1, time: '2026-11-14T08:00:00Z', boot_relative: null, class: 'info', locale: ['disk'], code: 112, volume: null, disk: 4, enclosure: 0, slot: 4, bay: 'Bay 4', percent: null, text: 'Patrol read',
      },
      {
        seq: 2, time: FAILED_AT, boot_relative: null, class: 'critical', locale: ['disk'], code: 81, volume: 1, disk: 4, enclosure: 0, slot: 4, bay: 'Bay 4', percent: null, text: 'PD 04 failed',
      },
    ],
    ...more,
  })));
}

/** The same server with a volume offline and no spare. */
export function offlineVolume() {
  const answer = failedBay();
  const [first] = answer.controllers;
  first.volumes[1].state = 'offline';
  first.disks[6] = raidDisk(6, 6, { state: 'failed', volume: 1 });
  return answer;
}

/** A healthy machine whose controller has no battery. */
export function noBattery() {
  const answer = healthy();
  answer.controllers[0].battery = null;
  answer.controllers[0].controller.battery = 'none';
  return answer;
}

/** A healthy machine whose battery is charging, so that the volume set to write back writes through. */
export function writeThrough() {
  const answer = healthy();
  const [first] = answer.controllers;
  first.battery = battery({ state: 'charging', charge: 40, write_through: true });
  first.volumes[0].write = 'through';
  return answer;
}
