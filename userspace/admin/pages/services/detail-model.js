// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the page of one service says (design 4.4), as plain values so that a
// test reaches every branch: which buttons it offers and why it offers none,
// the words each button runs with, the rows of its cards from `service NAME
// status --json`, the banner that explains a service that is not running,
// and the words of a name Spark does not know. The page that draws them is
// detail.js.

import { duration } from '../../lib/time.js';
import { isConsole, isUp, sinceOf } from './model.js';
import { detailOf } from './needs.js';

/** `words` with its first letter in capitals. */
const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

/** A sentence ends in a full stop, a question mark or an exclamation mark. */
const ended = (words) => (/[.!?]$/.test(words) ? words : `${words}.`);

/** `count` and its noun, with an s when it is not one. */
const counted = (count, noun) => `${count} ${noun}${count === 1 ? '' : 's'}`;

/** The strings of `list` that are not empty, or none when it is not a list. */
const namesOf = (list) => (Array.isArray(list) ? list.filter((one) => typeof one === 'string' && one !== '') : []);

// ---- What the person may do ------------------------------------------------

/** What a reader is told in place of the buttons. */
export const READER_LINE = 'Administrators can start and stop services.';

/** The command that stops or restarts a service of the console, at a terminal. */
export const consoleCommand = (name) => `elevate service ${name} restart`;

/** Why the page does not stop or restart `name`, a service the console is made of. */
export const consoleWords = (name) => `The console runs on ${name}, so this page does not stop it, restart it or turn off its start at boot: that is done at a terminal.`;

// The buttons by what they run. `id` is the name a test and the page know them by.
const START = { id: 'start', action: 'service.start', label: 'Run now' };
const STOP = { id: 'stop', action: 'service.stop', label: 'Stop' };
const RESTART = { id: 'restart', action: 'service.restart', label: 'Restart' };

/**
 * What the page offers for `service` to a person who is an administrator
 * (`admin`) or not. Gives `{buttons, boot, note}`: the buttons, each `{id,
 * action, label}`; the switch Starts at boot as `{on, action, fixed}`, where
 * `fixed` says that the console would refuse turning it off (the actions
 * refuse `admind` and `authd`), or null for a reader, who has none; and the
 * words that stand for what is not offered, with the command to run at a
 * terminal when there is one.
 */
export function controlsOf(service, admin) {
  if (!admin) return { buttons: [], boot: null, note: { words: READER_LINE, command: null } };
  const own = isConsole(service.name);
  const on = service.enabled === true;
  const boot = { on, action: on ? 'service.disable' : 'service.enable', fixed: own && on };
  let buttons = [START];
  if (isUp(service)) buttons = own ? [] : [STOP, RESTART];
  else if (service.state === 'stopping') buttons = [];
  const refused = own && (isUp(service) || boot.fixed);
  return {
    buttons,
    boot,
    note: refused ? { words: consoleWords(service.name), command: consoleCommand(service.name) } : null,
  };
}

/**
 * The words that go with running `action` for the service `name`, as
 * lib/action.js takes them: what the unlock dialog says the person is about
 * to do, the notice while it runs and when it is done, the notice when it
 * changed nothing, and the busy button's. Enabling starts the service and
 * disabling stops it, as service(8) says, and the words say so.
 */
export function wordsFor(action, name) {
  switch (action) {
    case 'service.start':
      return { unlock: `start ${name}`, running: `Starting ${name}`, done: `${name} started.`, unchanged: `${name} was running already.`, busy: 'Starting…' };
    case 'service.stop':
      return { unlock: `stop ${name}`, running: `Stopping ${name}`, done: `${name} stopped.`, unchanged: `${name} was stopped already.`, busy: 'Stopping…' };
    case 'service.restart':
      return { unlock: `restart ${name}`, running: `Restarting ${name}`, done: `${name} restarted.`, unchanged: `${name} restarted.`, busy: 'Restarting…' };
    case 'service.enable':
      return {
        unlock: `start ${name} now and at every boot`, running: `Turning on ${name}`, done: `${name} starts at boot, and runs now.`,
        unchanged: `${name} starts at boot already.`, busy: 'Turning on…',
      };
    case 'service.disable':
      return {
        unlock: `stop ${name} and turn off its start at boot`, running: `Turning off ${name}`, done: `${name} no longer starts at boot, and is stopped.`,
        unchanged: `${name} did not start at boot already.`, busy: 'Turning off…',
      };
    default:
      return {};
  }
}

/** The line under the switch Starts at boot, which says what turning it on or off does besides. */
export const BOOT_HINT = 'Turning it on also starts the service now, and turning it off also stops it.';

// ---- Why it is in its state ------------------------------------------------

// The states in which Spark's reason and the chain of services behind it are worth a sentence.
const EXPLAINED = Object.freeze(['failed', 'stopped', 'disabled', 'pending', 'waiting', 'stopping']);

/** Whether the page explains the state of `service`: one that is not running or done. */
export const explainable = (service) => EXPLAINED.includes(service?.state);

/**
 * The banner above the cards: `{kind, sentence, fix, root}`. `explanation`
 * is `service NAME explain --json`'s answer `{sentence, chain, root, fix}`,
 * or null when it could not be read, and then Spark's own words of the
 * service stand in. `root` is the service that is behind it, when it is
 * not this one.
 */
export function bannerOf(service, explanation) {
  const kind = service.state === 'failed' ? 'bad' : 'warn';
  if (typeof explanation?.sentence === 'string' && explanation.sentence !== '') {
    const root = typeof explanation.root === 'string' && explanation.root !== service.name ? explanation.root : null;
    return { kind, sentence: explanation.sentence, fix: typeof explanation.fix === 'string' && explanation.fix !== '' ? explanation.fix : null, root };
  }
  return { kind, sentence: `${service.name} is not running. ${detailOf(service)}`, fix: null, root: null };
}

/** The line under the state when the service's health check has failed, or null. */
export function healthOf(service) {
  const health = service?.health;
  if (!health || !(health.failures > 0)) return null;
  const last = typeof health.last_failure === 'string' && health.last_failure !== '' ? `: ${ended(health.last_failure)}` : '.';
  const restart = health.restart_after > 0 ? ` Spark restarts it after ${counted(health.restart_after, 'failed check')} in a row.` : '';
  return `Its health check failed ${counted(health.failures, 'time')} in a row${last}${restart}`;
}

/** What Spark says is wrong with the service's file while it runs the last good copy, or null. */
export const complaintOf = (service) => (typeof service?.complaint === 'string' && service.complaint !== '' ? ended(capital(service.complaint)) : null);

// ---- The cards -------------------------------------------------------------

/** The clock time of the document `status`, in milliseconds since the machine started: beside `service`, or inside it. */
export function nowMsOf(status) {
  if (Number.isFinite(status?.now_ms)) return status.now_ms;
  return Number.isFinite(status?.service?.now_ms) ? status.service.now_ms : null;
}

/** How long it took to be ready, from `ms`, in the shortest honest words. */
export function readyWords(ms) {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 10_000) return `${(ms / 1000).toFixed(1)} s`;
  return duration(ms / 1000);
}

const STARTED_BY = Object.freeze({ boot: 'The machine starting' });

const RESTART_RULES = Object.freeze({
  always: 'Starts again whenever it ends',
  'on-failure': 'Starts again when it fails',
  never: 'Does not start again when it ends',
});

/** The restart rule of `service` in words, with when Spark gives up, or null when Spark does not say. */
export function restartWords(service) {
  if (typeof service.restart !== 'string' || service.restart === '') return null;
  const rule = Object.hasOwn(RESTART_RULES, service.restart) ? RESTART_RULES[service.restart] : capital(service.restart);
  const given = service.give_up_after;
  return given > 0 ? `${rule}, and gives up after ${counted(given, 'start')} that ${given === 1 ? 'fails' : 'fail'} in a row` : rule;
}

// The most of a service's other processes the card lists.
const MORE_PROCESSES = 5;

/** A process as a line: its number, what it runs and who it runs as. */
const processWords = (process) => [process.pid, process.program, process.user ? `as ${process.user}` : null].filter(Boolean).join(' ');

/** The lines of the Processes row: the service's own process first, then up to five more, and how many are left out. */
function processLines(service, processes) {
  const list = (Array.isArray(processes) ? processes : []).filter((one) => Number.isFinite(one?.pid));
  const main = list.find((one) => one.pid === service.pid) ?? (Number.isFinite(service.pid) ? { pid: service.pid } : null);
  const others = list.filter((one) => one.pid !== main?.pid);
  const shown = [...(main ? [main] : []), ...others].slice(0, 1 + MORE_PROCESSES);
  const lines = shown.map(processWords);
  const left = (main ? 1 : 0) + others.length - shown.length;
  if (left > 0) lines.push(`and ${left} more`);
  return lines;
}

/** `{port, tcp}` as "port 443 (TCP)". */
const listeningWords = (one) => `port ${one.port} (${one.tcp === false ? 'UDP' : 'TCP'})`;

/**
 * The rows of the Runs card: since when, its processes, where it listens,
 * restarts so far, what started it, how long it took to be ready and its
 * restart rule. Each is `{label, value}` or `{label, items}`; one that Spark
 * has nothing for is left out. `now` is the browser's time in Unix seconds
 * and `time` the person's formatters (lib/time.js).
 */
export function runsOf(status, now, time) {
  const service = status.service ?? {};
  const rows = [];
  const since = sinceOf(nowMsOf(status), service.since_ms, now);
  if (since !== null) rows.push({ label: 'Since', value: time.when(since) });
  const lines = processLines(service, status.processes);
  if (lines.length > 0) rows.push({ label: 'Processes', items: lines });
  const listening = (Array.isArray(status.listening) ? status.listening : []).filter((one) => Number.isFinite(one?.port));
  if (listening.length > 0) rows.push({ label: 'Listening on', items: listening.map(listeningWords) });
  if (Number.isFinite(service.restarts)) {
    rows.push({ label: 'Restarts', value: service.restarts === 0 ? 'None since the machine started' : `${service.restarts} since the machine started` });
  }
  if (typeof service.started_by === 'string' && service.started_by !== '') {
    rows.push({ label: 'Started by', value: Object.hasOwn(STARTED_BY, service.started_by) ? STARTED_BY[service.started_by] : capital(service.started_by) });
  }
  if (Number.isFinite(service.started_ms) && Number.isFinite(service.ready_ms) && service.ready_ms >= service.started_ms) {
    rows.push({ label: 'Ready in', value: readyWords(service.ready_ms - service.started_ms) });
  }
  const rule = restartWords(service);
  if (rule !== null) rows.push({ label: 'Restart rule', value: rule });
  return rows;
}

/** The rows of the Needs card: `{label, names}`, each name a service to link to. A list that is empty is left out. */
export function needsRows(service) {
  return [
    { label: 'Requires', names: namesOf(service.requires) },
    { label: 'Wants', names: namesOf(service.wants) },
    { label: 'Starts after', names: namesOf(service.after) },
    { label: 'Waiting for', names: namesOf(service.waiting_for) },
  ].filter((row) => row.names.length > 0);
}

/**
 * The rows of the Holds card, from what the service's file gives it: what it
 * reaches and may do, in the words of service.toml(5). Each is `{label,
 * value}` or `{label, items}`.
 */
export function holdsRows(service) {
  const holds = service.holds;
  if (!holds || typeof holds !== 'object') return [];
  const rows = [];
  const items = (label, list) => {
    const names = namesOf(list);
    if (names.length > 0) rows.push({ label, items: names });
  };
  if (holds.whole_tree) rows.push({ label: 'Files', value: 'The whole tree, with the machine\'s own access' });
  rows.push({ label: 'Acts as', value: typeof holds.account === 'string' && holds.account !== '' ? holds.account : 'root' });
  items('May read', holds.reads);
  items('May change', holds.changes);
  items('Mounts under', holds.mounts);
  if (typeof holds.devices === 'string' && holds.devices !== '') {
    rows.push({ label: 'Devices', value: `${holds.devices}, ${holds.devices_write ? 'to read and write' : 'to read only'}` });
  }
  items('Volumes', holds.root_subvolumes);
  if (holds.every_process) rows.push({ label: 'Processes', value: 'Sees every process on the machine' });
  items('Secrets', holds.secrets);
  items('May listen on', holds.listen);
  rows.push({ label: 'Connects out', value: holds.outbound ? 'Yes' : 'No' });
  items('Settings it may change', holds.admin);
  if (holds.names) rows.push({ label: 'Names', value: 'May give processes and files another user\'s name' });
  items('Cards', holds.cards);
  return rows;
}

/**
 * The card for a trial that `service try` began: its words, with the time
 * it is undone as the time of day, and the commands that keep or undo it at
 * a terminal. Null when none runs. `readAt` is the browser's time in Unix
 * seconds when `status` was read, and `time` the person's formatters.
 */
export function triedOf(status, readAt, time) {
  const trial = status?.trial;
  if (!trial || typeof trial !== 'object') return null;
  const name = status.service?.name ?? '';
  const ends = Number.isFinite(trial.left_ms) ? Math.floor(readAt + trial.left_ms / 1000) : null;
  return {
    words: `A change to this service's file is being tried. It is undone ${ends === null ? '' : `at ${time.when(ends)} `}unless it is kept.`,
    keep: `elevate service keep ${name}`,
    undo: `elevate service undo ${name}`,
  };
}

// ---- A name Spark does not know --------------------------------------------

/** Whether `error` says that Spark has no service of the name asked for: the command ended with 4, or the route found none. */
export const unknownService = (error) => error?.status === 404 || (error?.id === 'ADM-123' && error.command?.status === 4);

/**
 * The words of the error view for a service called `name` that there is not.
 * `error` is what admind answered, or null when the name could not be one and
 * nothing was asked: its ID and the program's own words stay with the error.
 */
export function unknownWords(name, error) {
  return {
    id: error?.id ?? null,
    title: `There is no service called ${name}`,
    detail: 'Spark does not know a service by that name. It may have been removed, or the address may have a mistake in it.',
    fix: 'Open the list to see the services this server has.',
    status: error?.status ?? 404,
    command: error?.command ?? null,
  };
}
