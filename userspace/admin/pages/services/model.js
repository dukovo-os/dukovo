// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Services pages make of Spark's `show` document (design 4), as
// plain values so that a test reaches every branch: the words for a
// service's state and for whether it starts at boot, the rows of the list in
// the order the page shows them, the filter, and the address that holds it.
// The times in the document count from the start of the machine, not from
// 1970: `sinceOf` works out the clock time of one.

import { search } from '../../lib/fuzzy.js';
import { serviceHref } from './needs.js';

/**
 * The services the console is made of. The actions that end them are
 * refused, since the answer would never arrive, so the page says so before
 * a person tries.
 */
export const CONSOLE_SERVICES = Object.freeze(['admind', 'authd']);

/** Whether `name` is a service the console runs on. */
export const isConsole = (name) => CONSOLE_SERVICES.includes(name);

// The rule the service actions check a name by: the most characters, the
// words that service(8) reads as commands of its own, and the characters.
const NAME_MAX = 23;
const OWN_COMMANDS = Object.freeze(['check', 'new', 'edit', 'try', 'keep', 'undo', 'secret']);
const NAME_PATTERN = /^[A-Za-z0-9_][A-Za-z0-9_.-]*$/;

/** Whether `name` can be the name of a service: one that cannot is not asked for. */
export const isServiceName = (name) => typeof name === 'string' && name.length >= 1 && name.length <= NAME_MAX
  && NAME_PATTERN.test(name) && !OWN_COMMANDS.includes(name);

// What each state Spark names is called on the page, and the colour that
// repeats the word (`ok`, `warn`, `bad`, or `off`, a hollow dot).
const STATES = Object.freeze({
  running: { word: 'Running', kind: 'ok' },
  listening: { word: 'Ready', kind: 'ok' },
  starting: { word: 'Starting', kind: 'warn' },
  done: { word: 'Done', kind: 'ok' },
  waiting: { word: 'Waiting', kind: 'warn' },
  pending: { word: 'Waiting', kind: 'off' },
  stopping: { word: 'Stopping', kind: 'warn' },
  failed: { word: 'Failed', kind: 'bad' },
  stopped: { word: 'Stopped', kind: 'off' },
  disabled: { word: 'Stopped', kind: 'off' },
});

/** The states in which a service has a process, or is ready for one. */
const UP = Object.freeze(['running', 'listening', 'starting']);

/** Whether `service` is in a state that has a process, or waits for a connection to start one. */
export const isUp = (service) => UP.includes(service?.state);

/** `words` with its first letter in capitals. */
const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

/** The pill for the state of `service`: `{id, word, kind}`. A state this page does not know is its own word, with no colour. */
export function stateOf(service) {
  const id = typeof service?.state === 'string' ? service.state : '';
  const known = Object.hasOwn(STATES, id) ? STATES[id] : null;
  return { id, word: known?.word ?? (id === '' ? 'Unknown' : capital(id)), kind: known?.kind ?? 'off' };
}

/**
 * Whether and how `service` starts by itself: `{id, word, kind}` with the
 * id `boot`, `connection`, `timer` or `off`. A server whose Spark does not
 * give `start` and `timer` (design 3.6) has the first and the last.
 */
export function atBootOf(service) {
  if (!service?.enabled) return { id: 'off', word: 'Off', kind: '' };
  if (service.timer === true) return { id: 'timer', word: 'By its timer', kind: 'info' };
  if (service.start === 'on-connection') return { id: 'connection', word: 'On connection', kind: 'info' };
  return { id: 'boot', word: 'Starts at boot', kind: 'info' };
}

/**
 * The time, in Unix seconds, that a moment of the machine's clock was.
 * `nowMs` is the time of the document and `ms` the moment, both in
 * milliseconds since the machine started; `now` is the browser's time. Null
 * when either is not known. A moment ahead of the document is now.
 */
export function sinceOf(nowMs, ms, now) {
  if (!Number.isFinite(nowMs) || !Number.isFinite(ms)) return null;
  return Math.floor(now - Math.max(0, nowMs - ms) / 1000);
}

/** The trial in `show` that was begun for the service `name`, or null: a `service try` names it `service.NAME`. */
export function trialOf(show, name) {
  const trials = Array.isArray(show?.trials) ? show.trials : [];
  return trials.find((trial) => trial?.id === `service.${name}`) ?? null;
}

/** Failed services come first, then by name, as characters go. */
const compareRows = (a, b) => Number(b.state.id === 'failed') - Number(a.state.id === 'failed')
  || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);

/**
 * The rows of the list for `show`, in the order shown. `now` is the
 * browser's time in Unix seconds. A row is `{name, href, description,
 * state, atBoot, since, trying}`; `title` and `keywords` are what
 * lib/fuzzy.js matches a filter against.
 */
export function rowsOf(show, now) {
  const services = Array.isArray(show?.services) ? show.services : [];
  return services
    .filter((service) => typeof service?.name === 'string' && service.name !== '')
    .map((service) => {
      const description = typeof service.description === 'string' ? service.description : '';
      return {
        name: service.name,
        title: service.name,
        keywords: [description],
        href: serviceHref(service.name),
        description,
        state: stateOf(service),
        atBoot: atBootOf(service),
        since: sinceOf(show.now_ms, service.since_ms, now),
        trying: trialOf(show, service.name) !== null,
      };
    })
    .sort(compareRows);
}

/** The choices of the state filter, as `{id, words}`; the first is no filter. */
export const STATE_CHOICES = Object.freeze([
  { id: 'any', words: 'Any state' },
  { id: 'running', words: 'Running' },
  { id: 'failed', words: 'Failed' },
  { id: 'stopped', words: 'Stopped' },
  { id: 'off-at-boot', words: 'Off at boot' },
  { id: 'trying', words: 'Being tried' },
]);

/** Whether `row` is in the state filter `state`: Running has a process or is ready for one, Stopped is stopped or disabled. */
export function passesState(row, state) {
  switch (state) {
    case 'running': return UP.includes(row.state.id);
    case 'failed': return row.state.id === 'failed';
    case 'stopped': return row.state.id === 'stopped' || row.state.id === 'disabled';
    case 'off-at-boot': return row.atBoot.id === 'off';
    case 'trying': return row.trying;
    default: return true;
  }
}

/**
 * The rows that pass `{q, state}`, in the order they came. The words of `q`
 * match the name and the description as the palette matches: each must be
 * found, letter by letter in order, ignoring case and accents.
 */
export function filterRows(rows, { q, state }) {
  const inState = rows.filter((row) => passesState(row, state));
  if (q.trim() === '') return inState;
  const found = new Set(search(inState, q, { limit: Infinity }).map((one) => one.row));
  return inState.filter((row) => found.has(row));
}

/** The caption of the table, which the page's h1 makes visually redundant: "Services, 3 shown of 12". */
export const captionOf = (shown, total) => `Services, ${shown} shown of ${total}`;

/** The filter in the address `params` (a URLSearchParams): `{q, state}`, with a state that is not one of the choices read as any. */
export function queryOf(params) {
  const state = params.get('state');
  return { q: params.get('q') ?? '', state: STATE_CHOICES.some((choice) => choice.id === state) ? state : 'any' };
}

/** The address of the list with the filter `{q, state}` in it, leaving out what is no filter. */
export function addressOf({ q, state }) {
  const params = new URLSearchParams();
  if (q.trim() !== '') params.set('q', q);
  if (state !== 'any') params.set('state', state);
  const text = params.toString();
  return text === '' ? '/services' : `/services?${text}`;
}
