// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Storage page says (design 4), as plain values, so that a test
// reaches every branch: the banner, the cards of use with their projection,
// the rows of disks to scale, a scrub's progress and the Overview's needs.
// One disk's details are in disk-detail.js and the log's in log-model.js,
// which are loaded when they are drawn. The files of this folder only draw
// what these give. `answer` is `GET /api/1/storage` (`storage show --json`),
// `usage` the answer of `GET /api/1/storage/usage/30d`.

import { combine } from '../../lib/chart-model.js';
import {
  diskOfPartition, healthWords, isMirror, mirrorWords, roleOf, scaled, volumeName, volumeOf, volumeTitle,
} from '../../lib/disk-model.js';
import { bytesWords, counted, enoughHistory, fullIn, roughly } from '../../lib/size-words.js';

const DAY = 86400;

/** The list `name` of `answer`, or an empty one when the answer has none. */
export const listOf = (answer, name) => (Array.isArray(answer?.[name]) ? answer[name] : []);

/** `words` with a capital first letter. */
export const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

/** The volumes that have a partition of `disk` as a device. */
export function volumesOfDisk(disk, answer) {
  const names = (disk.partitions ?? []).map((partition) => partition.name);
  return listOf(answer, 'volumes').filter((volume) => (volume.devices ?? []).some((device) => names.includes(device)));
}

/** The disk of `answer` named `name`, or null. */
export const findDisk = (answer, name) => listOf(answer, 'disks').find((disk) => disk.name === name) ?? null;

/** The disk name that the address after `/storage/` holds, or null for the list; a name that is not text is none either. */
export function diskNamed(rest) {
  const name = rest.replace(/\/$/, '');
  if (name === '') return null;
  try {
    return decodeURIComponent(name);
  } catch {
    return name;
  }
}

/** The address of a disk's page. */
export const diskHref = (name) => `/storage/${encodeURIComponent(name)}`;

/** The mirror's name in a sentence, "the data mirror", or the volume's, "the data volume", when it is not one. */
const mirrorName = (volume) => `${volumeName(volume).replace(/ volume$/, '')} ${isMirror(volume) ? 'mirror' : 'volume'}`;

// ---- the banner ----------------------------------------------------------

/** The button that opens the steps to replace a mirror's disk. */
const REPLACE = Object.freeze({ id: 'replace', label: 'How to replace a mirror disk' });

/** When an early warning was seen, in words: "Seen 20 hours ago." */
export const seenWords = (rose, time) => `Seen ${time.relative(Math.floor(rose.at_ms / 1000))}.`;

/**
 * The banners, worst first, each `{id, tone, lead, detail, buttons}`: a
 * volume that lost a disk, a disk that says it is failing, a count that
 * rose, and a mirror that is behind. A button is `{id, label, ...}`: `scrub`
 * runs a scrub of its `mount`, `replace` opens the steps to replace a
 * disk, `check` goes to the page of its `disk` and reads its health there,
 * and `open` only goes there. `scrubbing` lists the mounts of the volumes
 * that a scrub is checking now, and `time` is lib/time.js's.
 */
export function bannersOf(answer, { scrubbing = [], time }) {
  const disks = listOf(answer, 'disks');
  const volumes = listOf(answer, 'volumes');
  const found = [];
  for (const volume of volumes) {
    if (!(volume.missing > 0)) continue;
    const mirror = isMirror(volume);
    found.push({
      id: `missing-${volume.uuid}`,
      tone: 'bad',
      lead: `${capital(mirrorName(volume))} has lost a disk.`,
      detail: mirror
        ? 'Every file is still there, on one disk now, with no second copy. Find out which disk is missing, then replace it.'
        : 'Some files may be gone. Find out which disk is missing, then reconnect or replace it.',
      buttons: mirror ? [REPLACE] : [],
    });
  }
  for (const disk of disks) {
    if (disk.health?.verdict !== 'failing') continue;
    const mirror = volumesOfDisk(disk, answer).some((volume) => isMirror(volume) && !(volume.missing > 0));
    found.push({
      id: `failing-${disk.name}`,
      tone: 'bad',
      lead: `${disk.name} says it is failing.`,
      detail: mirror
        ? 'Copy what matters off it now, or replace it while the mirror still has a full copy.'
        : 'Copy what matters off it now: it may stop working at any time.',
      buttons: [{ id: 'open', label: `Open ${disk.name}`, disk: disk.name }, ...(mirror ? [REPLACE] : [])],
    });
  }
  for (const disk of disks) {
    const { rose, verdict } = disk.health ?? {};
    if (!rose || verdict === 'failing') continue;
    const inMirror = volumesOfDisk(disk, answer).some((volume) => isMirror(volume) && !(volume.missing > 0) && !(volume.behind > 0));
    found.push({
      id: `rising-${disk.name}`,
      tone: 'warn',
      lead: `${rose.title}.`,
      detail: [
        inMirror
          ? 'It still works and the mirror is healthy, but a count that grows is often the first sign a disk is wearing out. Keep backups current and have a replacement ready.'
          : 'It still works, but a count that grows is often the first sign a disk is wearing out. Its files have no second copy on this machine: keep backups current.',
        seenWords(rose, time),
      ].join(' '),
      buttons: [{ id: 'check', label: `Check ${disk.name} now`, disk: disk.name }, ...(inMirror ? [REPLACE] : [])],
    });
  }
  for (const volume of volumes) {
    if (!(volume.behind > 0) || volume.missing > 0) continue;
    const running = scrubbing.includes(volume.mount);
    found.push({
      id: `behind-${volume.uuid}`,
      tone: 'info',
      lead: running ? `${capital(mirrorName(volume))} is catching up.` : `${capital(mirrorName(volume))} is behind.`,
      detail: running
        ? 'Checking every block is copying the changes that a disk missed.'
        : 'A disk of it missed changes while it was away, so it has older copies of some files. The other disk is read meanwhile. Checking every block copies the changes across.',
      buttons: [{ id: 'scrub', label: isMirror(volume) ? 'Scrub the mirror' : 'Check every block', mount: volume.mount }],
    });
  }
  return found;
}

// ---- the cards of use ----------------------------------------------------------

/** The mounts that are not shown as a card of their own: the start-up partition and what is under it. */
const hidden = (mount) => mount === '/boot' || mount.startsWith('/boot/');

/** The mounts of the partitions that `volume` is on, for the series of metricsd that name a folder. */
function mountsOf(volume, disks) {
  const mounts = [volume.mount];
  for (const device of volume.devices ?? []) {
    for (const disk of disks) for (const partition of disk.partitions ?? []) if (partition.name === device) mounts.push(...(partition.mounts ?? []));
  }
  return [...new Set(mounts.filter(Boolean))];
}

/**
 * What the use of `volume` is going to do, from the answer of `storage.usage`
 * for 30 days: `{words, seconds, days}`, with `seconds` null when there is
 * not enough history; or null when use is level or falls, or when no
 * history was read (`usage` null).
 */
export function projectionOf(volume, disks, usage) {
  if (!Array.isArray(usage)) return null;
  let points = [];
  for (const mount of mountsOf(volume, disks)) {
    points = combine(usage, `fs.used{${mount}}`);
    if (points.length > 0) break;
  }
  const full = fullIn(points, volume.size);
  if (full) return { words: `${full.words} at the rate of the last ${full.days} days`, seconds: full.seconds, days: full.days };
  return enoughHistory(points) ? null : { words: 'Not enough history yet', seconds: null, days: 0 };
}

/** The tone of a meter that is `percent` full: the line metricsd warns at, and the one it calls critical. */
const meterTone = (percent) => (percent >= 97 ? 'bad' : percent >= 90 ? 'warn' : '');

/**
 * The cards, each `{id, title, used, of, percent, tone, pill, note,
 * projection}`: one for each volume, the system first, then one for each
 * other filesystem that is mounted but is not the start-up partition.
 * `used` is "61 GB used", `of` "of 236 GB on nvme0", `pill` the mirror's
 * words (`{word, tone}`) for a volume that has any, `note` a sentence
 * under it, `projection` the words of the projection.
 */
export function cardsOf(answer, usage, { scrubbing = [] } = {}) {
  const disks = listOf(answer, 'disks');
  const cards = [];
  const rank = (volume) => (volume.mount === '/' ? 0 : 1);
  const volumes = [...listOf(answer, 'volumes')].sort((a, b) => rank(a) - rank(b) || String(a.label).localeCompare(String(b.label)));
  for (const volume of volumes) {
    const mirror = mirrorWords(volume, disks, scrubbing.includes(volume.mount));
    const percent = volume.size > 0 ? Math.round((volume.used / volume.size) * 100) : 0;
    const projection = projectionOf(volume, disks, usage);
    const soon = projection?.seconds != null && projection.seconds < FULL_SOON;
    cards.push({
      id: `card-${volume.uuid}`,
      title: volumeTitle(volume),
      used: `${bytesWords(volume.used)} used`,
      of: `of ${bytesWords(volume.size)}${mirror.where ? ` ${mirror.where}` : ''}`,
      percent,
      tone: meterTone(percent) || (soon ? 'warn' : ''),
      pill: mirror.word ? { word: mirror.word, tone: mirror.tone } : null,
      note: mirror.detail,
      projection: projection?.words ?? null,
    });
  }
  for (const one of [...listOf(answer, 'filesystems')].sort((a, b) => String(a.mount).localeCompare(String(b.mount)))) {
    if (hidden(one.mount)) continue;
    const percent = one.size > 0 ? Math.round((one.used / one.size) * 100) : 0;
    const where = diskOfPartition(disks, one.device)?.name;
    cards.push({
      id: `card-${one.mount}`,
      title: one.mount,
      used: `${bytesWords(one.used)} used`,
      of: `of ${bytesWords(one.size)}${where ? ` on ${where}` : ''}`,
      percent,
      tone: meterTone(percent),
      pill: null,
      note: null,
      projection: null,
    });
  }
  return cards;
}

// ---- the disks to scale -------------------------------------------------------------

/** A part narrower than this share of its bar has a number in it, and its words in a key under the bar. */
export const NARROW = 14;

/** The classes that colour a part by what it is for. */
const KINDS = Object.freeze({ system: 'p1', data: 'p2', other: 'p3', small: 'p4', free: 'free' });

/** What a part of a bar is called and coloured as: `{label, kind}`, `kind` one of `KINDS`'s keys. */
function partWords(part, disk, answer) {
  const volumes = listOf(answer, 'volumes');
  if (part.free) {
    return (disk.partitions ?? []).length === 0
      ? { label: `Not used yet · ${bytesWords(part.bytes)}`, kind: 'free' }
      : { label: `Free ${bytesWords(part.bytes)}`, kind: 'free' };
  }
  const { partition } = part;
  if (partition.type === 'efi') return { label: 'ESP', kind: 'small' };
  if (partition.type === 'bios-boot') return { label: 'BIOS boot', kind: 'small' };
  if (partition.type === 'swap' || partition.filesystem === 'swap') return { label: 'Swap', kind: 'small' };
  const volume = volumeOf(partition.name, volumes);
  if (volume) {
    const title = volumeTitle(volume);
    const role = roleOf(volume);
    return { label: `${isMirror(volume) ? `${title} mirror` : title} · ${bytesWords(volume.used)} used`, kind: role === 'data' ? 'data' : role === 'other' ? 'other' : 'system' };
  }
  if (partition.filesystem) return { label: partition.label || usedFor(partition, volumes), kind: 'other' };
  return { label: 'Unused', kind: 'free' };
}

/** The words that name `disk`'s kind of connection, when it is not an internal one: "USB", "Removable", or null. */
const connectionOf = (disk) => (disk.bus === 'usb' ? 'USB' : disk.removable ? 'Removable' : null);

/**
 * A row for each disk, `{name, href, model, line, parts, label, key, health}`:
 * `line` is "WD Red Plus · 2 TB" (and "· USB"), `parts` each `{id, label,
 * kind, percent, number}` (`kind` the class that colours it; `number` the
 * number a narrow part shows, null for the others), `label` the bar's name
 * for a screen reader, `key` the words of the numbered parts, and `health`
 * `{word, tone, note}`.
 */
export function rowsOf(answer) {
  const disks = listOf(answer, 'disks');
  const drawn = new Map(scaled(disks).map((one) => [one.name, one.parts]));
  return disks.map((disk) => {
    let numbers = 0;
    const parts = (drawn.get(disk.name) ?? []).map((part) => {
      const words = partWords(part, disk, answer);
      const narrow = part.percent < NARROW;
      numbers += narrow ? 1 : 0;
      return { id: part.id, label: words.label, kind: KINDS[words.kind], percent: part.percent, number: narrow ? numbers : null, bytes: part.bytes };
    });
    const connection = connectionOf(disk);
    return {
      name: disk.name,
      href: diskHref(disk.name),
      model: disk.model || 'Disk',
      line: [disk.model || 'Disk', bytesWords(disk.size), connection].filter(Boolean).join(' · '),
      parts,
      label: `${disk.name}, ${bytesWords(disk.size)}: ${parts.map((part) => `${part.label.replace(' · ', ', ')} (${bytesWords(part.bytes)})`).join('; ')}`,
      key: parts.filter((part) => part.number !== null).map((part) => `${part.number} ${part.label}`),
      empty: (disk.partitions ?? []).length === 0,
      health: healthWords(disk.health ?? null),
    };
  });
}

// ---- a scrub as a task ---------------------------------------------------------------

/** The action a scrub is run as. */
export const SCRUB_ACTION = 'storage.scrub';

const percentOf = (task) => (Number.isInteger(task.percent) ? Math.min(100, Math.max(0, task.percent)) : null);
const running = (task) => task.state === 'waiting' || task.state === 'running';

/**
 * What the page knows of the scrubs that run: a map of task id to `{id,
 * mount, percent}`. `learn(tasks)` takes the scrubs of `GET /api/1/tasks`;
 * `started(id, mount)` a scrub this page began; `event(data)` one `event:
 * task` and gives `{ended: mount}` for a scrub that ended, `{learn: id}` for
 * a scrub it does not know yet (read it whole), else `{}`. `mounts()` lists
 * the mounts being scrubbed and `of(mount)` the task of one.
 */
export function makeScrubs() {
  const byId = new Map();
  return {
    learn(tasks) {
      byId.clear();
      for (const task of Array.isArray(tasks) ? tasks : []) {
        if (task.action === SCRUB_ACTION && running(task)) byId.set(task.id, { id: task.id, mount: task.args?.mount ?? null, percent: percentOf(task) });
      }
    },
    started(id, mount) {
      byId.set(id, { id, mount, percent: null });
    },
    event(data) {
      if (data?.action !== SCRUB_ACTION) return {};
      const before = byId.get(data.id);
      if (!running(data)) {
        byId.delete(data.id);
        return before ? { ended: before.mount, state: data.state } : {};
      }
      if (!before) return { learn: data.id };
      byId.set(data.id, { ...before, percent: percentOf(data) ?? before.percent });
      return {};
    },
    /** Takes a scrub read whole, which has its arguments. */
    put(task) {
      if (task.action === SCRUB_ACTION && running(task)) byId.set(task.id, { id: task.id, mount: task.args?.mount ?? null, percent: percentOf(task) });
    },
    mounts: () => [...byId.values()].map((one) => one.mount).filter(Boolean),
    of: (mount) => [...byId.values()].find((one) => one.mount === mount) ?? null,
  };
}

/** The words on the button of a scrub that runs: "Scrubbing… 41%". */
export const scrubbingWords = (entry) => (entry.percent === null ? 'Scrubbing…' : `Scrubbing… ${entry.percent}%`);


// ---- the Overview's needs ---------------------------------------------------------------------

/** How soon a full volume is a thing to look at, in seconds. */
export const FULL_SOON = 30 * DAY;

/**
 * The things the Overview lists for `answer`, worst first as the Overview
 * sorts them: a volume that lost a disk, a disk that says it is failing, a
 * count that rose, and a volume that will be full within 30 days at the
 * rate of the last 30. `usage` is the 30 days of `fs.used` (null when it
 * could not be read). `covers` names the checks of the machine's own that
 * say the same, so that one fault is listed once.
 */
export function needsOf(answer, usage) {
  const disks = listOf(answer, 'disks');
  const needs = [];
  for (const volume of listOf(answer, 'volumes')) {
    if (!(volume.missing > 0)) continue;
    const mirror = isMirror(volume);
    needs.push({
      id: `storage-missing-${volume.uuid}`,
      severity: 'bad',
      title: `${capital(mirrorName(volume))} has lost a disk`,
      detail: mirror ? 'Every file is still there, with no second copy. Replace the missing disk.' : 'Some files may be gone. Find the missing disk and reconnect or replace it.',
      href: '/storage',
      icon: 'storage',
      at: null,
      covers: ['MET-103'],
    });
  }
  for (const disk of disks) {
    const health = disk.health;
    if (health?.verdict === 'failing') {
      const mirror = volumesOfDisk(disk, answer).some((volume) => isMirror(volume) && !(volume.missing > 0));
      needs.push({
        id: `storage-failing-${disk.serial || disk.name}`,
        severity: 'bad',
        title: `${disk.name} says it is failing`,
        detail: mirror ? 'Copy what matters off it now, or replace it while the mirror still has a full copy.' : 'Copy what matters off it now: it may stop working at any time.',
        href: diskHref(disk.name),
        icon: 'storage',
        at: Number.isFinite(health.checked_ms) ? Math.floor(health.checked_ms / 1000) : null,
        covers: ['HW-111'],
      });
    } else if (health?.rose) {
      needs.push({
        id: `storage-rising-${disk.serial || disk.name}`,
        severity: 'warn',
        title: `${disk.name} is wearing${health.moved > 0 ? `: ${counted(health.moved, 'sector')} moved to spares` : ''}`,
        detail: `${health.rose.title}. Keep backups current and have a replacement ready.`,
        href: diskHref(disk.name),
        icon: 'storage',
        at: Math.floor(health.rose.at_ms / 1000),
        covers: ['HW-115'],
      });
    }
  }
  for (const volume of listOf(answer, 'volumes')) {
    const projection = projectionOf(volume, disks, usage);
    if (projection?.seconds == null || projection.seconds >= FULL_SOON) continue;
    const nearly = volume.size > 0 && volume.used / volume.size >= 0.9;
    needs.push({
      id: `storage-full-${volume.uuid}`,
      severity: 'warn',
      title: `${volumeTitle(volume)} is full ${projection.seconds === 0 ? 'now' : `in ${roughly(projection.seconds)}`}`,
      detail: `${bytesWords(volume.used)} of ${bytesWords(volume.size)} is used. At the rate of the last ${projection.days} days, it is full ${projection.seconds === 0 ? 'now' : `in ${roughly(projection.seconds)}`}. Free some space or add a disk.`,
      href: '/storage',
      icon: 'storage',
      at: null,
      covers: nearly ? ['MET-101', 'MET-102'] : ['MET-102'],
    });
  }
  return needs;
}
