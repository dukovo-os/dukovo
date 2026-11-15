// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Overview's cards say (design 10), as plain values, so that a test
// reaches every branch: the list under the health ring, the words of the
// card for each state the checks can be in, the rows of What changed and
// This server, the services that use the most memory, and which sign-in
// notices a browser has hidden. The cards in the other files of this folder
// only draw what this file gives.

import { fingerprint } from '../../lib/store.js';
import { formatBytes, formatPercent } from '../../lib/chart-model.js';
import { duration } from '../../lib/time.js';
import { filesWords } from './part-model.js';

// ---- Health ----------------------------------------------------------------

const SEVERITY = Object.freeze({ bad: 0, warn: 1, info: 2 });

/** What a row of the list says first, which the colour and icon of the row do not say to a screen reader. */
export const SEVERITY_WORDS = Object.freeze({ bad: 'Needs you: ', warn: 'Worth a look: ', info: '' });

/** The most things the card lists before "See all". */
export const SHOWN = 4;

/** The icon of the page a finding of the checks belongs to, by its area, and `security` for an area not named here. */
const AREA_ICONS = Object.freeze({
  network: 'network', firewall: 'firewall', ssh: 'security', accounts: 'accounts', updates: 'updates', time: 'reliability',
  services: 'services', health: 'hardware', backups: 'snapshots', storage: 'storage', hardware: 'hardware',
});

const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

/** A need as a part gave it, with what it left out filled in, or null when it names nothing. */
export function normalizeNeed(raw) {
  if (!raw || typeof raw.title !== 'string' || raw.title === '') return null;
  return {
    id: String(raw.id ?? raw.title),
    severity: Object.hasOwn(SEVERITY, raw.severity) ? raw.severity : 'info',
    title: raw.title,
    detail: raw.detail ?? '',
    href: raw.href ?? null,
    icon: raw.icon ?? 'info',
    at: Number.isFinite(raw.at) ? raw.at : null,
    covers: Array.isArray(raw.covers) ? raw.covers : [],
  };
}

/** A finding of the checks as a row of the list: its words, and the fix as its detail. */
function rowOfFinding(finding) {
  return {
    id: `check-${finding.id ?? `${finding.area}-${finding.check}`}-${finding.detail}`,
    severity: finding.severity,
    title: finding.detail ? capital(finding.detail) : capital(finding.check),
    detail: finding.fix,
    href: null,
    icon: AREA_ICONS[finding.area] ?? 'security',
    at: null,
    covers: [],
    check: finding.id,
    points: finding.points,
  };
}

/**
 * The list under the ring: the parts' needs and the checks that did not
 * pass, worst first, a need before a check of the same weight and the newer
 * of two needs first. A check that a need `covers` is left out, so that one
 * fault is listed once: SEC-150 names the services that the services part
 * lists already.
 */
export function healthRows(needs, findings) {
  const rows = needs.map(normalizeNeed).filter(Boolean);
  const covered = new Set(rows.flatMap((row) => row.covers));
  const checks = findings.filter((finding) => !covered.has(finding.id)).map(rowOfFinding);
  const byTime = (a, b) => (b.at ?? -Infinity) - (a.at ?? -Infinity);
  return [...rows.map((row) => ({ ...row, kind: 'need' })), ...checks.map((row) => ({ ...row, kind: 'check' }))]
    .sort((a, b) => SEVERITY[a.severity] - SEVERITY[b.severity]
      || (a.kind === b.kind ? 0 : a.kind === 'need' ? -1 : 1)
      || byTime(a, b)
      || (b.points ?? 0) - (a.points ?? 0)
      || a.id.localeCompare(b.id));
}

/** "4 things to look at", "1 thing to look at", or "Nothing to look at". */
export function lookWords(count) {
  if (count === 0) return 'Nothing to look at';
  return `${count} ${count === 1 ? 'thing' : 'things'} to look at`;
}

/** The command that keeps a result, for a machine whose checks have not run, and the words that say so. */
export const CHECK_COMMAND = 'elevate dukovo-check';

/**
 * What the Health card shows. `checks` is where the kept result stands:
 * `{state: 'ok', assessed}` with lib/health.js's `assess`; `'missing'` for a
 * server whose checks are not there yet (ADM-129); `'reader'` for a person
 * who is not an administrator; `'none'` when no result is kept; `'error'`
 * when it could not be read. Gives `{score, band, tone, line, note,
 * command, count, rows, shown, more}`: the ring's number and band when the
 * checks answered, a tone for the line under it, the line, a note for the
 * states with no score, the command to run when there is no result, and
 * the rows, the first few of them and how many there are in all when there
 * are more.
 */
export function healthCard(checks, needs) {
  const rows = healthRows(needs, checks.state === 'ok' ? checks.assessed.findings : []);
  const base = { count: rows.length, rows, shown: rows.slice(0, SHOWN), more: rows.length > SHOWN ? rows.length : 0, command: null };
  if (checks.state === 'ok') {
    const { score, band } = checks.assessed;
    return { ...base, score, band, tone: band.id, line: band.words, note: null };
  }
  const notes = {
    missing: 'Checks are not on this server yet.',
    reader: 'The health score is for administrators.',
    none: 'Not checked yet.',
    error: 'The checks could not be read.',
  };
  const tone = rows.some((row) => row.severity === 'bad') ? 'bad' : rows.length > 0 ? 'warn' : 'off';
  const note = notes[checks.state] ?? notes.error;
  return { ...base, score: null, band: null, tone, line: note, note, command: checks.state === 'none' ? CHECK_COMMAND : null };
}

/** "Checked today at 4:00 AM", "Checked yesterday at 4:00 AM", "Checked Nov 11 4:00 AM"; `time` is lib/time.js's. */
export function checkedWords(seconds, time) {
  if (seconds === null) return null;
  const when = time.when(seconds);
  if (/^\d/.test(when)) return `Checked today at ${when}`;
  if (when.startsWith('Yesterday ')) return `Checked yesterday at ${when.slice('Yesterday '.length)}`;
  return `Checked ${when}`;
}

// ---- What changed ----------------------------------------------------------

/** How far back What changed looks when no good start is known, in seconds. */
export const WEEK = 7 * 24 * 3600;

/** The words after "What changed", and the time the list starts at (`since`, Unix seconds). */
export function changedHeading(about, time, now) {
  const start = about?.last_good_start;
  if (Number.isFinite(start)) return { more: `since the last good start, ${time.when(start)}`, since: start };
  return { more: 'in the last 7 days', since: now - WEEK };
}

/** Who made a change, as the row says: "alice in this console", "ci with the API token ci", or a hand edit's author. */
export function byWords(change) {
  const who = change.who === 'outside the console' ? 'Someone' : change.who;
  const token = /^token (.+)$/.exec(change.how ?? '');
  if (change.how === 'by hand') return `${who} edited it by hand`;
  if (token) return `By ${change.who} with the API token ${token[1]}`;
  return `By ${change.who} in this console`;
}

/**
 * One row of What changed: `{id, time, title, sub, dot, hand, unseen}`.
 * `dot` is the colour of the dot on the rail, `ok` for a hand edit that was
 * kept, and `bad` for a command that did not work.
 */
export function changeRow(change, time) {
  const hand = change.how === 'by hand';
  const files = Array.isArray(change.files) ? change.files.length : 0;
  const failed = Number.isInteger(change.status) && change.status !== 0;
  const unseen = hand && !change.seen;
  const sub = [
    byWords(change),
    hand && change.seen ? `kept by ${change.seen.by}` : null,
    !hand && files > 0 ? `${files} ${files === 1 ? 'file' : 'files'}` : null,
    failed ? 'it did not work' : null,
    change.note || null,
  ].filter(Boolean).join(' · ');
  return {
    id: change.id,
    time: time.when(change.at),
    title: hand ? `${filesWords(change)} changed outside this console` : change.title ?? change.action ?? 'A change',
    sub,
    dot: failed ? 'bad' : unseen ? 'warn' : hand ? 'ok' : 'info',
    hand,
    unseen,
  };
}

// ---- This server -----------------------------------------------------------

/** The built-in rows are ten apart, so that a part's row can sit between two. */
export const ROW_ORDER = Object.freeze({
  name: 10, system: 20, machine: 30, up: 40, time: 50, signIn: 100, keys: 110,
});

const orNothing = (words) => (words === null || words === undefined || words === '' ? null : words);

/** "Dell Inc. OptiPlex 7040 · Intel Core i5-6500 · 16 GB", whatever of it the machine says. */
export function machineWords(about) {
  const maker = orNothing(about.machine?.maker);
  const model = orNothing(about.machine?.model);
  const named = [maker, model].filter(Boolean).join(' ');
  const memory = Number.isFinite(about.memory) && about.memory > 0 ? formatBytes(about.memory) : null;
  return [named, orNothing(about.processor), memory].filter(Boolean).join(' · ') || null;
}

/** The line the phone shows under the charts: "Up for 4 h 52 min · Dell Inc. OptiPlex 7040", whatever of it the machine says. */
export function upWords(about) {
  const up = Number.isFinite(about?.up) ? `Up for ${duration(about.up)}` : null;
  const named = [orNothing(about?.machine?.maker), orNothing(about?.machine?.model)].filter(Boolean).join(' ');
  return [up, named].filter(Boolean).join(' · ') || null;
}

/** The words of the time row: the zone, the clock there and whether the clock is in sync. `serverTime` is lib/time.js's for the server's zone, or null when this browser does not know it. */
export function clockWords(about, serverTime, now) {
  const sync = about.clock?.synchronized ? 'in sync with a time source' : 'not in sync with a time source';
  const clock = serverTime ? `${serverTime.clock(now)} there` : null;
  return [about.zone, clock, sync].filter(Boolean).join(' · ');
}

/** One sign-in as words: "today at 7:12 AM from 192.168.1.25 (pts/0)". */
function signInWords(signIn, time) {
  const from = signIn.from ? ` from ${signIn.from}` : ' at the computer';
  return `${time.when(signIn.at)}${from}${signIn.terminal ? ` (${signIn.terminal})` : ''}`;
}

/**
 * The rows of This server: `[{label, value, more, mono, order}]` from
 * `about` (`GET /api/1/console/about`), sorted by `order` with the rows the
 * parts add (`{label, value, order}`). `more` is a second, quieter line.
 * `time` is lib/time.js's for the person, `serverTime` for the server's zone
 * or null, and `now` is in Unix seconds. A fact the machine does not say is
 * left out rather than shown as nothing.
 */
export function serverRows(about, { time, serverTime, now }, extra = []) {
  const rows = [];
  const add = (label, value, order, more = {}) => {
    if (value !== null && value !== undefined && value !== '') rows.push({ label, value, order, ...more });
  };
  add('Name', orNothing(about.host), ROW_ORDER.name);
  add('System', orNothing(about.release) && (about.fallback ? `${about.release} (the earlier version: the new one did not start)` : about.release), ROW_ORDER.system);
  add('Machine', machineWords(about), ROW_ORDER.machine);
  add('Up for', Number.isFinite(about.up) ? duration(about.up) : null, ROW_ORDER.up);
  add('Time', about.zone ? clockWords(about, serverTime, now) : null, ROW_ORDER.time);
  const [last, before] = Array.isArray(about.sign_ins) ? about.sign_ins : [];
  add('Last sign-in', last ? signInWords(last, time) : null, ROW_ORDER.signIn, before ? { more: `Before this: ${signInWords(before, time)}` } : {});
  add('SSH host key', orNothing(about.ssh_host_key), ROW_ORDER.keys, { mono: true });
  for (const row of extra) {
    if (typeof row?.label === 'string' && typeof row.value === 'string') {
      rows.push({ label: row.label, value: row.value, order: Number.isFinite(row.order) ? row.order : 1000 });
    }
  }
  return rows.sort((a, b) => a.order - b.order);
}

// ---- Using the most memory ----------------------------------------------------

/** How many services the card lists. */
export const TOP = 5;

/**
 * The services that use the most memory, from `metrics.now`'s object
 * (`service.memory{NAME}` and `service.cpu{NAME}`) and the list of services
 * (`{services: [{name, description}]}`): `[{name, description, memory, cpu}]`
 * with the numbers as words, most memory first. One that holds none is left
 * out, since a service that is not running uses nothing.
 */
export function topServices(now, show, count = TOP) {
  const described = new Map((Array.isArray(show?.services) ? show.services : []).map((service) => [service.name, service.description]));
  const found = [];
  for (const [key, value] of Object.entries(now ?? {})) {
    const named = /^service\.memory\{(.+)\}$/.exec(key);
    if (!named || typeof value !== 'number' || !(value > 0)) continue;
    const cpu = now[`service.cpu{${named[1]}}`];
    found.push({
      name: named[1],
      description: described.get(named[1]) ?? '',
      bytes: value,
      memory: formatBytes(value),
      cpu: typeof cpu === 'number' ? formatPercent(cpu) : '–',
    });
  }
  return found.sort((a, b) => b.bytes - a.bytes || a.name.localeCompare(b.name)).slice(0, count);
}

// ---- Notices ------------------------------------------------------------------

/** The most hidden notices a browser keeps. */
export const HIDDEN_KEPT = 100;

/** The key a hidden notice is kept under: its name and a fingerprint of its words, so that the same notice with other words is shown again. */
export const noticeKey = (notice) => `${notice.name}:${fingerprint((notice.lines ?? []).join('\n'))}`;

/** The notices of `list` that are not in `hidden`, each with its `key` and the `kind` of its banner. */
export function visibleNotices(list, hidden) {
  return (Array.isArray(list) ? list : [])
    .filter((notice) => typeof notice?.name === 'string' && Array.isArray(notice.lines) && notice.lines.length > 0)
    .map((notice) => ({ ...notice, key: noticeKey(notice), kind: notice.name === 'failed-start' ? 'warn' : 'info' }))
    .filter((notice) => !hidden.includes(notice.key));
}

/** `hidden` with `key` added, the oldest forgotten past `HIDDEN_KEPT`. */
export const hide = (hidden, key) => [...hidden.filter((one) => one !== key), key].slice(-HIDDEN_KEPT);

// ---- Usage -------------------------------------------------------------------------

/**
 * What the Usage card says when `metrics.usage` has no numbers: whether
 * metricsd, which writes them, is running. `show` is the services' document.
 */
export function noNumbersWords(show) {
  const metricsd = (Array.isArray(show?.services) ? show.services : []).find((service) => service.name === 'metricsd');
  if (metricsd && metricsd.state !== 'running') return `No numbers yet: metricsd is not running (${metricsd.state}).`;
  return 'No numbers yet. The first ones come within 10 minutes of metricsd starting.';
}
