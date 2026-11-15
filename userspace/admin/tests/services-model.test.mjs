// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The list of services (design 4.3): the words of each state and of when a
// service starts, the rows in the order shown, the filter and the address
// that holds it, and the scheduled tasks: a schedule in words and the times
// of each timer on its own clock.

import assert from 'node:assert/strict';
import test from 'node:test';

import { makeTime } from '../lib/time.js';
import {
  addressOf, atBootOf, captionOf, CONSOLE_SERVICES, filterRows, isConsole, isServiceName, isUp, passesState,
  queryOf, rowsOf, sinceOf, stateOf, STATE_CHOICES, trialOf,
} from '../pages/services/model.js';
import { serviceHref } from '../pages/services/needs.js';
import {
  scheduleWords, TIMER_NOTES, timerCardRows, timerHeaders, timerRows,
} from '../pages/services/timer-model.js';
import {
  document, NOW, NOW_MS, service, timers, withStarts,
} from './services-fixtures.mjs';

test('each state Spark names is a word with a colour that only repeats it', () => {
  const words = Object.fromEntries(['running', 'listening', 'starting', 'done', 'waiting', 'pending', 'stopping', 'failed', 'stopped', 'disabled']
    .map((state) => [state, stateOf({ state })]));
  assert.deepEqual(words.running, { id: 'running', word: 'Running', kind: 'ok' });
  assert.deepEqual(words.listening, { id: 'listening', word: 'Ready', kind: 'ok' });
  assert.deepEqual(words.starting, { id: 'starting', word: 'Starting', kind: 'warn' });
  assert.deepEqual(words.done, { id: 'done', word: 'Done', kind: 'ok' });
  assert.deepEqual(words.waiting, { id: 'waiting', word: 'Waiting', kind: 'warn' });
  assert.deepEqual(words.pending, { id: 'pending', word: 'Waiting', kind: 'off' });
  assert.deepEqual(words.stopping, { id: 'stopping', word: 'Stopping', kind: 'warn' });
  assert.deepEqual(words.failed, { id: 'failed', word: 'Failed', kind: 'bad' });
  assert.deepEqual(words.stopped, { id: 'stopped', word: 'Stopped', kind: 'off' });
  assert.deepEqual(words.disabled, { id: 'disabled', word: 'Stopped', kind: 'off' });
});

test('a state this page does not know is said in its own word, and no state is unknown', () => {
  assert.deepEqual(stateOf({ state: 'frozen' }), { id: 'frozen', word: 'Frozen', kind: 'off' });
  assert.deepEqual(stateOf({}), { id: '', word: 'Unknown', kind: 'off' });
  assert.deepEqual(stateOf(undefined), { id: '', word: 'Unknown', kind: 'off' });
  assert.deepEqual(stateOf({ state: 'constructor' }), { id: 'constructor', word: 'Constructor', kind: 'off' }, 'a name of Object is no state');
});

test('a service is up while it has a process or waits for a connection to start one', () => {
  const up = ['running', 'listening', 'starting'];
  for (const state of ['running', 'listening', 'starting', 'done', 'waiting', 'pending', 'stopping', 'failed', 'stopped', 'disabled']) {
    assert.equal(isUp({ state }), up.includes(state), state);
  }
  assert.equal(isUp(undefined), false);
});

test('At boot says Starts at boot, or Off when the file says so', () => {
  assert.deepEqual(atBootOf({ enabled: true }), { id: 'boot', word: 'Starts at boot', kind: 'info' });
  assert.deepEqual(atBootOf({ enabled: false }), { id: 'off', word: 'Off', kind: '' });
  assert.deepEqual(atBootOf({}), { id: 'off', word: 'Off', kind: '' });
  assert.deepEqual(atBootOf(undefined), { id: 'off', word: 'Off', kind: '' });
});

test('with #390 At boot says On connection or By its timer, and Off still wins', () => {
  assert.equal(atBootOf({ enabled: true, start: 'on-connection' }).word, 'On connection');
  assert.equal(atBootOf({ enabled: true, start: 'at-boot' }).word, 'Starts at boot');
  assert.equal(atBootOf({ enabled: true, start: 'at-boot', timer: true }).word, 'By its timer');
  assert.equal(atBootOf({ enabled: true, start: 'on-connection', timer: true }).word, 'By its timer');
  assert.equal(atBootOf({ enabled: false, start: 'on-connection' }).word, 'Off');
  assert.equal(atBootOf({ enabled: false, timer: true }).word, 'Off', 'a timer that is off does not run');
});

test('the time a service came to its state is worked out from the machine\'s clock', () => {
  assert.equal(sinceOf(NOW_MS, NOW_MS - 3_600_000, NOW), NOW - 3600);
  assert.equal(sinceOf(NOW_MS, NOW_MS, NOW), NOW);
  assert.equal(sinceOf(NOW_MS, NOW_MS + 5000, NOW), NOW, 'a moment ahead of the document is now');
  assert.equal(sinceOf(NOW_MS, NOW_MS - 1500, NOW), NOW - 2, 'to the second below');
  for (const [nowMs, ms] of [[undefined, 1], [1, undefined], [null, 1], [NaN, 1], ['1', 1]]) assert.equal(sinceOf(nowMs, ms, NOW), null);
});

test('the rows have the failed services first, then each by name', () => {
  const rows = rowsOf(document, NOW);
  assert.deepEqual(rows.map((row) => row.name), [
    'backup-sync', 'Notes_2', 'admind', 'db', 'logrotate', 'mailer', 'nginx', 'printer', 'tunnel', 'web2',
  ]);
});

test('a row has the words of the page: the state, At boot, since when, and the address', () => {
  const [failed, , , , , , nginx, printer] = rowsOf(document, NOW);
  assert.deepEqual(failed, {
    name: 'backup-sync',
    title: 'backup-sync',
    keywords: ['Copies backups to the NAS'],
    href: '/services/backup-sync',
    description: 'Copies backups to the NAS',
    state: { id: 'failed', word: 'Failed', kind: 'bad' },
    atBoot: { id: 'boot', word: 'Starts at boot', kind: 'info' },
    since: NOW - 10_800,
    trying: false,
  });
  assert.equal(nginx.since, NOW - 3600);
  assert.equal(nginx.trying, true, 'a service with a trial running is tried');
  assert.equal(printer.state.word, 'Stopped');
  assert.equal(printer.atBoot.word, 'Off');
});

test('a network trial is no service\'s, and a trial marks only the service it names', () => {
  assert.deepEqual(rowsOf(document, NOW).filter((row) => row.trying).map((row) => row.name), ['nginx']);
  assert.equal(trialOf(document, 'nginx').left_ms, 42_000);
  assert.equal(trialOf(document, 'network'), null, 'the id of a service\'s trial is service.NAME');
  assert.equal(trialOf(document, 'web'), null);
  assert.equal(trialOf({}, 'nginx'), null);
});

test('rows are made of what Spark gave, and skip what has no name', () => {
  const odd = { now_ms: 10, services: [null, {}, { name: '' }, { name: 7 }, { name: 'ok', state: 'running' }, 'text'] };
  assert.deepEqual(rowsOf(odd, NOW).map((row) => [row.name, row.description, row.since]), [['ok', '', null]]);
  for (const answer of [null, undefined, {}, { services: null }, { services: {} }, []]) assert.deepEqual(rowsOf(answer, NOW), []);
});

test('a name is made safe in the address of its page, and a dot is written out so that the server does not take it for a file', () => {
  assert.equal(serviceHref('nginx'), '/services/nginx');
  assert.equal(serviceHref('a b/c'), '/services/a%20b%2Fc');
  assert.equal(serviceHref('web.example-2'), '/services/web%2Eexample-2');
  assert.equal(rowsOf({ services: [{ name: 'x.y_z-1' }] }, NOW)[0].href, '/services/x%2Ey_z-1');
  assert.equal(new URL(serviceHref('a.b'), 'https://host').pathname.split('/').pop().includes('.'), false);
  assert.equal(decodeURIComponent('web%2Eexample-2'), 'web.example-2');
});

test('the filter matches the name and the description, in any case, and keeps the order of the list', () => {
  const rows = rowsOf(document, NOW);
  const names = (found) => found.map((row) => row.name);
  assert.deepEqual(names(filterRows(rows, { q: 'nginx', state: 'any' })), ['nginx']);
  assert.deepEqual(names(filterRows(rows, { q: 'WEB', state: 'any' })), ['nginx', 'web2']);
  assert.deepEqual(names(filterRows(rows, { q: 'the nas', state: 'any' })), ['backup-sync'], 'the description counts, and its words may come in any order');
  assert.deepEqual(names(filterRows(rows, { q: 'notes', state: 'any' })), ['Notes_2']);
  assert.deepEqual(names(filterRows(rows, { q: 'zzz', state: 'any' })), []);
  assert.equal(filterRows(rows, { q: '   ', state: 'any' }).length, rows.length, 'spaces are no filter');
});

test('each state filter keeps the rows it names', () => {
  const rows = rowsOf(document, NOW);
  const names = (state) => filterRows(rows, { q: '', state }).map((row) => row.name);
  assert.deepEqual(names('any'), rows.map((row) => row.name));
  assert.deepEqual(names('running'), ['admind', 'nginx', 'web2'], 'a service that has a process, ready or not');
  assert.deepEqual(names('failed'), ['backup-sync']);
  assert.deepEqual(names('stopped'), ['Notes_2', 'printer'], 'stopped and disabled, not one that waits or is done');
  assert.deepEqual(names('off-at-boot'), ['Notes_2', 'printer']);
  assert.deepEqual(names('trying'), ['nginx']);
  assert.deepEqual(names('nothing-like-it'), rows.map((row) => row.name), 'a word that is no filter is none');
  assert.equal(passesState(rows[0], 'failed'), true);
});

test('a state and words filter together', () => {
  const rows = rowsOf(document, NOW);
  assert.deepEqual(filterRows(rows, { q: 'web', state: 'running' }).map((row) => row.name), ['nginx', 'web2']);
  assert.deepEqual(filterRows(rows, { q: 'web', state: 'failed' }), []);
});

test('the caption says how many of the services are shown', () => {
  assert.equal(captionOf(3, 12), 'Services, 3 shown of 12');
  assert.equal(captionOf(0, 0), 'Services, 0 shown of 0');
});

test('the filter is read from the address, and a state that is none of the choices is any', () => {
  assert.deepEqual(queryOf(new URLSearchParams('')), { q: '', state: 'any' });
  assert.deepEqual(queryOf(new URLSearchParams('q=web+server&state=failed')), { q: 'web server', state: 'failed' });
  assert.deepEqual(queryOf(new URLSearchParams('state=sleeping')), { q: '', state: 'any' });
  for (const choice of STATE_CHOICES) assert.equal(queryOf(new URLSearchParams(`state=${choice.id}`)).state, choice.id);
});

test('the address holds the filter, and leaves out what is no filter', () => {
  assert.equal(addressOf({ q: '', state: 'any' }), '/services');
  assert.equal(addressOf({ q: '  ', state: 'any' }), '/services');
  assert.equal(addressOf({ q: 'web', state: 'any' }), '/services?q=web');
  assert.equal(addressOf({ q: '', state: 'off-at-boot' }), '/services?state=off-at-boot');
  assert.equal(addressOf({ q: 'a&b c', state: 'failed' }), '/services?q=a%26b+c&state=failed');
  for (const query of [{ q: 'a&b c', state: 'failed' }, { q: 'x', state: 'trying' }, { q: '', state: 'any' }]) {
    assert.deepEqual(queryOf(new URL(addressOf(query), 'https://host').searchParams), query);
  }
});

test('the state choices begin with no filter and name each of the rest once', () => {
  assert.deepEqual(STATE_CHOICES.map((choice) => choice.id), ['any', 'running', 'failed', 'stopped', 'off-at-boot', 'trying']);
  assert.equal(new Set(STATE_CHOICES.map((choice) => choice.words)).size, STATE_CHOICES.length);
});

test('with #390 the list says when each service starts', () => {
  const rows = rowsOf(withStarts, NOW);
  assert.deepEqual(Object.fromEntries(rows.map((row) => [row.name, row.atBoot.word])), {
    cleanup: 'By its timer', syncer: 'Off', web: 'On connection',
  });
  assert.equal(rows.find((row) => row.name === 'web').state.word, 'Ready');
});

test('a service name is one the actions accept, so that nothing else is asked for', () => {
  for (const good of ['nginx', 'Notes_2', '_x', '9lives', 'a.b-c', 'a'.repeat(23)]) assert.equal(isServiceName(good), true, good);
  for (const bad of ['', '.hidden', '-x', 'a b', 'a/b', '../x', 'ü', 'a'.repeat(24), 'check', 'new', 'edit', 'try', 'keep', 'undo', 'secret', null, undefined, 7]) {
    assert.equal(isServiceName(bad), false, String(bad));
  }
  assert.equal(isServiceName('checker'), true);
});

test('the console\'s own services are admind and authd', () => {
  assert.deepEqual([...CONSOLE_SERVICES], ['admind', 'authd']);
  assert.equal(isConsole('admind'), true);
  assert.equal(isConsole('authd'), true);
  assert.equal(isConsole('nginx'), false);
  assert.equal(isConsole('admind2'), false);
});

test('a service helper of the fixtures is the document\'s shape', () => {
  const made = service('x', 'y', 'running', 60);
  assert.equal(made.since_ms, NOW_MS - 60_000);
  assert.equal(made.enabled, true);
});

// ---- the scheduled tasks ---------------------------------------------------------

test('a schedule is said in words: times of day, a name, every and after boot', () => {
  const said = (schedule) => scheduleWords(schedule);
  assert.equal(said('0 3 * * *'), 'Every day at 3:00 AM');
  assert.equal(said('30 15 * * *'), 'Every day at 3:30 PM');
  assert.equal(said('0 0 * * *'), 'Every day at 12:00 AM');
  assert.equal(said('0 3,15 * * *'), 'Every day at 3:00 AM and 3:00 PM');
  assert.equal(said('30 3 * * 1-5'), 'Monday to Friday at 3:30 AM');
  assert.equal(said('0 3 * * 1,3'), 'Every Monday and Wednesday at 3:00 AM');
  assert.equal(said('0 3 * * 7'), 'Every Sunday at 3:00 AM');
  assert.equal(said('15 14 22 * *'), 'On the 22nd of every month at 2:15 PM');
  assert.equal(said('0 0 1 * *'), 'On the 1st of every month at 12:00 AM');
  assert.equal(said('0 0 3 * *'), 'On the 3rd of every month at 12:00 AM');
  assert.equal(said('0 0 11 * *'), 'On the 11th of every month at 12:00 AM');
  assert.equal(said('* * * * *'), 'Every minute');
  assert.equal(said('*/15 * * * *'), 'Every 15 minutes');
  assert.equal(said('5 * * * *'), 'Every hour, 5 minutes past');
  assert.equal(said('0 * * * *'), 'Every hour');
  assert.equal(said('0 */6 * * *'), 'Every 6 hours');
  assert.equal(said('20 */4 * * *'), 'Every 4 hours, 20 minutes past');
  for (const [word, words] of [['hourly', 'Every hour'], ['daily', 'Every day'], ['weekly', 'Every week'], ['monthly', 'Every month'], ['yearly', 'Every year']]) {
    assert.equal(said(word), words);
  }
  assert.equal(said('every 6h'), 'Every 6 hours');
  assert.equal(said('every 1h'), 'Every hour');
  assert.equal(said('every 90m'), 'Every 90 minutes');
  assert.equal(said('every 1d12h'), 'Every 1 day and 12 hours');
  assert.equal(said('every 1w2d3h'), 'Every 1 week, 2 days and 3 hours');
  assert.equal(said('0 3 * * *, every 6h, first 10m after boot'), 'Every day at 3:00 AM, every 6 hours, the first start comes 10 minutes after the machine starts');
});

test('a schedule that is not one of those is shown as written, and a file that gives none says so', () => {
  assert.equal(scheduleWords('0 3 1 6 *'), '0 3 1 6 *');
  assert.equal(scheduleWords('0 3 * * mon'), '0 3 * * mon');
  assert.equal(scheduleWords('0 25 * * *'), '0 25 * * *');
  assert.equal(scheduleWords('*/1 * * * *'), '*/1 * * * *');
  assert.equal(scheduleWords('0 3 * * *, 0 3 1 6 *'), 'Every day at 3:00 AM, 0 3 1 6 *');
  assert.equal(scheduleWords(null), null);
  assert.equal(scheduleWords(''), null);
});

test('the timers are in the order they run next, a timer with no next run last', () => {
  const rows = timerRows(timers, { time: makeTime('UTC', () => NOW * 1000), nowMs: NOW * 1000 });
  assert.deepEqual(rows.map((row) => row.service), ['certs', 'cleanup', 'backup-sync', 'logrotate']);
  assert.deepEqual(timerRows(timers.slice(0, 1).concat(timers.slice(0, 1)), { time: makeTime('UTC', () => NOW * 1000), nowMs: NOW * 1000 }).map((row) => row.service), ['backup-sync', 'backup-sync']);
  assert.equal(rows[0].href, '/services/certs');
  assert.deepEqual(timerRows(undefined, { time: makeTime('UTC'), nowMs: 0 }), []);
});

test('a timer on the person\'s clock shows its times as the pages do, and one on another says whose clock it is', () => {
  const rows = timerRows(timers, { time: makeTime('UTC', () => NOW * 1000), nowMs: NOW * 1000 });
  const byName = Object.fromEntries(rows.map((row) => [row.service, row]));
  assert.equal(byName.cleanup.server, false);
  assert.deepEqual([byName.cleanup.next.words, byName.cleanup.last.words], ['12:00 PM', '6:00 AM']);
  assert.equal(byName.cleanup.next.iso, '2026-11-14T12:00:00.000Z');
  assert.equal(byName.cleanup.next.full, 'Nov 14, 2026, 12:00:00 PM');
  // Prague is an hour ahead of UTC in November: the same moments, on the timer's own clock, with its zone after them.
  assert.equal(byName['backup-sync'].server, true);
  assert.deepEqual([byName['backup-sync'].next.words, byName['backup-sync'].last.words], ['Nov 15 3:00 AM Europe/Prague', '3:00 AM Europe/Prague']);
  assert.equal(byName['backup-sync'].next.full, 'Nov 15, 2026, 3:00:00 AM Europe/Prague');
  assert.equal(byName['backup-sync'].next.iso, '2026-11-15T02:00:00.000Z');
  // A timer is on the person's clock when its own is the same, whatever the name of the person's zone says of the moment.
  const prague = timerRows(timers, { time: makeTime('Europe/Prague', () => NOW * 1000), nowMs: NOW * 1000 });
  assert.equal(prague.find((row) => row.service === 'backup-sync').server, false);
  assert.equal(prague.find((row) => row.service === 'backup-sync').next.words, 'Nov 15 3:00 AM');
  assert.equal(prague.find((row) => row.service === 'cleanup').server, true);
});

test('what a timer leaves out is said as a word and not a blank', () => {
  const rows = timerRows(timers, { time: makeTime('UTC', () => NOW * 1000), nowMs: NOW * 1000 });
  const byName = Object.fromEntries(rows.map((row) => [row.service, row]));
  assert.equal(byName.logrotate.when, 'Its file does not read');
  assert.equal(byName.logrotate.next, null);
  assert.equal(byName.logrotate.last, null);
  assert.deepEqual(byName.logrotate.result, { words: 'Not known', kind: 'off' });
  assert.deepEqual(byName.cleanup.result, { words: 'Worked', kind: 'ok' });
  assert.deepEqual(byName['backup-sync'].result, { words: 'Failed', kind: 'bad' });
  assert.equal(byName.certs.last, null);
  assert.equal(byName.cleanup.written, 'every 6h, first 10m after boot');
  assert.equal(byName.certs.written, '30 4 * * 1-5, daily');
  assert.equal(timerRows([{ service: 'x', schedule: '0 3 1 6 *', zone: null }], { time: makeTime('UTC'), nowMs: 0 })[0].written, null, 'a schedule shown as written is not said twice');
  assert.match(TIMER_NOTES, /five years/);
});

test('the column headers say which clock the times are on, when any row is not the person\'s', () => {
  const utc = makeTime('UTC', () => NOW * 1000);
  const rows = timerRows(timers, { time: utc, nowMs: NOW * 1000 });
  assert.deepEqual(timerHeaders(rows), ['Task', 'When', 'Next, on the server\'s clock', 'Last, on the server\'s clock', 'Result']);
  assert.deepEqual(timerHeaders(rows.filter((row) => !row.server)), ['Task', 'When', 'Next', 'Last', 'Result']);
  assert.deepEqual(timerHeaders([]), ['Task', 'When', 'Next', 'Last', 'Result']);
});

test('the Timer card of a service says when, next, last and the result', () => {
  const [row] = timerRows(timers.filter((timer) => timer.service === 'cleanup'), { time: makeTime('UTC', () => NOW * 1000), nowMs: NOW * 1000 });
  assert.deepEqual(timerCardRows(row), [
    { label: 'When', value: 'Every 6 hours, the first start comes 10 minutes after the machine starts (every 6h, first 10m after boot)' },
    { label: 'Next', value: '12:00 PM' },
    { label: 'Last', value: '6:00 AM' },
    { label: 'Result', value: 'Worked' },
  ]);
  const [none] = timerRows(timers.filter((timer) => timer.service === 'logrotate'), { time: makeTime('UTC'), nowMs: 0 });
  assert.deepEqual(timerCardRows(none), [
    { label: 'When', value: 'Its file does not read' },
    { label: 'Next', value: '–' },
    { label: 'Last', value: 'Never' },
    { label: 'Result', value: 'Not known' },
  ]);
});
