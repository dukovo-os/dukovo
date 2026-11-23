// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the RAID part of the Storage page says (the read side of controller
// support), as plain values, from admind's `storage.raid` answer (`storage raid
// --json`, `GET /api/1/storage/raid`) and with no DOM: the banner for a failed
// disk or a volume that is degraded or offline, the controllers, the front
// bays, the volumes with their task bars and the physical disks with the health
// the controller reports. Every string a person reads is made here, so that a
// test reaches each one. `raid` is the answer, `{controllers: [...]}`; a
// machine with no controller has an empty list and nothing here is drawn.
// raid.js draws these values; the disk cards of the page use `raidVolumes`.

import { bytesWords, counted } from '../../lib/size-words.js';

/** Where the answer is read. */
export const ROUTE = '/api/1/storage/raid';

/** The sentence under a battery that makes the controller write straight to disk. */
export const WRITES_STRAIGHT = 'Writes go straight to disk until the battery charges';
const WRITES_STRAIGHT_BROKEN = 'Writes go straight to disk until the battery is replaced';

const LEVELS = Object.freeze({
  raid0: 'RAID 0', raid1: 'RAID 1', raid5: 'RAID 5', raid6: 'RAID 6', raid00: 'RAID 00', raid10: 'RAID 10', raid50: 'RAID 50', raid60: 'RAID 60', raid1e: 'RAID 1E',
});

/** Whether `raid` is an answer of `storage.raid`. */
export const isAnswer = (raid) => raid !== null && typeof raid === 'object' && Array.isArray(raid.controllers);

/** The controllers of `raid`, or none when it is not an answer. */
export const controllersOf = (raid) => (isAnswer(raid) ? raid.controllers : []);

const listOf = (value) => (Array.isArray(value) ? value : []);
const known = (value) => value !== null && value !== undefined && value !== '';
const whole = (value) => (Number.isFinite(value) && value > 0 ? Math.floor(value) : 0);

/** `RAID 1` for `raid1`; a level the console does not know is just "RAID". */
export const levelWords = (level) => LEVELS[level] ?? 'RAID';

/** A volume's name as a person says it: the name it was given, else "volume 0" by its target. */
export const volumeNameOf = (volume) => (known(volume.name) ? String(volume.name) : `volume ${volume.target}`);

/** `0`, `0 and 1`, `2, 3, 5 and 6`; with `joiner` "or", `2, 3, 5 or 6`. */
export function listWords(items, joiner = 'and') {
  if (items.length <= 1) return items.join('');
  return `${items.slice(0, -1).join(', ')} ${joiner} ${items[items.length - 1]}`;
}

/** The bays of `disks` in words: "Bay 4", "Bays 0 and 1", "Slots 2 and 3" for a controller with no backplane; with `joiner` "or", "Bays 2, 3, 5 or 6". */
export function baysWords(disks, joiner = 'and') {
  if (disks.length === 0) return 'No disks';
  const direct = disks.every((disk) => !known(disk.enclosure));
  const noun = direct ? 'Slot' : 'Bay';
  // With more than one backplane a number alone would be ambiguous, so each bay says its own.
  if (disks.some((disk) => /backplane/.test(String(disk.bay)))) return listWords(disks.map((disk) => disk.bay), joiner);
  const slots = disks.map((disk) => disk.slot);
  return `${noun}${slots.length === 1 ? '' : 's'} ${listWords(slots, joiner)}`;
}

/** Lower-case first letter, for a bay's name in the middle of a sentence ("the disk in bay 4"). */
const lower = (words) => words.charAt(0).toLowerCase() + words.slice(1);

// ---- time ---------------------------------------------------------------------

/**
 * The time left of a task that is `percent` done after `elapsed` seconds, from
 * the pace so far: "about 1 h 10 min left". A task with no pace yet says that
 * it is working it out. `duration` is lib/time.js's.
 */
export function timeLeftWords(percent, elapsed, duration) {
  if (!Number.isFinite(percent) || !Number.isFinite(elapsed) || percent <= 0 || elapsed <= 0) return 'working out the time left';
  if (percent >= 100) return 'finishing';
  const left = (elapsed * (100 - percent)) / percent;
  return left < 60 ? 'less than a minute left' : `about ${duration(left)} left`;
}

// ---- the controller ------------------------------------------------------------

const MODES = Object.freeze({
  raid: 'RAID: its disks are in volumes',
  hba: 'Pass-through: its disks are used as they are',
  mixed: 'RAID volumes and pass-through disks',
});

const MIB = 1024 * 1024;

/**
 * The cache's size as its label says it: a controller's memory is sold in
 * powers of two, so 8 589 934 592 bytes is "8 GB", not the 8.59 GB that the
 * disks' thousands would give.
 */
export function cacheSize(bytes) {
  if (!(bytes > 0)) return null;
  if (bytes % MIB !== 0) return bytesWords(bytes);
  const mib = bytes / MIB;
  return mib >= 1024 && mib % 1024 === 0 ? `${mib / 1024} GB` : `${mib} MB`;
}

/** How the controller's cache treats writes, from the write policy its volumes have now. */
function cacheWords(controller, volumes) {
  const size = cacheSize(controller.cache);
  if (size === null) return 'No cache memory';
  const back = volumes.filter((volume) => volume.write === 'back').length;
  const through = volumes.filter((volume) => volume.write === 'through').length;
  if (volumes.length === 0) return size;
  if (through === 0 && back > 0) return `${size}, writes held in the cache (write back)`;
  if (back === 0) return `${size}, writes go straight to disk (write through)`;
  return `${size}, writes held in the cache for some volumes and not for others`;
}

const BATTERY = Object.freeze({
  optimal: { word: 'Charged', tone: 'ok', note: 'Writes in the cache are kept if the power goes.' },
  charging: { word: 'Charging', tone: 'warn', note: null },
  discharging: { word: 'Running on the battery', tone: 'warn', note: 'The power to the controller is off or low.' },
  learning: { word: 'Learning', tone: 'off', note: 'The battery is testing its own capacity, which takes a few hours.' },
  degraded: { word: 'Worn', tone: 'warn', note: 'The battery holds less than it should. Plan to replace it.' },
  failed: { word: 'Failed', tone: 'bad', note: 'The battery cannot keep the cache. Replace it.' },
  missing: { word: 'Not found', tone: 'bad', note: 'The controller has no battery in its place. Fit one, or check the cable.' },
});

/**
 * The battery of a controller as `{present, word, tone, note, writeThrough}`:
 * `word` is the state and the charge ("Charging, 40%"), `note` the sentence under
 * it. A controller with none says so, and says what that means when a volume
 * holds writes in the cache.
 */
export function batteryWords(battery, volumes) {
  if (!battery) {
    const risky = volumes.some((volume) => volume.write === 'back');
    return {
      present: false,
      word: 'None',
      tone: risky ? 'warn' : 'off',
      note: risky ? 'Writes held in the cache are lost if the power goes.' : 'No battery or cache vault is fitted.',
      writeThrough: false,
    };
  }
  const base = BATTERY[battery.state] ?? { word: String(battery.state ?? 'Unknown'), tone: 'off', note: null };
  const charge = Number.isFinite(battery.charge) ? `, ${Math.round(battery.charge)}%` : '';
  const broken = battery.state === 'failed' || battery.state === 'missing' || battery.state === 'degraded';
  const note = battery.write_through === true ? (broken ? WRITES_STRAIGHT_BROKEN : WRITES_STRAIGHT) : base.note;
  return {
    present: true,
    word: `${base.word}${battery.state === 'charging' ? charge : ''}`,
    tone: battery.write_through === true && base.tone === 'ok' ? 'warn' : base.tone,
    note: note ? `${note}${/[.!?]$/.test(note) ? '' : '.'}` : null,
    writeThrough: battery.write_through === true,
  };
}

/** the firmware version, with the package it came in when it has both: "51.16.0 (package 51.16.0-5150)". */
function firmwareWords(controller) {
  if (known(controller.firmware) && known(controller.package)) return `${controller.firmware} (package ${controller.package})`;
  return known(controller.firmware) ? String(controller.firmware) : (known(controller.package) ? `package ${controller.package}` : 'Not known');
}

/** The controller's own state: working, or the fault it reports. */
function stateWords(controller) {
  if (controller.state === 'fault') return { word: 'Fault', tone: 'bad', note: known(controller.fault) ? `The firmware reports the fault ${controller.fault}.` : 'The firmware reports a fault.' };
  return { word: 'Working', tone: 'ok', note: null };
}

/**
 * The controllers' card: for each `{key, title, state, rows}`. `rows` are `{label,
 * value, tone, note}`; the title is the model and the firmware version.
 */
export function controllerRows(raid) {
  return controllersOf(raid).map((entry) => {
    const controller = entry.controller ?? {};
    const volumes = listOf(entry.volumes);
    const firmware = known(controller.firmware) ? controller.firmware : controller.package;
    const battery = batteryWords(entry.battery ?? null, volumes);
    const rows = [
      { label: 'Mode', value: MODES[controller.mode] ?? 'Not known' },
      { label: 'Chip', value: known(controller.chip) ? String(controller.chip) : 'Not known' },
      { label: 'Firmware', value: firmwareWords(controller) },
      { label: 'Cache', value: cacheWords(controller, volumes) },
      { label: 'Battery', value: battery.word, tone: battery.tone, note: battery.note, pill: battery.present },
    ];
    if (Number.isFinite(controller.temperature)) rows.push({ label: 'Temperature', value: `${controller.temperature} °C` });
    const state = stateWords(controller);
    if (state.note) rows.unshift({ label: 'State', value: state.word, tone: state.tone, note: state.note, pill: true });
    return {
      key: `controller-${entry.n}`,
      n: entry.n,
      title: [controller.model || 'RAID controller', firmware].filter(known).join(' · '),
      state,
      rows,
    };
  });
}

// ---- the disks ------------------------------------------------------------------

/** The volume of `entry` with the target `target`, or null. */
const volumeOf = (entry, target) => listOf(entry.volumes).find((volume) => volume.target === target) ?? null;

const byBay = (a, b) => (a.index ?? -1) - (b.index ?? -1) || a.slot - b.slot;

/** The disks of `entry` that are members of `volume` now, by bay: the volume's own list, else the disks that name it. */
function membersOf(entry, volume) {
  const disks = listOf(entry.disks);
  const named = listOf(volume.disks).map((id) => disks.find((disk) => disk.id === id)).filter(Boolean);
  return (named.length > 0 ? named : disks.filter((disk) => disk.volume === volume.target)).sort(byBay);
}

/** The disks of `volume` that are lost to it: a failed or offline one that the volume lists or that still names it. */
const lostOf = (entry, volume) => listOf(entry.disks)
  .filter((disk) => (disk.state === 'failed' || disk.state === 'offline') && (disk.volume === volume.target || listOf(volume.disks).includes(disk.id)))
  .sort(byBay);

const KIND_MEDIA = { hdd: 'HDD', ssd: 'SSD' };

/** "HDD · SAS", "SSD · SATA", or whatever of the two is known. */
export function kindWords(disk) {
  return [KIND_MEDIA[disk.media], known(disk.interface) ? String(disk.interface).toUpperCase() : null].filter(Boolean).join(' · ') || 'Not known';
}

/** "480 GB SSD", "1.2 TB SAS": what a bay says of the disk in it: an SSD says so, else the way it is connected. */
function bayKind(disk) {
  const kind = disk.media === 'ssd' ? 'SSD' : (known(disk.interface) ? String(disk.interface).toUpperCase() : KIND_MEDIA[disk.media]);
  return [bytesWords(disk.size), kind].filter(Boolean).join(' ');
}

/** "boot · RAID 1" for a disk that is in a volume, else null. */
const volumeTag = (entry, disk) => {
  const volume = disk.volume === null || disk.volume === undefined ? null : volumeOf(entry, disk.volume);
  return volume ? `${volumeNameOf(volume)} · ${levelWords(volume.level)}` : null;
};

/**
 * What a bay says of its disk, `{kind, word, sub}`: `kind` is the class that
 * draws it (`use`, `rebuild`, `fail`, `spare`, `ready`, `direct`, `off`),
 * `word` its state, and `sub` the lines under it.
 */
export function bayState(entry, disk) {
  const tag = volumeTag(entry, disk);
  const volume = disk.volume === null || disk.volume === undefined ? null : volumeOf(entry, disk.volume);
  const percent = Number.isFinite(disk.percent) ? `${Math.round(disk.percent)}%` : null;
  switch (disk.state) {
    case 'online':
      return { kind: 'use', word: 'In use', sub: tag ?? 'in a volume' };
    case 'rebuilding':
      return { kind: 'rebuild', word: 'Rebuilding', sub: [percent, volume ? `into ${volumeNameOf(volume)}` : null].filter(Boolean).join(' · ') || 'rebuilding' };
    case 'copyback':
      return { kind: 'rebuild', word: 'Copying back', sub: [percent, volume ? `into ${volumeNameOf(volume)}` : null].filter(Boolean).join(' · ') || 'copying back' };
    case 'failed':
      return { kind: 'fail', word: 'Failed', sub: volume ? `was in ${volumeNameOf(volume)}` : 'stopped working' };
    case 'offline':
      return { kind: 'fail', word: 'Offline', sub: volume ? `out of ${volumeNameOf(volume)}` : 'taken out of its volume' };
    case 'hot-spare':
      return { kind: 'spare', word: 'Hot spare', sub: disk.spare === 'dedicated' ? 'for one volume' : 'for any volume' };
    case 'unconfigured-good':
      return { kind: 'ready', word: 'Ready', sub: 'not in a volume' };
    case 'unconfigured-bad':
      return { kind: 'fail', word: 'Not usable', sub: 'the controller marked it bad' };
    case 'jbod':
      return { kind: 'direct', word: 'Used directly', sub: known(disk.os) ? `as ${disk.os}` : 'without RAID' };
    default:
      return { kind: 'off', word: 'Other', sub: 'state not known' };
  }
}

/** A bay's name when no disk is in it, in the way `bay` names the ones that have one. */
function emptyBayName(slot, index, enclosures, direct) {
  if (direct) return `Slot ${slot}`;
  return enclosures > 1 ? `Bay ${slot}, backplane ${index + 1}` : `Bay ${slot}`;
}

/**
 * The front of the server for a controller: `{key, title, groups}`; `groups`
 * is one row of bays for each backplane, `{key, title, bays}` with `title` null
 * for a machine with one; a bay is `{key, name, empty, kind, word, sub, label}`.
 * A slot between the first and the last that holds no disk is drawn empty; the
 * answer does not say how many slots the backplane has, so the drawing ends at
 * the last slot with a disk.
 */
export function bayGroups(entry) {
  const disks = listOf(entry.disks);
  const byEnclosure = new Map();
  for (const disk of disks) {
    const key = known(disk.enclosure) ? disk.enclosure : 'direct';
    if (!byEnclosure.has(key)) byEnclosure.set(key, { index: disk.index, disks: [] });
    byEnclosure.get(key).disks.push(disk);
  }
  const enclosures = [...byEnclosure.keys()].filter((key) => key !== 'direct').length;
  const groups = [...byEnclosure.entries()]
    .sort(([, a], [, b]) => (a.index ?? -1) - (b.index ?? -1))
    .map(([key, group]) => {
      const direct = key === 'direct';
      const last = Math.max(...group.disks.map((disk) => whole(disk.slot)));
      const bays = [];
      for (let slot = 0; slot <= last; slot += 1) {
        const disk = group.disks.find((one) => one.slot === slot);
        if (!disk) {
          const name = emptyBayName(slot, group.index ?? 0, enclosures, direct);
          bays.push({ key: `empty-${key}-${slot}`, name, empty: true, kind: 'empty', word: 'Empty', sub: null, size: null, label: `${name}: empty` });
          continue;
        }
        const state = bayState(entry, disk);
        const size = bayKind(disk);
        bays.push({
          key: `disk-${disk.id}`, name: disk.bay, empty: false, ...state, size, id: disk.id,
          label: `${disk.bay}: ${state.word.toLowerCase()}${state.sub ? `, ${state.sub}` : ''}, ${size}`,
        });
      }
      return { key: String(key), title: enclosures > 1 && !direct ? `Backplane ${(group.index ?? 0) + 1}` : null, bays };
    });
  return { key: `bays-${entry.n}`, n: entry.n, title: controllerTitle(entry), groups };
}

const controllerTitle = (entry) => entry.controller?.model || 'RAID controller';

// ---- the health of a disk ---------------------------------------------------------

/**
 * What the controller reports of a disk's health, `{word, tone, line}`: the
 * word is its worst sign, the line the numbers (temperature, media errors,
 * predictive failures). A disk that failed is not answering.
 */
export function diskHealth(disk) {
  const media = whole(disk.media_errors);
  const other = whole(disk.other_errors);
  const predictive = whole(disk.predictive);
  const temperature = Number.isFinite(disk.temperature) ? `${disk.temperature} °C` : null;
  if (disk.state === 'failed') return { word: 'Not answering', tone: 'bad', line: temperature ?? '' };
  const line = [
    temperature,
    media > 0 ? counted(media, 'media error') : 'no media errors',
    other > 0 ? counted(other, 'other error') : null,
    predictive > 0 ? counted(predictive, 'predictive failure') : null,
  ].filter(Boolean).join(' · ');
  if (disk.smart_alert === true) return { word: 'Expects to fail', tone: 'bad', line: `The drive's own warning is on · ${line}` };
  if (predictive > 0) return { word: 'Predicted to fail', tone: 'bad', line };
  if (media > 0) return { word: 'Media errors', tone: 'warn', line };
  return { word: 'No errors', tone: 'ok', line };
}

/** The "state" cell of the physical disks: the state, and the line under it. */
function diskStateCell(entry, disk, failedAt, time) {
  const state = bayState(entry, disk);
  const volume = disk.volume === null || disk.volume === undefined ? null : volumeOf(entry, disk.volume);
  const tone = { use: 'ok', rebuild: 'warn', fail: 'bad', spare: 'off', ready: 'off', direct: 'off', off: 'off' }[state.kind];
  const percent = Number.isFinite(disk.percent) ? `, ${Math.round(disk.percent)}%` : '';
  const word = disk.state === 'rebuilding' || disk.state === 'copyback' ? `${state.word}${percent}` : state.word;
  let note = null;
  if (disk.state === 'online') note = volume ? `${levelWords(volume.level)} ${volumeNameOf(volume)}` : null;
  else if (disk.state === 'rebuilding' || disk.state === 'copyback') note = volume ? `Now in ${levelWords(volume.level)} ${volumeNameOf(volume)}` : null;
  else if (disk.state === 'failed') note = failedAt === null ? null : `Stopped answering at ${time.clock(failedAt)}`;
  else if (disk.state === 'unconfigured-good') note = 'In no volume';
  else if (disk.state === 'hot-spare') note = disk.spare === 'dedicated' ? 'Takes over for one volume' : 'Takes over for any volume';
  else if (disk.state === 'jbod') note = known(disk.os) ? `Used directly as ${disk.os}` : 'Used without RAID';
  else if (disk.state === 'offline') note = volume ? `Taken out of ${levelWords(volume.level)} ${volumeNameOf(volume)}` : null;
  return { word, tone, note };
}

/** The seconds of an event's time (RFC 3339), or null when the controller only counted from its own start. */
const eventSeconds = (event) => {
  const ms = typeof event.time === 'string' ? Date.parse(event.time) : Number.NaN;
  return Number.isFinite(ms) ? Math.floor(ms / 1000) : null;
};

/** When `disk` failed, in Unix seconds, from the newest critical event that names it; null when none says. */
export function failedAt(entry, disk) {
  const events = listOf(entry.events).filter((event) => event.disk === disk.id && (event.class === 'critical' || event.class === 'fatal'));
  const newest = events.sort((a, b) => b.seq - a.seq)[0];
  return newest ? eventSeconds(newest) : null;
}

/**
 * The physical disks' card: for each controller `{key, title, rows}`, a row for each
 * disk by bay: `{key, bay, disk, details, kind, state, health}`. `time` is lib/time.js's.
 */
export function diskTables(raid, time) {
  return controllersOf(raid).map((entry) => {
    const rows = [...listOf(entry.disks)]
      .sort((a, b) => (a.index ?? -1) - (b.index ?? -1) || a.slot - b.slot)
      .map((disk) => ({
        key: `disk-${disk.id}`,
        bay: disk.bay,
        disk: [disk.vendor, disk.model].filter(known).join(' ').trim() === '' ? bytesWords(disk.size) : `${[disk.vendor, disk.model].filter(known).join(' ')}, ${bytesWords(disk.size)}`,
        details: [known(disk.serial) ? `Serial ${disk.serial}` : null, known(disk.revision) ? `firmware ${disk.revision}` : null].filter(Boolean).join(' · '),
        kind: kindWords(disk),
        state: diskStateCell(entry, disk, failedAt(entry, disk), time),
        health: diskHealth(disk),
      }));
    return { key: `disks-${entry.n}`, n: entry.n, title: controllerTitle(entry), rows };
  });
}

// ---- the volumes -----------------------------------------------------------------

const BASE_STATES = Object.freeze({
  optimal: { word: 'Optimal', tone: 'ok' },
  'partly-degraded': { word: 'Partly degraded', tone: 'warn' },
  degraded: { word: 'Degraded', tone: 'warn' },
  offline: { word: 'Offline', tone: 'bad' },
});

const TASKS = Object.freeze({
  check: 'Checking consistency',
  init: 'Preparing the volume',
  'background-init': 'Preparing the volume in the background',
  reconstruct: 'Reconstructing',
  rebuild: 'Rebuilding',
  copyback: 'Copying back',
});

/**
 * The task a volume is running, `{words, percent, left}` or null: the volume's own
 * (a check, a preparation, a reconstruction), else the rebuild of one of its
 * disks. `percent` is null when the controller gave none yet.
 */
export function volumeTask(entry, volume, duration) {
  let task = volume.task;
  let percent = volume.percent;
  let elapsed = volume.elapsed;
  if (!task || task === 'none') {
    const rebuilding = membersOf(entry, volume).find((disk) => disk.task === 'rebuild' || disk.task === 'copyback');
    if (!rebuilding) return null;
    ({ task, percent, elapsed } = rebuilding);
  }
  const words = TASKS[task] ?? 'Working';
  const done = Number.isFinite(percent) ? Math.min(100, Math.max(0, Math.round(percent))) : null;
  return {
    words,
    percent: done,
    left: done === null ? null : timeLeftWords(done, elapsed, duration),
    line: done === null ? words : `${words}, ${done}%, ${timeLeftWords(done, elapsed, duration)}`,
  };
}

/** The levels that lose their data on one more failure when degraded, and the ones that may take another. */
const SECOND_FAILURE_OK = new Set(['raid6', 'raid60']);

/** The note under a volume that is not in order, from what it is and which bays hold it. */
function volumeNote(volume, task, orBays) {
  const rebuilding = task !== null && (task.words === 'Rebuilding' || task.words === 'Copying back');
  if (volume.state === 'offline') return 'The controller cannot read this volume. Its data is out of reach until enough of its disks are back.';
  if (volume.state === 'optimal') return null;
  const risk = SECOND_FAILURE_OK.has(volume.level)
    ? `more failures in ${lower(orBays)} can lose the data`
    : `one more failure in ${lower(orBays)} can lose the data`;
  const partly = volume.state === 'partly-degraded' ? 'Part of the volume has lost a disk' : 'The volume has lost a disk';
  return rebuilding ? `Degraded until the rebuild ends: ${risk}.` : `${partly}, and nothing is rebuilding it: ${risk}.`;
}

const POLICY = { back: 'writes held in the cache', through: 'writes go straight to disk' };
const READ_POLICY = { ahead: 'reads ahead', none: 'no reading ahead' };

/** "64 KB stripe · writes held in the cache, reads ahead", and when the battery forces write through, what was asked for. */
function cacheLine(volume) {
  const stripe = Number.isFinite(volume.stripe) && volume.stripe > 0 ? `${Math.round(volume.stripe / 1024)} KB stripe` : null;
  const write = POLICY[volume.write] ?? null;
  const forced = volume.default_write === 'back' && volume.write === 'through' ? 'set to write back, writing straight to disk now because of the battery' : null;
  return [stripe, forced ?? write, READ_POLICY[volume.read] ?? null].filter(Boolean).join(' · ') || 'Not known';
}

/**
 * The stack line of a volume, from what is on top to the bays: `["volume data",
 * "sda", "RAID 1", "Bays 0 and 1"]`. `holders` says which volume of the machine sits
 * on `sda` (`storage show`'s volumes, from `holdersOf`); with none known the line
 * starts at the disk.
 */
export function stackOf(volume, members, holders = []) {
  const top = holders.length > 0 ? [`volume ${listWords(holders)}`] : [];
  const os = known(volume.os) ? [String(volume.os)] : ['not shown to the system'];
  return [...top, ...os, levelWords(volume.level), baysWords(members)];
}

/**
 * The volumes' card: for each controller `{key, title, rows}`, a row for each volume:
 * `{key, name, head, size, state, task, note, bays, bayNote, cache, stack, os}`.
 * `storage` is `storage show`'s answer, used for the name of the volume that sits on a
 * volume's disk; `duration` is lib/time.js's.
 */
export function volumeCards(raid, { storage = null, duration }) {
  return controllersOf(raid).map((entry) => ({
    key: `volumes-${entry.n}`,
    n: entry.n,
    title: controllerTitle(entry),
    rows: listOf(entry.volumes).map((volume) => {
      const members = membersOf(entry, volume);
      const task = volumeTask(entry, volume, duration);
      const base = BASE_STATES[volume.state] ?? { word: String(volume.state ?? 'Unknown'), tone: 'off' };
      const rebuilding = task !== null && (task.words === 'Rebuilding' || task.words === 'Copying back');
      const bays = baysWords(members);
      const spare = members.find((disk) => disk.state === 'rebuilding' && known(disk.spare));
      const lost = lostOf(entry, volume)[0];
      return {
        key: `volume-${entry.n}-${volume.target}`,
        name: volumeNameOf(volume),
        target: volume.target,
        head: `${levelWords(volume.level)} · ${bytesWords(volume.size)}`,
        state: { word: rebuilding ? 'Rebuilding' : base.word, tone: base.tone },
        task,
        note: volumeNote(volume, task, baysWords(members, 'or')),
        bays: bays + (spare ? ` · ${lower(spare.bay)}, the hot spare, is taking ${lost ? `the place of ${lower(lost.bay)}` : 'a failed disk\'s place'}` : ''),
        cache: cacheLine(volume),
        stack: stackOf(volume, members, holdersOf(storage, volume.os)),
        os: known(volume.os) ? volume.os : null,
      };
    }),
  }));
}

/** The names of the volumes of `storage show` that sit on the disk named `os`. */
function holdersOf(storage, os) {
  if (!known(os) || !storage) return [];
  const disk = listOf(storage.disks).find((one) => one.name === os);
  if (!disk) return [];
  const partitions = listOf(disk.partitions).map((partition) => partition.name);
  return listOf(storage.volumes)
    .filter((volume) => listOf(volume.devices).some((device) => partitions.includes(device)))
    .map((volume) => String(volume.label || volume.mount || '').replace(/^dukovo-/, ''))
    .filter(Boolean);
}

// ---- the banner ------------------------------------------------------------------

/** `Bay 4` for the failed disk `disk`, "the disk in bay 4". */
const diskIn = (disk) => `the disk in ${lower(disk.bay)}`;

/** The disks of `entry` that failed, offline ones included. */
const failedOf = (entry) => listOf(entry.disks).filter((disk) => disk.state === 'failed');

/**
 * The banners, worst first, each `{id, tone, lead, detail, replace}`: a volume that
 * is offline, a disk that failed (and what took its place), a volume that is
 * degraded with no failed disk to blame, and a controller in fault. `replace`
 * carries what the steps to replace a disk need, or is null. Nothing for a machine
 * with no controller, or whose disks and volumes are in order.
 */
export function raidBanners(raid, { time, duration = time?.duration }) {
  const found = [];
  for (const entry of controllersOf(raid)) {
    const volumes = listOf(entry.volumes);
    const failed = failedOf(entry);
    for (const volume of volumes.filter((one) => one.state === 'offline')) {
      found.push({
        id: `raid-offline-${entry.n}-${volume.target}`,
        tone: 'bad',
        lead: `The volume ${volumeNameOf(volume)} is offline.`,
        detail: `The controller cannot read it, so its data is out of reach until enough of its disks are back. Its disks are in ${lower(baysWords(membersOf(entry, volume)))}.`,
        replace: failed.length > 0 ? { n: entry.n, bays: failed.map((disk) => disk.bay) } : null,
      });
    }
    if (failed.length > 0) found.push(failedBanner(entry, failed, { time, duration }));
    // A failed disk's banner says what the degraded volume does; a volume degraded with no failed disk to blame has its own.
    for (const volume of failed.length > 0 ? [] : volumes.filter((one) => one.state === 'degraded' || one.state === 'partly-degraded')) {
      const task = volumeTask(entry, volume, duration);
      const orBays = baysWords(membersOf(entry, volume), 'or');
      found.push({
        id: `raid-degraded-${entry.n}-${volume.target}`,
        tone: 'warn',
        lead: `The volume ${volumeNameOf(volume)} is degraded.`,
        detail: task
          ? `${task.line}. Until it ends, one more failure in ${lower(orBays)} can lose the data.`
          : `It has lost a disk and nothing is rebuilding it, so one more failure in ${lower(orBays)} can lose the data. Look at the disks below for the one that is missing.`,
        replace: null,
      });
    }
    if (entry.controller?.state === 'fault') {
      found.push({
        id: `raid-fault-${entry.n}`,
        tone: 'bad',
        lead: `The RAID controller ${controllerTitle(entry)} reports a fault.`,
        detail: known(entry.controller.fault) ? `The firmware gives the fault ${entry.controller.fault}. Its volumes may stop answering.` : 'Its volumes may stop answering.',
        replace: null,
      });
    }
  }
  const rank = { bad: 0, warn: 1 };
  return found.sort((a, b) => rank[a.tone] - rank[b.tone]);
}

/** The banner for the disks of `entry` that failed. */
function failedBanner(entry, failed, { time, duration }) {
  const first = failed[0];
  const at = failedAt(entry, first);
  const volume = first.volume === null || first.volume === undefined ? null : volumeOf(entry, first.volume);
  const hurt = volume ?? listOf(entry.volumes).find((one) => one.state === 'degraded' || one.state === 'partly-degraded') ?? null;
  const replacement = hurt ? membersOf(entry, hurt).find((disk) => disk.state === 'rebuilding') : null;
  const task = hurt ? volumeTask(entry, hurt, duration) : null;
  const lead = failed.length === 1
    ? `The disk in ${lower(first.bay)} failed${at === null ? '' : ` at ${time.clock(at)}`}.`
    : `${failed.length} disks failed: ${listWords(failed.map((disk) => lower(disk.bay)))}.`;
  let detail;
  if (!hurt) detail = 'It was in no volume, so no data is at risk. Replace it when you can.';
  else if (replacement && task) {
    const who = known(replacement.spare) ? `The hot spare in ${lower(replacement.bay)}` : `The disk in ${lower(replacement.bay)}`;
    detail = `${who} took its place, and the volume ${volumeNameOf(hurt)} is rebuilding onto it: ${task.line.replace(/^[^,]*, /, '')}. Until the rebuild ends, one more failure in ${volumeNameOf(hurt)} loses it.`;
  } else if (hurt.state === 'offline') detail = `The volume ${volumeNameOf(hurt)} is offline, and its data is out of reach.`;
  else detail = `No spare took its place, so the volume ${volumeNameOf(hurt)} stays degraded until the disk is replaced. One more failure in it can lose the data.`;
  return {
    id: `raid-failed-${entry.n}-${first.id}`,
    tone: 'bad',
    lead,
    detail,
    replace: { n: entry.n, bays: failed.map((disk) => disk.bay) },
  };
}

/**
 * The pill that sits by the page's title, `{words, kind}`, or null: the first
 * failed bay, else the first volume that is not in order.
 */
export function headPill(raid) {
  for (const entry of controllersOf(raid)) {
    const failed = failedOf(entry);
    if (failed.length === 1) return { words: `${failed[0].bay} failed`, kind: 'bad' };
    if (failed.length > 1) return { words: `${failed.length} disks failed`, kind: 'bad' };
    const bad = listOf(entry.volumes).find((volume) => volume.state !== 'optimal');
    if (bad) return { words: bad.state === 'offline' ? 'Volume offline' : 'Volume degraded', kind: bad.state === 'offline' ? 'bad' : 'warn' };
  }
  return null;
}

// ---- the disk cards of the page ----------------------------------------------------

/**
 * A map of the `os` names of the RAID volumes (`sda`) to what the disk cards say of
 * them: `{controller, target, name, level, health: {word, tone, note}, bays, line,
 * members, failedBays, replace}`. `health` is the volume's own state, and `bays` where
 * its disks are. A disk that is not a RAID volume is not in it.
 */
export function raidVolumes(raid) {
  const found = new Map();
  for (const entry of controllersOf(raid)) {
    for (const volume of listOf(entry.volumes)) {
      if (!known(volume.os)) continue;
      const members = membersOf(entry, volume);
      const state = BASE_STATES[volume.state] ?? { word: 'Unknown', tone: 'off' };
      const failed = lostOf(entry, volume);
      const bays = baysWords(members);
      found.set(String(volume.os), {
        controller: controllerTitle(entry),
        n: entry.n,
        target: volume.target,
        name: volumeNameOf(volume),
        level: levelWords(volume.level),
        health: { word: state.word, tone: state.tone, note: `A ${levelWords(volume.level)} volume on ${controllerTitle(entry)}. Its health is the controller's: ${state.word.toLowerCase()}.` },
        bays,
        line: `A ${levelWords(volume.level)} volume on ${controllerTitle(entry)}, made of ${lower(bays)}`,
        members: members.map((disk) => disk.bay),
        failedBays: failed.map((disk) => disk.bay),
        // The steps are of use while a disk of it is not in order.
        replace: volume.state !== 'optimal' || failed.length > 0,
      });
    }
  }
  return found;
}

/**
 * The steps to replace a failed disk of a RAID volume, as plain sentences: the controller
 * rebuilds the volume by itself, so what a person does is at the bay. `bays` are the failed
 * bays' names (empty when the page does not know which one), `volume` a `raidVolumes` value or null.
 */
export function replaceSteps({ bays = [], volume = null } = {}) {
  const where = bays.length > 0 ? listWords(bays.map(lower)) : (volume && volume.failedBays.length > 0 ? listWords(volume.failedBays.map(lower)) : 'the bay that shows as failed');
  const mine = volume ? ` Its disks are in ${lower(volume.bays)}.` : '';
  return {
    intro: `This disk is a RAID volume, so you replace the failed disk in its bay, not the volume.${mine} The controller does the copying; this console shows how far it is.`,
    steps: [
      'Look at the Bays card. The failed disk is the red bay, and the table of physical disks gives its model and serial number so that you pull the right one.',
      'If a hot spare took its place, the volume is rebuilding already. Wait for the rebuild to end, or replace the failed disk now: the volume is safe from one more failure only after the rebuild ends.',
      `Pull the failed disk from ${where}. Do not pull a disk from a bay that shows In use: a second disk out can lose the data.`,
      'Put in a new disk that is as big as the one it replaces, or bigger, in the same bay.',
      'The controller usually starts rebuilding onto it by itself, and the volume\'s card shows the progress. A new disk that shows Ready and does not start a rebuild has to be told to take the failed disk\'s place, which this page cannot do yet.',
    ],
  };
}
