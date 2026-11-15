// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Snapshots and backups page says (design 5), as plain values, so
// that a test reaches every branch: which view an address asks for, the
// banners, the rows of What is protected, the backup places, the tasks that
// back up, check or restore, and the disks a backup can go to. What one
// opened place holds is detail-model.js's, what the forms may send
// form-model.js's. The files of this folder only draw what these give.
// `list` is `GET /api/1/snapshots` (`snap list --json`
// without a place), `backups` is `GET /api/1/backups` (`backup list
// --json`) and `storage` is `GET /api/1/storage`; any of them may be
// missing or older than this file, and each function does without what its
// answer leaves out.

import { counted } from '../../lib/size-words.js';
import { inOrder, placeName, placeShort } from '../../lib/snapshot-model.js';
import { dayWords } from '../../lib/snapshot-words.js';

/** The list `name` of `answer`, or an empty one when the answer has none. */
export const listOf = (answer, name) => (Array.isArray(answer?.[name]) ? answer[name] : []);

/** `words` as a sentence: admind's and backup's words carry no full stop of their own. */
export const sentence = (words) => (/[.!?]$/.test(words) ? words : `${words}.`);

/** A time of `backup list --json`: milliseconds, or null for 0 and for what is not one. */
export const msOf = (value) => (Number.isFinite(value) && value > 0 ? value : null);

/** The words of a state's text, or empty text. */
const textOf = (value) => (typeof value === 'string' ? value.trim() : '');

// ---- the address ------------------------------------------------------------

/**
 * What an address under `/snapshots` asks for, from `rest`, the part after
 * it: `{browse, backups, open}`. `browse` is a snapshot's folder; `backups`
 * the page scrolled to the backup places, with the place `open` opened.
 */
export function routeOf(rest) {
  const parts = String(rest ?? '').split('/').filter(Boolean).map((part) => {
    try {
      return decodeURIComponent(part);
    } catch {
      return part;
    }
  });
  if (parts[0] === 'browse') return { browse: true, backups: false, open: null };
  if (parts[0] === 'backups') return { browse: false, backups: true, open: parts[1] ?? null };
  return { browse: false, backups: false, open: null };
}

/** The address of the backup place `name`. */
export const placeHref = (name) => `/snapshots/backups/${encodeURIComponent(name)}`;

// ---- what backup list says of a place -----------------------------------------

/** The backup places of `backups`. */
export const backupPlaces = (backups) => listOf(backups, 'places').filter((place) => typeof place?.name === 'string');

/** A place's state as plain values: `{tried, done, failed, verified, verifyFailed}`, times in milliseconds or null, texts or empty. */
export function stateOf(place) {
  const state = place?.state ?? {};
  return {
    tried: msOf(state.tried_ms),
    done: msOf(state.done_ms),
    failed: textOf(state.failed),
    verified: msOf(state.verified_ms),
    verifyFailed: textOf(state.verify_failed),
  };
}

/** The days that backup(8) lets pass without a backup, by how often it is made: a place is bad from twice as many. */
const ALLOWED_DAYS = Object.freeze({ day: 3, week: 14 });

/** Whether `place` is overdue, as `backup list` counts it (`overdue_days`, null when it is not): `{days, tone}`, `tone` bad from twice the allowed days, or null. */
export function lateOf(place) {
  const days = place?.overdue_days;
  if (!Number.isInteger(days) || days <= 0) return null;
  const allowed = ALLOWED_DAYS[place.every];
  return { days, tone: allowed && days >= 2 * allowed ? 'bad' : 'warn' };
}

/** A time as a sentence has it: "Nov 10 at 2:04 AM". `time` is lib/time.js's, `now` milliseconds. */
export const atWords = (ms, { time, now }) => `${dayWords(ms, time.zone, now)} at ${time.clock(ms / 1000)}`;

/** The newest snapshot of any place that is made, in milliseconds, or null. */
function newestSnapshot(list) {
  const times = listOf(list, 'places').filter((place) => place.in_service !== false).map((place) => Date.parse(place.newest_made)).filter(Number.isFinite);
  return times.length === 0 ? null : Math.max(...times);
}

/** How old the newest snapshot may be and still be current: one daily round and a day's grace. */
const CURRENT_MS = 2 * 86_400_000;

/**
 * Whether the snapshots on this machine are current, which is what a
 * failed backup leaves between a person and a lost file: true, false, or
 * null when `list` was not read.
 */
export function snapshotsCurrent(list, now) {
  if (list == null) return null;
  const newest = newestSnapshot(list);
  return newest !== null && now - newest <= CURRENT_MS;
}

/** What still protects the files while a backup is behind, as banner 5.3 says it. */
const PROTECTS = Object.freeze({
  current: 'Snapshots on this server are current, so files changed since then can still be brought back, but nothing is outside this machine.',
  stale: 'Snapshots on this server are not current either, so nothing recent is kept anywhere else.',
});

// ---- the banner ---------------------------------------------------------------

/** The most bad a banner can be: sorting puts it first. */
const WORSE = Object.freeze({ bad: 0, warn: 1, info: 2 });

/**
 * The banners, worst first, each `{id, tone, lead, detail, buttons}`: a
 * backup that failed, one that is overdue, a check by a restore that failed,
 * and a whole-machine restore that waits for the next start. A button is
 * `{id, label, ...}`: `run` backs up to its `name` now, `disk` opens Add a
 * backup place with a plugged-in disk chosen, `move` opens the page of a
 * whole-machine restore. `disks` says whether a mounted removable volume
 * could take a backup instead; `current` is `snapshotsCurrent`'s answer.
 */
export function bannersOf(backups, { time, now, current = null, disks = false }) {
  const found = [];
  for (const place of backupPlaces(backups)) {
    const { name } = place;
    const state = stateOf(place);
    const late = lateOf(place);
    const lastGood = state.done === null ? 'Nothing has been backed up there yet.' : `The last good one was ${atWords(state.done, { time, now })}.`;
    const protects = current === null ? '' : PROTECTS[current ? 'current' : 'stale'];
    const buttons = [{ id: 'run', label: 'Back up now', name }, ...(disks ? [{ id: 'disk', label: 'Back up to a plugged-in disk instead…' }] : [])];
    if (state.failed) {
      found.push({ id: `failed:${name}`, tone: 'bad', lead: sentence(`The last backup to ${name} failed: ${state.failed}`), detail: [lastGood, protects].filter(Boolean).join(' '), buttons });
    } else if (late) {
      found.push({ id: `overdue:${name}`, tone: late.tone, lead: `Backups to ${name} are ${counted(late.days, 'day')} overdue.`, detail: [lastGood, protects].filter(Boolean).join(' '), buttons });
    }
    if (state.verifyFailed) {
      found.push({
        id: `check:${name}`,
        tone: 'warn',
        lead: sentence(`Restoring ${name}'s newest backups to check them failed: ${state.verifyFailed}`),
        detail: 'The backups may still be good: a check can fail for a reason of its own, such as a full disk. Check again to see.',
        buttons: [{ id: 'verify', label: 'Check now', name }],
      });
    }
  }
  const staged = backups?.staged;
  if (staged && typeof staged.from === 'string') {
    found.push({
      id: 'staged',
      tone: 'warn',
      lead: `A whole-machine restore from ${staged.from} is staged for the next start.`,
      detail: 'Cancel it on Move to new hardware.',
      buttons: [{ id: 'move', label: 'Open Move to new hardware', href: '/move' }],
    });
  }
  return found.map((banner, at) => ({ banner, at })).sort((a, b) => WORSE[a.banner.tone] - WORSE[b.banner.tone] || a.at - b.at).map((one) => one.banner);
}

// ---- What is protected --------------------------------------------------------------

/** How many snapshots a place of `snap list` has: a count, or the length of a list. */
const countOf = (value) => (Array.isArray(value) ? value.length : Number.isFinite(value) ? value : null);

/** The rule's word for a snapshot's row: "Daily", "Weekly", or "Newest" when the machine keeps neither. */
function ruleWord(settings) {
  if (settings?.daily > 0) return 'Daily';
  if (settings?.weekly > 0) return 'Weekly';
  return 'Newest';
}

/** The places of `list` that snap knows, `[{place, newest, count, inService}]`. */
export function snapPlaces(list) {
  return listOf(list, 'places').filter((place) => typeof place?.place === 'string').map((place) => ({
    place: place.place,
    newest: Number.isFinite(Date.parse(place.newest_made)) ? Date.parse(place.newest_made) : null,
    count: countOf(place.snapshots),
    inService: place.in_service !== false,
  }));
}

/** The places snap.toml names: from `settings` when the machine says them, else the places `snap list` gives. */
export const scheduledPlaces = (list) => (Array.isArray(list?.settings?.places) ? list.settings.places : snapPlaces(list).map((place) => place.place));

/** Whether a snapshot is made of `path` by the schedule: named by snap.toml and in the service's view. */
export function isCovered(list, path) {
  const known = snapPlaces(list).find((place) => place.place === path);
  return scheduledPlaces(list).includes(path) && known?.inService !== false;
}

/** What the Snapshots column says of `path`: `{words, tone}`, `tone` ok, warn or empty (a place that waits for its first). */
function snapshotsCell(list, path, { time }) {
  const known = snapPlaces(list).find((place) => place.place === path);
  if (known && !known.inService) return { words: 'Not made', tone: 'warn' };
  if (!scheduledPlaces(list).includes(path)) return { words: 'None', tone: 'warn' };
  if (!(known?.count > 0) || known.newest === null) return { words: 'None yet: the first is made within the hour', tone: '' };
  return { words: `${ruleWord(list?.settings)} · ${time.when(known.newest / 1000)}`, tone: 'ok' };
}

/** What a backup place's state says of its copy of a path: `{name, rest, tone}`, `rest` what follows the place's name, "Nov 10, 3 days overdue". */
function copyOf(place, { time, now }) {
  const state = stateOf(place);
  const late = lateOf(place);
  const day = state.done === null ? 'not yet' : dayWords(state.done, time.zone, now);
  if (state.failed) return { name: place.name, rest: `${day}, failed`, tone: 'bad' };
  if (late) return { name: place.name, rest: `${day}, ${counted(late.days, 'day')} overdue`, tone: late.tone };
  if (state.done === null) return { name: place.name, rest: 'not yet', tone: place.every === 'hand' ? '' : 'warn' };
  return { name: place.name, rest: day, tone: 'ok' };
}

/** What the last restore check of the places that hold a path says: `{words, tone, name}`; `name` is the place that Check now would check. */
function checkCell(holding, { time, now }) {
  if (holding.length === 0) return { words: 'No copy to check', tone: '', name: null };
  const failed = holding.find((place) => stateOf(place).verifyFailed);
  if (failed) return { words: 'failed', tone: 'warn', name: failed.name };
  const checked = holding.map((place) => ({ place, at: stateOf(place).verified })).filter((one) => one.at !== null).sort((a, b) => b.at - a.at);
  if (checked.length > 0) return { words: `${dayWords(checked[0].at, time.zone, now)}, passed`, tone: 'ok', name: checked[0].place.name };
  return { words: 'Never', tone: '', name: holding[0].name };
}

/**
 * The rows of What is protected: the places snap.toml names and every
 * place a backup place holds, once each, in the order the page lists them.
 * A row is `{path, name, short, snapshots, copies, check}`: `snapshots`
 * and `check` are `{words, tone}`, `check.name` the backup place that Check
 * now would check, and `copies` each backup place that holds the path
 * (`{name, rest, tone}`), none when it is not copied off this server.
 */
export function protectedRows(list, backups, { time, now }) {
  const places = backupPlaces(backups);
  const paths = inOrder([...scheduledPlaces(list), ...places.flatMap((place) => listOf(place, 'places').filter((path) => typeof path === 'string'))]);
  return paths.map((path) => {
    const holding = places.filter((place) => listOf(place, 'places').includes(path));
    return {
      path,
      name: placeName(path),
      short: placeShort(path),
      snapshots: snapshotsCell(list, path, { time }),
      copies: holding.map((place) => copyOf(place, { time, now })),
      check: checkCell(holding, { time, now }),
    };
  });
}

/** The places snap names that the snap service does not see, which it skips: `[path]`. */
export const skippedPlaces = (list) => snapPlaces(list).filter((place) => !place.inService).map((place) => place.place);

// ---- the backup places -----------------------------------------------------------------

/** The host of an address that `backup list` gives: an SSH one, as `ssh://user@host:port/path` or `user@host:/path`. */
export function hostOf(to) {
  const found = /^(?:ssh:\/\/)?(?:[^@/\s]+@)?(\[[^\]]+\]|[^:/\s]+)/.exec(to);
  return found ? found[1].replace(/^\[|\]$/g, '') : to;
}

/** The label of the volume a folder under `/volumes` is on. */
export const volumeOf = (to) => /^\/volumes\/([^/]+)/.exec(to)?.[1] ?? null;

/** Where a backup place keeps its backups, in words: "SSH to 192.168.1.20 · encrypted", "USB disk BACKUP · encrypted". */
export function whereWords(place) {
  const to = textOf(place.to);
  let where;
  if (place.kind === 'ssh') {
    where = `SSH to ${hostOf(to)}`;
  } else if (place.kind === 's3') {
    const bucket = /^s3(?:\+http)?:\/\/[^/]+\/([^/]+)/.exec(to)?.[1];
    where = bucket ? `S3 bucket ${bucket}` : 'S3 storage';
  } else if (place.plugged_in) {
    const label = typeof place.plugged_in === 'string' ? place.plugged_in : place.plugged_in?.label ?? volumeOf(to);
    where = label ? `USB disk ${label}` : 'USB disk';
  } else {
    where = `Folder ${to}`;
  }
  return `${where} · encrypted`;
}

/** How often a place is backed up, as the Every column says it. */
export const everyWords = (place) => (place.every === 'hand' ? 'by hand' : place.every ?? '');

/**
 * One row of the table of backup places, `{name, href, where, every, last,
 * now, job}`: `last` the day of the last good backup, `now` `{words, tone}`
 * ("3 days overdue", "OK"), and `job` the kind of task that runs for it
 * (`run`, `verify`, `restore`), or `run` when only the round's lock says so.
 */
export function placeRows(backups, { time, now, jobs = null }) {
  return backupPlaces(backups).map((place) => {
    const state = stateOf(place);
    const late = lateOf(place);
    const job = jobs?.of(place.name)?.kind ?? (place.running ? 'run' : null);
    let words = { words: 'OK', tone: 'ok' };
    if (state.failed) words = { words: 'failed', tone: 'bad' };
    else if (late) words = { words: `${counted(late.days, 'day')} overdue`, tone: late.tone };
    else if (state.verifyFailed) words = { words: 'check failed', tone: 'warn' };
    else if (state.done === null) words = { words: place.every === 'hand' ? 'Not yet' : 'Waiting for the first backup', tone: '' };
    return {
      name: place.name,
      href: placeHref(place.name),
      where: whereWords(place),
      every: everyWords(place),
      last: state.done === null ? 'Never' : dayWords(state.done, time.zone, now),
      now: words,
      job,
    };
  });
}

/** The place of `backups` called `name`, or null. */
export const findPlace = (backups, name) => backupPlaces(backups).find((place) => place.name === name) ?? null;

// ---- the disks a backup can go to ----------------------------------------------------------------------

/** The folder on a plugged-in disk that backups go to, which `backup add` makes in the volume it is given. */
export const DISK_FOLDER = 'dukovo-backup';

/**
 * The mounted removable volumes of `storage` that can take a backup place:
 * `[{mount, label, disk, free, size, to}]`, where `to` is the volume itself,
 * as `backup add` takes it, which puts the backups in DISK_FOLDER there. A volume that a backup place
 * already uses is left out, since a second place there would hold the same
 * backups twice.
 */
export function removableVolumes(storage, backups = null) {
  const used = backupPlaces(backups).map((place) => textOf(place.to));
  const found = [];
  for (const disk of listOf(storage, 'disks')) {
    if (!disk.removable) continue;
    for (const part of listOf(disk, 'partitions')) {
      const mount = listOf(part, 'mounts').find((path) => path.startsWith('/volumes/'));
      if (!mount || used.some((to) => to === mount || to.startsWith(`${mount}/`))) continue;
      const volume = listOf(storage, 'volumes').find((one) => listOf(one, 'devices').includes(part.name));
      const filesystem = listOf(storage, 'filesystems').find((one) => one.device === part.name);
      const free = volume?.free ?? filesystem?.free ?? null;
      found.push({ mount, label: part.label || volumeOf(mount) || part.name, disk: disk.name, free: Number.isFinite(free) ? free : null, size: part.size ?? null, to: mount });
    }
  }
  return found;
}

// ---- tasks that back up, check and restore --------------------------------------------------------

/** The actions that run as tasks on a backup place, and the kind of each. */
export const JOB_ACTIONS = Object.freeze({ 'backups.run': 'run', 'backups.verify': 'verify', 'backups.restore': 'restore' });

const running = (task) => task.state === 'waiting' || task.state === 'running';
const percentOf = (task) => (Number.isInteger(task.percent) ? Math.min(100, Math.max(0, task.percent)) : null);
const stepOf = (task) => (Number.isInteger(task.step) && Number.isInteger(task.steps) && task.steps > 1 ? { step: task.step, steps: task.steps } : null);

/** What the page keeps of a task: `{id, kind, name, args, percent, step, mine}`; `mine` says that this page began it. */
const entryOf = (task) => ({
  id: task.id, kind: JOB_ACTIONS[task.action], name: task.args?.name ?? null, args: task.args ?? null, percent: percentOf(task), step: stepOf(task), mine: false,
});

/**
 * What the page knows of the backups, checks and restores that run: `learn(tasks)`
 * takes those of `GET /api/1/tasks`; `started(id, action, args)` one this
 * page began, with its arguments; `event(data)` one `event: task` and gives `{ended: job,
 * state}` for one that ended, `{learn: id}` for one it does not know yet
 * (the event holds no arguments, so it is read whole), else `{}`; `put(task)`
 * takes one read whole. `of(name)` is the job of a place, `all()` every job.
 */
export function makeJobs() {
  const byId = new Map();
  const put = (task) => {
    if (JOB_ACTIONS[task.action] && running(task)) byId.set(task.id, { ...entryOf(task), mine: byId.get(task.id)?.mine === true });
  };
  return {
    learn(tasks) {
      const mine = new Set([...byId.values()].filter((job) => job.mine).map((job) => job.id));
      byId.clear();
      for (const task of Array.isArray(tasks) ? tasks : []) put(task);
      for (const job of byId.values()) job.mine = mine.has(job.id);
    },
    started(id, action, args) {
      byId.set(id, { id, kind: JOB_ACTIONS[action], name: args?.name ?? null, args, percent: null, step: null, mine: true });
    },
    event(data) {
      const before = byId.get(data?.id);
      if (data?.action !== undefined && !JOB_ACTIONS[data.action]) return {};
      if (!before) return JOB_ACTIONS[data?.action] && running(data) ? { learn: data.id } : {};
      if (!running(data)) {
        byId.delete(data.id);
        return { ended: before, state: data.state };
      }
      byId.set(data.id, { ...before, percent: percentOf(data) ?? before.percent, step: stepOf(data) ?? before.step });
      return {};
    },
    put,
    of: (name) => [...byId.values()].find((job) => job.name === name) ?? null,
    all: () => [...byId.values()],
  };
}

/** The first words of a job's button and of the Now column. */
const JOB_WORDS = Object.freeze({ run: 'Backing up…', verify: 'Checking…', restore: 'Restoring…' });

/** What a job says of how far it is: "Backing up… 41%", or "Checking… step 2 of 3". */
export function jobWords(job) {
  const first = JOB_WORDS[job.kind] ?? 'Working…';
  if (job.percent !== null) return `${first} ${job.percent}%`;
  return job.step ? `${first} step ${job.step.step} of ${job.step.steps}` : first;
}
