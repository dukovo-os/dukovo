// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Answers of `GET /api/1/move/sources` (`backup find --json`) and `POST
// /api/1/move/plan` (`backup restore --machine --plan --json`), in the shapes
// of design 11.6, for the tests of the Move to new hardware page. The times
// are those of snapshots-fixtures.mjs. Not a test itself, and never installed.

import { at, backupsOf, place } from './snapshots-fixtures.mjs';

export { at, backupsOf, context, NOW, place, time } from './snapshots-fixtures.mjs';

const TB = 1_000_000_000_000;

/** The MACs of the old machine's cards and of this machine's. */
export const OLD0 = '52:54:00:aa:00:01';
export const OLD1 = '52:54:00:aa:00:02';
export const NEW0 = '52:54:00:bb:00:01';
export const NEW1 = '52:54:00:bb:00:02';

/** A backup that `backup find` found. */
export const source = (machine, more = {}) => ({
  source: `/volumes/BACKUP/dukovo-backup/${machine}`,
  machine,
  newest: '2026-11-14-013000',
  newest_ms: at(2026, 11, 14, 1, 30),
  volume: 'BACKUP',
  removable: true,
  ...more,
});

/** A key file that `backup find` found. */
export const keyFile = (name, more = {}) => ({
  path: `/volumes/KEYS/${name}`, for: 'nas', public: 'age1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqs3qdy0', ...more,
});

/** `GET /api/1/move/sources`: duke-nas's backups on a stick, another machine's on a fixed disk, and two keys. */
export const found = (more = {}) => ({
  sources: [
    source('duke-nas'),
    source('old-pi', { source: '/volumes/DATA/backups/old-pi', volume: 'DATA', removable: false, newest_ms: at(2026, 11, 12, 22, 5) }),
  ],
  keys: [keyFile('duke-nas-recovery.key'), keyFile('spare.key', { for: 'spare' })],
  ...more,
});

/** `GET /api/1/move/sources` when nothing is plugged in. */
export const nothingFound = () => ({ sources: [], keys: [] });

const cardOld = (name, mac, model, more = {}) => ({ name, mac, model, ...more });

/** The hardware of the old machine and of this one, as a record of format 2 keeps it. */
export const hardware = (more = {}) => ({
  then: {
    cards: [cardOld('net0', OLD0, 'Realtek RTL8111'), cardOld('net1', OLD1, 'Intel I210')],
    disks: [{ model: 'WD Red', size: 2 * TB }, { model: 'WD Red', size: 2 * TB }],
    mirror: true,
    serial: { device: 'ttyUSB0', model: 'FT232R' },
    cpu: 'Intel N100',
  },
  now: {
    cards: [cardOld('net0', NEW0, 'Realtek RTL8125')],
    disks: [{ model: 'Samsung 870', size: 2 * TB }],
    mirror: false,
    serial: null,
    cpu: 'Intel N100',
  },
  ...more,
});

/** One old card with a fixed address, and the new card the plan pairs it with. */
export const card = (name, mac, address, pair = null) => ({
  old: { name, mac, addresses: [address], router: '192.168.1.1' },
  new: pair,
  choice: 'automatic',
});

/** `POST /api/1/move/plan` for a record of format 2: two cards, one fixed address on the first, a mirror that this machine lacks. */
export const plan = (more = {}) => ({
  machine: 'duke-nas',
  at: '2026-11-14-013000',
  at_ms: 1_763_083_800_000,
  record_format: 2,
  release: '0.16.0-dev.412',
  newer_release: false,
  staged: false,
  accounts: ['owner', 'helper'],
  apps: [{ id: 'org.nginx.nginx', version: '1.30.6' }, { id: 'org.postgresql.postgresql', version: '17.2' }],
  places: [
    { path: '/users', folder: 'users', bytes: 412_000_000_000 },
    { path: '/shared', folder: 'shared', bytes: 90_000_000_000 },
    { path: '/system/settings', folder: 'settings', bytes: 120_000 },
    { path: '/applications', folder: 'applications', bytes: 3_000_000_000 },
  ],
  needed: 1.24 * TB,
  free: 1.86 * TB,
  fits: true,
  hardware: hardware(),
  cards: [card('net0', OLD0, '192.168.1.12/24', { name: 'net0', mac: NEW0 })],
  firewall_on_missing: ['deny ping on net1'],
  ...more,
});

/** A record of format 1: no hardware, and no size for a place. */
export const planOne = (more = {}) => plan({
  record_format: 1,
  accounts: null,
  hardware: null,
  places: [{ path: '/users', folder: 'users', bytes: null }, { path: '/shared', folder: 'shared', bytes: null }],
  needed: null,
  free: null,
  fits: true,
  firewall_on_missing: [],
  ...more,
});

/** A plan whose files are bigger than this machine's free space. */
export const planFull = () => plan({ needed: 1.24 * TB, free: 0.9 * TB, fits: false });

/** A plan with two old cards that have fixed addresses, and this machine with two. */
export const planTwoCards = () => plan({
  hardware: hardware({
    now: {
      cards: [cardOld('net0', NEW0, 'Realtek RTL8125'), cardOld('net1', NEW1, 'Intel I226')],
      disks: [{ model: 'Samsung 870', size: 2 * TB }, { model: 'Samsung 870', size: 2 * TB }],
      mirror: true,
      serial: { device: 'ttyUSB0', model: 'FT232R' },
      cpu: 'Intel N100',
    },
  }),
  cards: [
    card('net0', OLD0, '192.168.1.12/24', { name: 'net0', mac: NEW0 }),
    card('net1', OLD1, '10.0.0.5/24', { name: 'net1', mac: NEW1 }),
  ],
  firewall_on_missing: [],
});

/** `GET /api/1/backups` of a machine that was just installed. */
export const freshBackups = () => backupsOf();

/** `GET /api/1/backups` with a whole-machine restore staged. */
export const stagedBackups = () => ({ ...backupsOf(), staged: { from: 'nas', at: '2026-11-14-013000' } });

/** `GET /api/1/backups` with the place nas. */
export const nasBackups = () => backupsOf(place('nas'));
