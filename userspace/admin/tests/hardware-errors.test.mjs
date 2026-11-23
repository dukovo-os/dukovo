// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Hardware errors card: what its model says from `hardware errors --json`
// (the newest ten first, the memory devices that have errors, the one calm line,
// and the firmware's errors from before this start) and what the page draws.

import assert from 'node:assert/strict';
import test, { afterEach, beforeEach } from 'node:test';

import {
  BEFORE, CAPTION, deviceRows, errorGroups, errorRows, foundBy, hiddenCount, MOST, NO_ERRORS, NOT_READ, severityOf, summaryOf, whenWords,
} from '../pages/hardware/errors-model.js';
import { about, NOW, time } from './hardware-fixtures.mjs';
import { error, noErrors, someErrors } from './hardware-errors-fixtures.mjs';
import { openHardware, refusal } from './hardware-view.mjs';

const realObserver = globalThis.MutationObserver;
beforeEach(() => {
  globalThis.MutationObserver = class { observe() {} disconnect() {} };
});
afterEach(() => {
  if (realObserver === undefined) delete globalThis.MutationObserver;
  else globalThis.MutationObserver = realObserver;
});

const text = (root) => root.textContent.replace(/\s+/g, ' ');
const dataRows = (card) => card.querySelectorAll('tbody tr').filter((one) => !one.classes.has('hw-grp'));
const ROUTE = '/api/1/hardware/errors';
/** `count` corrected errors of one device, numbered from 1. */
const many = (count) => {
  const answer = noErrors();
  answer.errors = Array.from({ length: count }, (_, at) => error({ seq: at + 1, seconds: 100 + at }));
  answer.counts.recorded = count;
  answer.counts.corrected = count;
  return answer;
};

test('a severity has its word and a tone of the state classes, and an unknown one is said as it is', () => {
  assert.deepEqual(severityOf('fatal'), { word: 'Fatal', tone: 'bad' });
  assert.deepEqual(severityOf('recoverable'), { word: 'Not corrected', tone: 'bad' });
  assert.deepEqual(severityOf('corrected'), { word: 'Corrected', tone: 'warn' });
  assert.deepEqual(severityOf('informational'), { word: 'Informational', tone: 'off' });
  assert.deepEqual(severityOf('dire'), { word: 'dire', tone: 'off' });
  assert.deepEqual(severityOf(undefined), { word: 'Unknown', tone: 'off' });
});

test('a time is on the clock when the machine\'s uptime is known, and else how long after the start', () => {
  const up = 4 * 3600 + 52 * 60;
  const now = Math.floor(NOW / 1000);
  assert.equal(whenWords(error({ seconds: 7300 }), { up, now, time }), time.when(now - up + 7300));
  assert.equal(whenWords(error({ seconds: 40 })), 'In the first minute after the start');
  assert.equal(whenWords(error({ seconds: 3700 })), '1 hour 1 minute after the start');
  assert.equal(whenWords(error({ seconds: 7300 }), { up: null, now, time }), '2 hours 1 minute after the start');
  assert.equal(whenWords(error({ seconds: -1 })), 'At an unknown time');
  assert.equal(whenWords(error({ seconds: 'x' })), 'At an unknown time');
});

test('an error of the firmware\'s record has no time of its own: its cell is blank unless the answer gives a date', () => {
  const bert = error({ source: 'bert', seconds: 12.345 });
  assert.equal(whenWords(bert), '');
  assert.equal(whenWords(bert, { up: 100, now: 1000, time }), '');
  assert.equal(whenWords({ ...bert, at: 1_794_000_000 }, { time }), time.when(1_794_000_000));
});

test('an error is found by the firmware for ghes and bert, by the kernel for mce and aer, and any other source is shown as it is', () => {
  for (const source of ['ghes0', 'ghes3', 'ghes12', 'bert']) assert.equal(foundBy(source), 'The firmware', source);
  for (const source of ['mce', 'aer']) assert.equal(foundBy(source), 'The kernel', source);
  assert.equal(foundBy('edac'), 'edac');
  assert.equal(foundBy(undefined), 'Unknown');
});

test('the rows are grouped, the firmware\'s record from before this start first and the rest newest first', () => {
  const context = { up: 17_520, now: Math.floor(NOW / 1000), time };
  const groups = errorGroups(someErrors(), context);
  assert.deepEqual(groups.map((group) => [group.key, group.title, group.rows.map((row) => row.key)]), [
    ['before', BEFORE, ['1']],
    ['since', 'Since this start, 4 hours 52 minutes ago', ['4', '3', '2']],
  ]);
  assert.deepEqual(groups[1].rows[0], {
    key: '4', when: time.when(Math.floor(NOW / 1000) - 17_520 + 7300), before: false, part: '0000:00:1c.0', what: 'bad TLP', count: 1, severity: { word: 'Not corrected', tone: 'bad' }, found: 'The kernel',
  });
  assert.equal(groups[1].rows[1].count, 4);
  assert.deepEqual([groups[0].rows[0].when, groups[0].rows[0].found, groups[0].rows[0].severity.word], ['', 'The firmware', 'Fatal']);
  assert.equal(groups[1].rows[2].found, 'The firmware');
  assert.deepEqual(errorRows(someErrors(), context).map((row) => row.key), ['1', '4', '3', '2']);
});

test('a group with no row is left out, and the title has no time when the uptime is not known', () => {
  assert.deepEqual(errorGroups(many(2)).map((group) => [group.key, group.title]), [['since', 'Since this start']]);
  const only = noErrors();
  only.errors = [error({ source: 'bert', severity: 'fatal' })];
  assert.deepEqual(errorGroups(only).map((group) => group.key), ['before']);
  assert.deepEqual(errorGroups(noErrors()), []);
});

test('a part with no name is said so, and a count that is not a number is one', () => {
  const [row] = errorRows({ errors: [error({ part: '', count: 'x', what: undefined })], counts: {} });
  assert.equal(row.part, 'Not named');
  assert.equal(row.count, 1);
  assert.equal(row.what, '');
});

test('at most ten rows are listed, the newest, unless all are asked for, and the rest are counted', () => {
  const answer = many(14);
  const rows = errorRows(answer);
  assert.equal(rows.length, MOST);
  assert.deepEqual([rows[0].key, rows.at(-1).key], ['14', '5']);
  assert.equal(errorRows(answer, {}, true).length, 14);
  assert.equal(hiddenCount(answer), 4);
  assert.equal(hiddenCount(many(10)), 0);
  assert.equal(hiddenCount(null), 0);
});

test('an answer that is not a document has no rows and no devices', () => {
  for (const none of [null, undefined, {}, [], 'no', { errors: 'x' }]) {
    assert.deepEqual(errorRows(none), []);
    assert.deepEqual(deviceRows(none), []);
  }
});

test('the memory devices that have errors are named by their place or their handle, with their size and their counts', () => {
  assert.deepEqual(deviceRows(someErrors()), [
    {
      key: '4352', name: 'DIMM_A1', size: '16 GiB', words: '5 corrected',
    },
    {
      key: '4354', name: 'Memory device 0x1102', size: null, words: '2 not corrected, 1 corrected',
    },
  ]);
  assert.deepEqual(deviceRows(noErrors()), []);
});

test('the devices the tables do not list come last as one', () => {
  const answer = someErrors();
  answer.counts.other_dimms = { corrected: 3, uncorrected: 0 };
  const rows = deviceRows(answer);
  assert.deepEqual(rows.at(-1), {
    key: 'other', name: 'Other memory devices', size: null, words: '3 corrected',
  });
});

test('a machine with no error has the one calm line and no pill', () => {
  assert.deepEqual(summaryOf(noErrors()), { calm: true, lead: NO_ERRORS, pill: null });
  assert.equal(NO_ERRORS, 'No hardware errors since this boot.');
});

test('the lead says the worst of this start and how many came before it', () => {
  const summary = summaryOf(someErrors());
  assert.equal(summary.calm, false);
  assert.equal(summary.lead, 'Not corrected, and 1 fatal before this start');
  assert.deepEqual(summary.pill, { word: 'Not corrected', tone: 'bad' });
  const mild = someErrors();
  mild.errors = mild.errors.filter((one) => one.severity === 'corrected' || one.source === 'bert');
  mild.counts.uncorrected = 0;
  assert.equal(summaryOf(mild).lead, 'Corrected, and 1 fatal before this start');
});

test('the lead has the worst word alone, or the firmware\'s errors alone, and counts the ones that were not corrected', () => {
  assert.equal(summaryOf(many(3)).lead, 'Corrected');
  assert.deepEqual(summaryOf(many(3)).pill, { word: 'Corrected', tone: 'warn' });
  const earlier = noErrors();
  earlier.errors = [error({ seq: 1, source: 'bert', severity: 'fatal' }), error({ seq: 2, source: 'bert', severity: 'recoverable', count: 2 })];
  earlier.counts.recorded = 2;
  assert.equal(summaryOf(earlier).lead, '1 fatal and 2 not corrected before this start');
});

test('an answer that cannot be read says so and has no pill', () => {
  assert.deepEqual(summaryOf(undefined), { calm: false, lead: NOT_READ, pill: null });
  assert.equal(NOT_READ, 'The hardware errors could not be read.');
});

// ---- The card ---------------------------------------------------------------

test('a machine with no error has the card with its one line, no table and no pill, and only the button that saves', async (context) => {
  const world = await openHardware(context);
  const card = world.card('errors');
  assert.ok(card, 'the Hardware errors card is on the page');
  assert.equal(text(card.querySelector('h2')), 'Hardware errors');
  assert.equal(text(card.querySelector('.hw-body')), NO_ERRORS);
  assert.equal(card.querySelector('table'), null);
  assert.equal(card.querySelector('.pill'), null);
  assert.deepEqual(card.querySelectorAll('button').filter((one) => !one.hidden).map(text), ['Save as a file']);
  assert.ok(world.calls.includes(ROUTE));
  assert.equal(world.problems.length, 0);
});

test('the card is the third, after the last check, and not across both columns', async (context) => {
  const world = await openHardware(context);
  const cards = world.view.main.querySelector('.hw').children.filter((one) => one.tag === 'section');
  assert.deepEqual(cards.slice(0, 3).map((one) => one.id), ['machine', 'check', 'errors']);
  assert.ok(!world.card('errors').classes.has('hw-wide'));
});

test('a machine with errors has a visible caption, group rows, the five columns and the devices that have errors', async (context) => {
  const world = await openHardware(context, { world: { reads: { [ROUTE]: someErrors(), '/api/1/console/about': about() } } });
  const card = world.card('errors');
  assert.equal(text(card.querySelector('caption')), CAPTION);
  assert.ok(!card.querySelector('caption').classes.has('sr-only'));
  assert.deepEqual(card.querySelectorAll('thead th').map(text), ['When', 'Part', 'What happened', 'Severity', 'Found by']);
  const groups = card.querySelectorAll('tbody');
  assert.deepEqual(groups.map((one) => one.getAttribute('data-group')), ['before', 'since']);
  const headers = card.querySelectorAll('tr').filter((one) => one.classes.has('hw-grp'));
  assert.deepEqual(headers.map(text), [BEFORE, 'Since this start, 4 hours 52 minutes ago']);
  assert.deepEqual(headers.map((one) => one.querySelector('th').getAttribute('colspan')), ['5', '5']);
  const cells = (row) => row.querySelectorAll('td').map(text);
  const before = groups[0].querySelectorAll('tr').filter((one) => !one.classes.has('hw-grp'));
  assert.deepEqual(cells(before[0]), ['', 'PCIe 0000:3b:00.0', 'PCIe error: device 0000:3b:00.0 (8086:1533), port root port', 'Fatal', 'The firmware']);
  const since = groups[1].querySelectorAll('tr').filter((one) => !one.classes.has('hw-grp'));
  assert.deepEqual(cells(since[0]), [time.when(Math.floor(NOW / 1000) - 17_520 + 7300), '0000:00:1c.0', 'bad TLP', 'Not corrected', 'The kernel']);
  assert.deepEqual(cells(since[1]).slice(1), ['CPU 3 bank 7 channel 1', 'corrected error4 times', 'Corrected', 'The kernel']);
  assert.deepEqual(cells(since[2]).slice(3), ['Corrected', 'The firmware']);
  const states = [...before, ...since].map((row) => [...row.querySelectorAll('td')[3].querySelector('span').classes].join(' '));
  assert.deepEqual(states, ['st bad', 'st bad', 'st warn', 'st warn']);
  assert.equal(since[0].querySelectorAll('td')[0].getAttribute('data-label'), 'When');
  const body = text(card);
  assert.match(body, /^Hardware errorsNot corrected.*Not corrected, and 1 fatal before this start/);
  assert.match(body, /Memory devices with errorsDIMM_A116 GiB5 correctedMemory device 0x11022 not corrected, 1 corrected$/);
  assert.equal(card.querySelector('.pill').textContent, 'Not corrected');
  assert.ok(!/DIMM_B1/.test(body), 'a device with none is not listed');
  assert.deepEqual(card.querySelectorAll('button').filter((one) => !one.hidden).map(text), ['Save as a file']);
});

test('Show all lists every error the record keeps in place and then reads Show the last ten', async (context) => {
  const world = await openHardware(context, { world: { reads: { [ROUTE]: many(14) } } });
  const card = world.card('errors');
  const rows = () => card.querySelectorAll('tbody tr').filter((one) => !one.classes.has('hw-grp')).length;
  assert.equal(rows(), 10);
  assert.equal(text(card.querySelector('caption')), CAPTION);
  const toggle = card.querySelectorAll('button').find((one) => text(one) === 'Show all 14');
  assert.ok(toggle, 'Show all 14');
  toggle.click();
  assert.equal(rows(), 14);
  assert.equal(text(toggle), 'Show the last ten');
  assert.match(text(card.querySelector('caption')), /^All 14 hardware errors/);
  toggle.click();
  assert.equal(rows(), 10);
  assert.equal(text(toggle), 'Show all 14');
});

test('Save as a file keeps the answer as hardware-errors.json with the console\'s own helper', async (context) => {
  const kept = [];
  const realBlob = globalThis.Blob;
  const realUrl = globalThis.URL;
  const links = [];
  context.mock.method(globalThis, 'setTimeout', () => 0);
  globalThis.Blob = class { constructor(parts) { kept.push(parts.join('')); } };
  globalThis.URL = Object.assign(function URLLike() {}, realUrl, { createObjectURL: () => 'blob:x', revokeObjectURL: () => {} });
  context.after(() => { globalThis.Blob = realBlob; globalThis.URL = realUrl; });
  const world = await openHardware(context, { world: { reads: { [ROUTE]: someErrors() } } });
  const realBody = globalThis.document?.body;
  const page = world.card('errors');
  const save = page.querySelectorAll('button').find((one) => text(one) === 'Save as a file');
  const body = globalThis.document.body;
  const appended = [];
  context.mock.method(body, 'append', (link) => { appended.push(link); link.click = () => links.push(link.getAttribute('download')); });
  save.click();
  assert.deepEqual(links, ['hardware-errors.json']);
  assert.deepEqual(JSON.parse(kept[0]), someErrors());
  assert.ok(realBody === body);
});

test('errors that cannot be read say so, once, and the rest of the page is drawn', async (context) => {
  const world = await openHardware(context, { world: { reads: { [ROUTE]: refusal('ADM-129', 'not installed', 501) } } });
  assert.equal(text(world.card('errors').querySelector('.hw-body')), NOT_READ);
  assert.equal(world.problems.length, 1);
  assert.ok(world.card('machine'));
});

test('a reader sees the card too, and its only button saves a file', async (context) => {
  const world = await openHardware(context, { session: { admin: false }, world: { reads: { [ROUTE]: someErrors() } } });
  assert.equal(dataRows(world.card('errors')).length, 4);
  assert.deepEqual(world.card('errors').querySelectorAll('button').filter((one) => !one.hidden).map(text), ['Save as a file']);
});

test('the page reads the errors again every minute, and the newest answer is drawn', async (context) => {
  const answers = [noErrors(), someErrors()];
  let reads = 0;
  const world = await openHardware(context, { world: { reads: { [ROUTE]: () => answers[Math.min(reads++, 1)] } } });
  assert.equal(text(world.card('errors').querySelector('.hw-body')), NO_ERRORS);
  await world.tick(60_000);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(dataRows(world.card('errors')).length, 4);
});
