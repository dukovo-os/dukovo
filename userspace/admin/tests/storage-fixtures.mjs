// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Storage page reads, as `storage show --json` prints it (design
// 11.1), for the tests of its logic: a machine with a system disk, a mirror
// of two disks and a USB disk, and the changes of it that each test makes.

import { NOW } from './overview-fixtures.mjs';

export { NOW };

export const MB = 1e6;
export const GB = 1e9;
export const TB = 1e12;

/** A partition of a disk as `storage show` gives it. */
export const partition = (name, number, start, size, more = {}) => ({
  name, number, start, size, type: 'linux', gpt_name: '', filesystem: null, label: '', uuid: '', mounts: [], ...more,
});

/** A disk as `storage show` gives it. */
export const disk = (name, model, size, partitions, more = {}) => ({
  name, model, serial: `${name.toUpperCase()}-SERIAL`, size, removable: false, rotational: false, bus: 'sata', partitions, free_after: 0, health: null, ...more,
});

/** The health of a SATA disk that passed. */
export const passed = (more = {}) => ({
  verdict: 'passed', checked_ms: 1_763_120_000_000, hours: 21804, temperature_c: 38, moved: 0, waiting: 0, offline: 0, media_errors: null, used_percent: null, rose: null, ...more,
});

/** A Btrfs volume as `storage show` gives it. */
export const volume = (uuid, label, mount, devices, more = {}) => ({
  uuid, label, mount, size: 2_000_397_795_328, used: 1_240_000_000_000, free: 580_000_000_000, devices, disks: devices.length, missing: 0, behind: 0,
  profiles: { data: ['single'], metadata: ['dup'], system: ['dup'] }, holds: [], scrub: null, ...more,
});

/** The profiles of a mirror of two disks. */
export const MIRROR = Object.freeze({ data: ['raid1'], metadata: ['raid1'], system: ['raid1'] });

/** The result of the last scrub, Nov 1 2026, 10:12 UTC, 13 days before the fixtures' time; it took 2 hours 55 minutes. */
export const SCRUB = Object.freeze({ at_ms: (NOW - 13 * 86400) * 1000, seconds: 10512, bytes: 1_240_000_000_000, errors: 0, fixed: 0, unfixable: 0, result: 'ok' });

/** The system disk: an NVMe disk with the start-up partition and the system volume. */
export const nvme = () => disk('nvme0', 'Samsung PM981', 256_060_514_304, [
  partition('nvme0n1p1', 1, 1_048_576, 536_870_912, { type: 'efi', filesystem: 'vfat', label: 'ESP', mounts: ['/boot/efi'] }),
  partition('nvme0n1p2', 2, 538_968_064, 255_000_000_000, { type: 'dukovo-root', filesystem: 'btrfs', label: 'dukovo-root', uuid: 'aaaa-root', mounts: ['/'] }),
], { bus: 'nvme', free_after: 500_000_000, health: passed({ temperature_c: 41, moved: null, waiting: null, offline: null, media_errors: 0, used_percent: 4, hours: 4120 }) });

/** One disk of the data mirror. */
export const mirrored = (name) => disk(name, 'WD Red Plus WD20EFZX', 2_000_398_934_016, [
  partition(`${name}1`, 1, 1_048_576, 2_000_397_795_328, { filesystem: 'btrfs', label: 'data', uuid: 'bbbb-data', gpt_name: 'data', mounts: name === 'sda' ? ['/volumes/data', '/users', '/shared'] : [] }),
], { rotational: true, health: passed() });

/** A disk on a USB port with nothing on it. */
export const stick = () => disk('sdc', 'Seagate Expansion', 4 * TB, [], { removable: true, bus: 'usb', free_after: 4 * TB });

/** The system volume: one disk, data single, metadata dup. */
export const system = () => volume('aaaa-root', 'dukovo-root', '/', ['nvme0n1p2'], { size: 255 * GB, used: 61 * GB, free: 194 * GB });

/** The data volume: a mirror of sda and sdb that holds the people's folders. */
export const data = (more = {}) => volume('bbbb-data', 'data', '/volumes/data', ['sda1', 'sdb1'], { profiles: MIRROR, holds: ['/users', '/shared'], scrub: SCRUB, ...more });

/** The mounted filesystems that are not Btrfs. */
export const filesystems = () => [{ mount: '/boot/efi', type: 'vfat', device: 'nvme0n1p1', size: 536_870_912, used: 41_943_040, free: 494_927_872 }];

/** `GET /api/1/storage` for a machine with the system disk, the mirror and the stick. */
export const show = (changes = {}) => ({
  format: 1,
  disks: [nvme(), mirrored('sda'), mirrored('sdb'), stick()],
  volumes: [system(), data()],
  filesystems: filesystems(),
  ...changes,
});

/** The early warning of a disk whose count of moved sectors rose, a day before the fixtures' time. */
export const ROSE = Object.freeze({ at_ms: NOW * 1000 - 20 * 3600 * 1000, title: 'sdb moved 8 worn-out sectors to spares' });

/** `show` with sdb wearing: 8 moved sectors, up since yesterday. */
export function showWearing() {
  const answer = show();
  answer.disks[2].health = passed({ verdict: 'warning', moved: 8, rose: ROSE });
  return answer;
}

/** `show` with sdb failing. */
export function showFailing() {
  const answer = show();
  answer.disks[2].health = passed({ verdict: 'failing', moved: 120, waiting: 4 });
  return answer;
}

/** `show` with sdb gone from the mirror: one disk left, one missing. */
export function showMissing() {
  const answer = show();
  answer.disks = answer.disks.filter((one) => one.name !== 'sdb');
  answer.volumes[1] = data({ devices: ['sda1'], disks: 2, missing: 1 });
  return answer;
}

/** `show` with sdb back and behind. */
export function showBehind() {
  const answer = show();
  answer.volumes[1] = data({ behind: 1 });
  return answer;
}

/** `show` for a machine with one disk and no mirror. */
export function showAlone() {
  return show({ disks: [nvme()], volumes: [system()] });
}

/** `show` for a machine whose people's folders are on the system volume. */
export function showSystemAndData() {
  const answer = showAlone();
  answer.volumes[0] = volume('aaaa-root', 'dukovo-root', '/', ['nvme0n1p2'], { size: 255 * GB, used: 61 * GB, free: 194 * GB, holds: ['/users', '/shared'] });
  return answer;
}

/** `show` with the mirror's data `single` over two disks. */
export function showSpread() {
  const answer = show();
  answer.volumes[1] = data({ profiles: { data: ['single'], metadata: ['raid1'], system: ['raid1'] } });
  return answer;
}

/** `show` with a balance changing the data from `single` to `raid1`. */
export function showConverting() {
  const answer = show();
  answer.volumes[1] = data({ profiles: { data: ['single', 'raid1'], metadata: ['raid1'], system: ['raid1'] } });
  return answer;
}

/** `show` with the system volume's data `dup`. */
export function showDup() {
  const answer = showAlone();
  answer.volumes[0].profiles = { data: ['dup'], metadata: ['dup'], system: ['dup'] };
  return answer;
}

/** `show` for a disk that can grow: sda has 40 GB after its last partition. */
export function showGrowable() {
  const answer = show();
  answer.disks[1].free_after = 40 * GB;
  return answer;
}

/** The points `[unix seconds, value]` of one series, `step` seconds apart from `start`, with `values`. */
export const series = (start, step, values) => values.map((value, at) => [start + at * step, value]);

/**
 * `GET /api/1/storage/usage/30d` for the data volume, as `metrics` answers
 * it with a point every 6 hours: `used` grows by `perDay` bytes a day for
 * `days` days, ending at `NOW`.
 */
export function usage30d({ used = 1_240_000_000_000, perDay = 3_000_000_000, days = 30, size = 2_000_397_795_328 } = {}) {
  const step = 6 * 3600;
  const count = days * 4 + 1;
  const start = NOW - days * 86400;
  const values = Array.from({ length: count }, (_, at) => used - (count - 1 - at) * (perDay / 4));
  return [
    { series: 'fs.used{/volumes/data}', points: series(start, step, values) },
    { series: 'fs.size{/volumes/data}', points: series(start, step, values.map(() => size)) },
  ];
}
