// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Hardware page says, as plain values: what this machine is, the
// last check and those before it, the memory test's last run and the power
// card's words. The answers are `hardware show`, `last` and `history` as
// they print; a member the machine does not say is null, and is left out.

import { endWords } from '../../lib/tasks-words.js';

const GIB = 2 ** 30;
const MIB = 2 ** 20;

const counted = (count, noun) => `${count} ${noun}${count === 1 ? '' : 's'}`;
const orNothing = (text) => (typeof text === 'string' && text.trim() !== '' ? text.trim() : null);

/** "15.5 GiB" or "512 MiB"; null for no memory. */
export function amountWords(bytes) {
  if (!Number.isFinite(bytes) || bytes <= 0) return null;
  return bytes >= GIB ? `${Number((bytes / GIB).toFixed(1))} GiB` : `${Math.round(bytes / MIB)} MiB`;
}

// ---- This machine -----------------------------------------------------------

/** "Dell Inc. OptiPlex 7040, mini tower"; null when the firmware names no maker or model. */
export function modelWords(machine) {
  const named = [orNothing(machine?.maker), orNothing(machine?.model)].filter(Boolean).join(' ');
  if (named === '') return null;
  const chassis = orNothing(machine.chassis)?.toLowerCase();
  return chassis ? `${named}, ${chassis}` : named;
}

const clockWords = (mhz) => `${Number((mhz / 1000).toFixed(2))} GHz`;

/** "Intel Core i5-6500 · 4 cores, 3.2 GHz, up to 3.6 GHz"; threads only when a core runs more than one. */
export function processorWords(processor) {
  const {
    model, cores, threads, base_mhz: base, max_mhz: max,
  } = processor ?? {};
  const details = [];
  if (cores > 0) details.push(counted(cores, 'core'));
  if (threads > 0 && threads !== cores) details.push(counted(threads, 'thread'));
  if (base > 0 && max > base) details.push(clockWords(base), `up to ${clockWords(max)}`);
  else if (base > 0 || max > 0) details.push(clockWords(base > 0 ? base : max));
  return [orNothing(model), details.join(', ')].filter(Boolean).join(' · ') || null;
}

/** "DDR4-2133": a module's kind and the speed it runs at. */
const kindWords = (module) => `${module.kind}${module.speed > 0 ? `-${module.speed}` : ''}`;

/** "16 GiB · in 2 of 4 slots · DDR4-2133 · no error correction"; `access` is `ok` when SMBIOS's tables were read, `locked` for a locked console and `reader` for anyone else. */
export function memoryWords(memory, access) {
  const parts = [amountWords(memory?.bytes)];
  if (access === 'reader') parts.push('An administrator sees the slots');
  if (access === 'ok') {
    if (!Array.isArray(memory?.modules)) {
      parts.push('The firmware does not list the slots');
    } else {
      const kinds = [...new Set(memory.modules.map(kindWords))];
      if (Number.isInteger(memory.slots)) parts.push(`in ${memory.modules.length} of ${memory.slots} slots`);
      if (kinds.length > 0) parts.push(kinds.join(' and '));
      if (typeof memory.ecc === 'boolean') parts.push(memory.ecc ? 'error correction' : 'no error correction');
    }
  }
  return parts.filter(Boolean).join(' · ') || null;
}

/** "DIMM A1 · 8 GiB · DDR4-2133 · Example Memory EM8G2133". */
export function moduleWords(module) {
  const made = [orNothing(module.maker), orNothing(module.part)].filter(Boolean).join(' ');
  return [orNothing(module.slot), amountWords(module.bytes), kindWords(module), made].filter(Boolean).join(' · ');
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** A firmware's `07/12/2023` as "Jul 12, 2023"; a date in another form stays as it is. */
export function dateWords(date) {
  const found = /^(\d{2})\/(\d{2})\/(\d{4})$/.exec(date ?? '');
  const month = found && MONTHS[Number(found[1]) - 1];
  return month ? `${month} ${Number(found[2])}, ${found[3]}` : orNothing(date);
}

/** "BIOS 1.24.0, Jul 12, 2023"; null when the firmware gives no version. */
export function firmwareWords(firmware) {
  const version = orNothing(firmware?.version);
  if (version === null) return null;
  const date = dateWords(firmware.date);
  return `BIOS ${version}${date ? `, ${date}` : ''}`;
}

/** How long the machine has run, in two units at most: "4 hours 52 minutes", "3 days 4 hours". */
export function upWords(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) return null;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 1) return 'less than a minute';
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const parts = days > 0 ? [[days, 'day'], [hours, 'hour']] : [[hours, 'hour'], [minutes % 60, 'minute']];
  return parts.filter(([count]) => count > 0).map(([count, noun]) => counted(count, noun)).join(' ');
}

/** "since the update was taken back (3:06 AM)" when the start was a fallback; `about` is `GET /api/1/console/about`, `now` is in Unix seconds. */
export function tookBackWords(about, time, now) {
  if (about?.fallback !== true || !Number.isFinite(about.up)) return null;
  return `since the update was taken back (${time.when(now - about.up)})`;
}

// ---- The last check ---------------------------------------------------------

/** What a result is called and how it is drawn, in the order the page lists them. */
const RESULTS = Object.freeze([
  { result: 'failed', word: 'Failed', tone: 'bad' },
  { result: 'warning', word: 'Warning', tone: 'warn' },
  { result: 'passed', word: 'Passed', tone: 'ok' },
  { result: 'skipped', word: 'Skipped', tone: 'off' },
]);

const checksOf = (last) => (Array.isArray(last?.checks) ? last.checks : []);
const withResult = (last, result) => checksOf(last).filter((one) => one.result === result);

/** " (HW-115, HW-117)" for the IDs of `checks`, each once; nothing for none. */
function idsOf(checks) {
  const found = [...new Set(checks.map((one) => one.id).filter(Boolean))];
  return found.length > 0 ? ` (${found.join(', ')})` : '';
}

/** Whether `last`, the answer of `hardware.last`, is a result: it is null when no check was kept. */
export const hasResult = (last) => last !== null && typeof last === 'object' && Number.isFinite(last.time);

/** "Last check Nov 9 2:10 AM (a full check): 8 passed, 1 warning (HW-115), 2 skipped."; `time` is lib/time.js's. */
export function summaryWords(last, time) {
  const [failed, warnings, passed, skipped] = ['failed', 'warning', 'passed', 'skipped'].map((result) => withResult(last, result));
  const parts = [
    passed.length > 0 && `${passed.length} passed`,
    warnings.length > 0 && `${counted(warnings.length, 'warning')}${idsOf(warnings)}`,
    failed.length > 0 && `${failed.length} failed${idsOf(failed)}`,
    skipped.length > 0 && `${skipped.length} skipped`,
  ].filter(Boolean);
  return `Last check ${time.when(last.time)} (${last.full ? 'a full check' : 'a quick check'}): ${parts.join(', ') || 'nothing could be checked'}.`;
}

/** The pill of the Last check card, `{word, tone}`, for the worst result. */
export function stateOf(last) {
  const worst = RESULTS.find((one) => withResult(last, one.result).length > 0) ?? RESULTS[3];
  return { word: worst.word, tone: worst.tone };
}

/** The checks of `last`, a failure first: `{key, area, name, result, word, tone, detail, id, advice}`. */
export function checkRows(last) {
  return RESULTS.flatMap((one) => withResult(last, one.result).map((check) => ({
    key: `${check.area}/${check.check}`,
    area: check.area,
    name: check.check,
    result: one.result,
    word: one.word,
    tone: one.tone,
    detail: orNothing(check.detail),
    id: orNothing(check.id),
    advice: orNothing(check.advice),
  })));
}

/** How many of the checks before the last one the page lists. */
export const EARLIER = 5;

/** The checks before the last, newest first, from `history` (oldest first): `[{key, words}]`, "Nov 2 7:00 AM · a quick check · 8 passed, 1 warning · HW-115". The last check's own line is left out. */
export function earlierRows(history, last, time) {
  const lines = (Array.isArray(history) ? history : []).filter((one) => Number.isFinite(one?.at_ms)).reverse();
  const before = hasResult(last) && lines[0] && Math.floor(lines[0].at_ms / 1000) === last.time ? lines.slice(1) : lines;
  return before.slice(0, EARLIER).map((one) => {
    const counts = [
      one.passed > 0 && `${one.passed} passed`,
      one.warnings > 0 && counted(one.warnings, 'warning'),
      one.failed > 0 && `${one.failed} failed`,
      one.skipped > 0 && `${one.skipped} skipped`,
    ].filter(Boolean).join(', ') || 'nothing could be checked';
    const ids = Array.isArray(one.ids) && one.ids.length > 0 ? ` · ${one.ids.join(', ')}` : '';
    return { key: String(one.at_ms), words: `${time.when(one.at_ms / 1000)} · ${one.full ? 'a full check' : 'a quick check'} · ${counts}${ids}` };
  });
}

/** What the card says when no check was kept, and what a reader is told about it. */
export const NOT_CHECKED = 'Not checked yet.';
export const READER_CHECK = 'An administrator can check the hardware.';

/** The sentence about the disks that starts under the head; what diskhealth does on its own. */
export const DISKHEALTH_WORDS = 'diskhealth also reads every disk\'s health once a day';

/** From `GET /api/1/storage`: `{lead, notes: [{words, href}]}`, "sdb has a warning" linking to the disk; null when no disk reports its health. */
export function diskNotes(storage) {
  const disks = Array.isArray(storage?.disks) ? storage.disks : [];
  if (!disks.some((disk) => disk.health)) return null;
  const notes = disks.flatMap((disk) => {
    const verdict = disk.health?.verdict;
    if (verdict !== 'failing' && verdict !== 'warning' && !disk.health?.rose) return [];
    return [{ words: `${disk.name} ${verdict === 'failing' ? 'is failing' : 'has a warning'}`, href: `/storage/${encodeURIComponent(disk.name)}` }];
  });
  return { lead: DISKHEALTH_WORDS, notes };
}

/** The newest `hardware.check` of `tasks` that failed or was stopped after the last check was kept, in words; null for none. */
export function endedCheck(tasks, last) {
  const since = hasResult(last) ? last.time : 0;
  const endedAt = (task) => task.ended ?? task.started ?? 0;
  const found = (Array.isArray(tasks) ? tasks : [])
    .filter((task) => task.action === 'hardware.check' && (task.state === 'failed' || task.state === 'stopped') && endedAt(task) > since)
    .sort((one, other) => endedAt(other) - endedAt(one));
  return found.length > 0 ? endWords(found[0]) : null;
}

// ---- The memory test --------------------------------------------------------

/** The arguments of the memory test that runs while the system does. */
export const MEMORY_TEST_ARGS = Object.freeze({ areas: 'memory' });

export const MEMORY_WORDS = Object.freeze({
  why: 'Memory can fail without a clear sign: files that change by themselves, crashes nobody can explain.',
  running: 'The memory area of a quick check tests up to 1 GiB of the free memory, in about a minute.',
  menu: 'Restart, choose Troubleshooting, then Test the memory (memtest86+). It runs until you turn the machine off or press Esc, so someone must be at the machine; the server is offline meanwhile.',
  nothing: 'The test from the start menu leaves no record.',
});
export const READER_TEST = 'An administrator can test the memory.';
export const MEMORY_RESTART_WHY = 'To test all of the memory from the start menu.';

/** The last run while the system ran, from `last`: "Nov 9 2:10 AM · passed, 1.0G tested by 4 CPUs"; a failure or a warning comes first. */
export function memoryRunWords(last, time) {
  if (!hasResult(last)) return 'never';
  const rows = checksOf(last).filter((one) => one.area === 'memory');
  const found = rows.find((one) => one.result === 'failed') ?? rows.find((one) => one.result === 'warning')
    ?? rows.find((one) => one.check === 'memory test') ?? rows[0];
  if (!found) return `not in the last check, ${time.when(last.time)}`;
  // The detail ends with a pointer to the start menu, which the card says itself.
  const detail = orNothing(found.detail)?.split('; ')[0];
  return `${time.when(last.time)} · ${found.result}${detail ? `, ${detail}` : ''}${found.id ? ` (${found.id})` : ''}`;
}

// ---- Restart or shut down ---------------------------------------------------

export const POWER_WORDS = 'A restart or a shut down begins a minute after you ask, and you can cancel until then. A restart can be set for a time of day instead.';
export const READER_POWER = 'An administrator can restart or shut down this machine.';
