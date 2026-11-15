// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What one disk's panel says (design 4.4 and 4.5), as plain values: its rows
// of facts, how old it is, when it was last checked in full, the scrub
// button and the room to grow into, and the steps to use or replace a disk.
// It is loaded when a disk is opened. `answer` is `GET /api/1/storage`.

import { disksOf, healthWords, isMirror, layoutWords, usedFor, volumeName, volumeOf } from '../../lib/disk-model.js';
import { bytesWords, counted, roughly } from '../../lib/size-words.js';
import { capital, findDisk, listOf, seenWords, volumesOfDisk } from './model.js';

/** "2 years 6 months powered on (21,804 hours)": how long a disk has been on, from its hours. */
export function ageWords(hours) {
  if (!Number.isFinite(hours) || hours < 0) return null;
  const months = Math.round(hours / 730.5);
  const years = Math.floor(months / 12);
  const rest = months % 12;
  let age;
  if (years > 0) age = rest > 0 ? `${counted(years, 'year')} ${counted(rest, 'month')}` : counted(years, 'year');
  else if (months > 0) age = counted(months, 'month');
  else if (hours >= 24) age = counted(Math.floor(hours / 24), 'day');
  else age = counted(Math.floor(hours), 'hour');
  return `${age} powered on (${new Intl.NumberFormat('en-US').format(Math.round(hours))} hours)`;
}

/** The day of `seconds` in the person's zone: "Nov 1", and with the year when it is not this one. */
export function dayWords(seconds, time, now) {
  const zone = time.zone;
  const year = (at) => new Intl.DateTimeFormat('en-US', { timeZone: zone, year: 'numeric' }).format(at * 1000);
  const day = new Intl.DateTimeFormat('en-US', { timeZone: zone, month: 'short', day: 'numeric' }).format(seconds * 1000);
  return year(seconds) === year(now) ? day : `${day}, ${year(seconds)}`;
}

/** The words for a disk's last scrub, from its volume's kept result: how many read errors it fixed, and when. */
function scrubRow(volumes, time, now) {
  if (volumes.length === 0) return null;
  const kept = volumes.map((volume) => volume.scrub).filter(Boolean).sort((a, b) => b.at_ms - a.at_ms)[0];
  if (!kept) return { label: 'Read errors fixed', value: 'No scrub has run yet' };
  const day = dayWords(Math.floor(kept.at_ms / 1000), time, now);
  if (kept.result === 'stopped') return { label: 'Read errors fixed', value: `The last scrub, ${day}, was stopped before its end` };
  if (kept.unfixable > 0 || kept.result === 'errors') {
    return { label: 'Read errors fixed', value: `${kept.fixed} of ${kept.errors} in the last scrub, ${day}; ${kept.unfixable} could not be mended`, tone: 'bad' };
  }
  return { label: 'Read errors fixed', value: `${kept.fixed} in the last scrub, ${day}` };
}

/**
 * The disk's last extended self-test from `selfTests` (diskhealth's log,
 * newest first), as words: "Passed, Oct 30". `hours` are the hours the disk
 * has been on now, which give the day of a test from the hours it logged.
 */
export function lastFullCheck(selfTests, hours, time, now) {
  const test = (Array.isArray(selfTests) ? selfTests : []).find((one) => one.kind === 'extended');
  if (!test) return { value: 'No full check is logged on this disk' };
  const result = capital(String(test.result));
  if (test.result === 'running') return { value: 'Running now' };
  if (!Number.isFinite(test.at_hours) || !Number.isFinite(hours)) return { value: result };
  return { value: `${result}, ${dayWords(now - Math.max(0, hours - test.at_hours) * 3600, time, now)}`, tone: test.result === 'failed' ? 'bad' : undefined };
}

/** The rows of the counts a disk keeps, as a SATA disk or an NVMe drive gives them. */
function countRows(health) {
  const rows = [];
  if (health.moved != null) rows.push({ label: 'Moved sectors', value: String(health.moved), tone: health.moved > 0 ? 'warn' : undefined });
  if (health.waiting != null) rows.push({ label: 'Waiting sectors', value: String(health.waiting), tone: health.waiting > 0 ? 'warn' : undefined });
  if (health.offline != null && health.offline > 0) rows.push({ label: 'Unreadable sectors', value: String(health.offline), tone: 'warn' });
  if (health.media_errors != null) rows.push({ label: 'Media errors', value: String(health.media_errors), tone: health.media_errors > 0 ? 'warn' : undefined });
  if (health.used_percent != null) rows.push({ label: 'Wear', value: `Used ${Math.round(health.used_percent)} percent of its rated writes`, tone: health.used_percent >= 90 ? 'warn' : undefined });
  return rows;
}

/** The scrub button of a volume: its words, and how long a scrub takes, from what the last one read. */
export function scrubOf(volume) {
  if (!volume) return null;
  const mirror = isMirror(volume);
  const kept = volume.scrub;
  const rate = kept && kept.seconds > 0 && kept.bytes > 0 ? kept.bytes / kept.seconds : null;
  const length = rate ? `: ${roughly(volume.used / rate)}` : '';
  return {
    mount: volume.mount,
    label: mirror ? 'Scrub the mirror' : 'Check every block',
    hint: `This reads every block of ${bytesWords(volume.used)}${length}. The machine stays usable, a little slower.`,
  };
}

/** The room after a disk's last partition that it can grow into, as `{words, bytes}`, or null when there is none worth saying. */
export function growOf(disk, answer) {
  const last = [...(disk.partitions ?? [])].sort((a, b) => b.start - a.start)[0];
  if (!last || last.filesystem !== 'btrfs' || !(disk.size > 0) || !(disk.free_after / disk.size > 0.01)) return null;
  const volume = volumeOf(last.name, listOf(answer, 'volumes'));
  const into = volume ? `${volumeName(volume).replace(/ volume$/, '')} volume` : 'the filesystem';
  return { bytes: disk.free_after, words: `This disk has ${bytesWords(disk.free_after)} after its last partition, which ${into} can grow into.` };
}

/**
 * What the panel of `disk` says: `{name, line, health, hasHealth, mirror, rows, scrub, grow}`.
 * `checked` is the answer of `storage.health` for it when it has been read
 * (the self-tests are in it); `admin` whether the person is an
 * administrator, who reads them. `time` is lib/time.js's, `now` Unix
 * seconds.
 */
export function detailOf(disk, answer, { time, now, checked = null, admin = false }) {
  const volumes = volumesOfDisk(disk, answer);
  const health = disk.health ?? null;
  const usedWords = [];
  for (const partition of disk.partitions ?? []) {
    const volume = volumeOf(partition.name, volumes);
    const words = volume ? `${usedFor(partition, volumes)} (${layoutWords(volume)})` : usedFor(partition, volumes);
    if (!usedWords.includes(words)) usedWords.push(words);
  }
  const rows = [
    { label: 'Model', value: [disk.model || 'Unknown model', disk.serial ? `serial ${disk.serial}` : null].filter(Boolean).join(' · ') },
    { label: 'Used for', items: usedWords.length > 0 ? usedWords : ['Unused'] },
  ];
  if (health) {
    const age = ageWords(health.hours);
    if (age) rows.push({ label: 'Age', value: age });
    if (Number.isFinite(health.temperature_c)) rows.push({ label: 'Temperature', value: `${health.temperature_c} °C` });
    rows.push(...countRows(health));
    if (health.rose) rows.push({ label: 'Early warning', value: `${health.rose.title}. ${seenWords(health.rose, time)}`, tone: 'warn' });
  } else {
    rows.push({ label: 'Health', value: 'Not checked: this disk does not report its health' });
  }
  const scrub = scrubRow(volumes, time, now);
  if (scrub) rows.push(scrub);
  if (health) {
    if (!admin) rows.push({ label: 'Last full check', value: 'Administrators see the disk\'s own tests', hardware: true });
    else if (checked) rows.push({ label: 'Last full check', ...lastFullCheck(checked.self_tests, checked.hours ?? health.hours, time, now), hardware: true });
    else rows.push({ label: 'Last full check', value: 'Press Check now to read it', hardware: true });
  }
  const volume = volumes.find((one) => one.missing === 0) ?? volumes[0] ?? null;
  return {
    name: disk.name,
    line: `${disk.model || 'Disk'} · ${bytesWords(disk.size)}`,
    health: healthWords(health),
    hasHealth: health !== null,
    mirror: volumes.some((one) => isMirror(one)),
    rows,
    scrub: scrubOf(volume),
    grow: growOf(disk, answer),
  };
}

/** The answer of `storage.health` for the disk `name`, which diskhealth prints as a list of one. */
export function healthOf(answer, name) {
  if (Array.isArray(answer)) return answer.find((one) => one?.disk === name) ?? answer[0] ?? null;
  return answer && typeof answer === 'object' ? answer : null;
}

/** The numbers of a SATA disk's attributes as table rows: `[name, value, worst, threshold, raw, note]`, the note saying that a falling value means failure. */
export function attributeRows(checked) {
  const list = Array.isArray(checked?.attributes) ? checked.attributes : [];
  return list.map((one) => [`${one.id} ${one.name}`, String(one.value), String(one.worst), String(one.threshold), String(one.raw), one.prefail ? 'Tells of failure' : 'Tells of age']);
}

// ---- the steps to use or replace a disk -----------------------------------------------

/**
 * What the steps to replace a mirror's disk are about, `{mount, survivor}`:
 * the volume (the mirror of the disk `diskName` when one is open, else the
 * one that lost a disk, else any mirror, else a volume that is not the
 * system) and the name of a disk of it that is not `diskName`, which is the
 * one that is left. `mount` is `/volumes/data` when no volume is known.
 */
export function stepsOf(answer, diskName) {
  const volumes = listOf(answer, 'volumes');
  const own = diskName && findDisk(answer, diskName) ? volumesOfDisk(findDisk(answer, diskName), answer) : [];
  const volume = own.find(isMirror) ?? volumes.find((one) => one.missing > 0) ?? volumes.find(isMirror) ?? own[0]
    ?? volumes.find((one) => one.mount !== '/') ?? null;
  const names = volume ? disksOf(volume, listOf(answer, 'disks')) : [];
  return { mount: volume?.mount ?? '/volumes/data', survivor: names.find((name) => name !== diskName) ?? null };
}
