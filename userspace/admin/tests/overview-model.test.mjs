// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// pages/overview/model.js and change-model.js: the words and lists of the
// Overview's cards, from fixtures of what admind answers.

import assert from 'node:assert/strict';
import test from 'node:test';

import { assess } from '../lib/health.js';
import { makeTime } from '../lib/time.js';
import { byWords, changedHeading, changeRow } from '../pages/overview/change-model.js';
import {
  checkedWords, CHECK_COMMAND, clockWords, hide, healthCard, healthRows, HIDDEN_KEPT,
  lookWords, machineWords, noNumbersWords, normalizeNeed, noticeKey, ROW_ORDER, serverRows, SHOWN, topServices, visibleNotices,
} from '../pages/overview/model.js';
import { needsOf as serviceNeeds } from '../pages/services/needs.js';
import {
  about, changes, checks, metricsNow, NOW, notices, services, utc,
} from './overview-fixtures.mjs';

const time = (zone = 'UTC') => makeTime(zone, () => NOW * 1000);
const need = (id, severity, more = {}) => ({ id, severity, title: `${id} needs you`, detail: '', href: null, icon: 'info', at: null, ...more });

// ---- Health -----------------------------------------------------------------------------

test('a need from a part has what it left out filled in, and one with no title is dropped', () => {
  assert.deepEqual(normalizeNeed({ title: 'Look' }), {
    id: 'Look', severity: 'info', title: 'Look', detail: '', href: null, icon: 'info', at: null, covers: [],
  });
  assert.equal(normalizeNeed({ severity: 'bad' }), null);
  assert.equal(normalizeNeed({ title: '' }), null);
  assert.equal(normalizeNeed(null), null);
  assert.equal(normalizeNeed({ title: 'x', severity: 'dire', at: 'soon', covers: 'SEC-150' }).severity, 'info');
  assert.equal(normalizeNeed({ title: 'x', at: 'soon' }).at, null);
  assert.deepEqual(normalizeNeed({ title: 'x', covers: 'SEC-150' }).covers, []);
});

test('the list is worst first, a need before a check, the newer need first, and the same on every read', () => {
  const findings = assess(checks).findings;
  const rows = healthRows([
    need('old', 'warn', { at: 100 }), need('new', 'warn', { at: 200 }), need('none', 'warn'), need('bad', 'bad', { at: 50 }), need('info', 'info'),
  ], findings);
  assert.deepEqual(rows.map((row) => row.id).slice(0, 5), ['bad', 'new', 'old', 'none', rows[4].id]);
  assert.equal(rows[4].kind, 'check');
  assert.equal(rows[rows.length - 1].id, 'info');
  assert.deepEqual(healthRows([need('a', 'warn'), need('b', 'warn')], []).map((row) => row.id), ['a', 'b']);
  assert.deepEqual(healthRows([need('b', 'warn'), need('a', 'warn')], []).map((row) => row.id), ['a', 'b']);
});

test('a check is a row in its own words, with the fix as its detail', () => {
  const rows = healthRows([], assess(checks).findings);
  assert.equal(rows.length, 3);
  const ssh = rows.find((row) => row.check === 'SEC-110');
  assert.equal(ssh.title, 'Passwords sign in over SSH (sshd_config: PasswordAuthentication yes)');
  assert.equal(ssh.detail, 'Add your SSH key, then set PasswordAuthentication no in /system/settings/ssh/sshd_config.');
  assert.deepEqual([ssh.severity, ssh.kind, ssh.icon, ssh.href], ['warn', 'check', 'security', null]);
  assert.equal(rows.find((row) => row.check === 'SEC-130').icon, 'updates');
  const odd = healthRows([], assess({ results: [{ result: 'warning', area: 'new-area', check: 'a check' }] }).findings);
  assert.deepEqual([odd[0].title, odd[0].icon], ['A check', 'security']);
});

test('a check that a need covers is listed once: SEC-150 and the services that failed', () => {
  const findings = assess(checks).findings;
  assert.equal(findings.filter((one) => one.id === 'SEC-150').length, 1);
  const rows = healthRows(serviceNeeds(services, NOW), findings);
  assert.equal(rows.filter((row) => row.check === 'SEC-150').length, 0);
  assert.deepEqual(rows.map((row) => row.kind), ['need', 'check', 'check']);
  assert.equal(healthRows([], findings).filter((row) => row.check === 'SEC-150').length, 1, 'with no need it is listed');
});

test('the card with a score has its band, the first four things and the number of all of them', () => {
  const card = healthCard({ state: 'ok', assessed: assess(checks) }, [
    need('a', 'bad'), need('b', 'warn'), need('c', 'warn'), need('d', 'info'),
  ]);
  assert.deepEqual([card.score, card.tone, card.line, card.note, card.command], [84, 'warn', 'Could be better', null, null]);
  assert.equal(SHOWN, 4);
  assert.equal(card.count, 7);
  assert.equal(card.shown.length, 4);
  assert.equal(card.more, 7);
  assert.equal(card.rows.length, 7);
});

test('with no more than four things there is no "See all"', () => {
  const card = healthCard({ state: 'ok', assessed: assess({ results: [] }) }, [need('a', 'warn')]);
  assert.deepEqual([card.score, card.tone, card.line, card.count, card.more], [100, 'ok', 'Good', 1, 0]);
  const four = healthCard({ state: 'ok', assessed: assess({ results: [] }) }, [1, 2, 3, 4].map((n) => need(`n${n}`, 'warn')));
  assert.equal(four.more, 0);
});

test('a server with no checks yet says so and lists what the parts need alone', () => {
  const card = healthCard({ state: 'missing' }, [need('a', 'bad'), need('b', 'warn')]);
  assert.deepEqual([card.score, card.band, card.line, card.note, card.tone, card.count, card.command], [null, null, 'Checks are not on this server yet.', 'Checks are not on this server yet.', 'bad', 2, null]);
  assert.equal(healthCard({ state: 'missing' }, [need('a', 'warn')]).tone, 'warn');
  assert.equal(healthCard({ state: 'missing' }, []).tone, 'off');
});

test('a reader is told the score is for administrators, and still sees their own needs', () => {
  const card = healthCard({ state: 'reader' }, [need('a', 'warn')]);
  assert.equal(card.note, 'The health score is for administrators.');
  assert.equal(card.count, 1);
});

test('a result that was never kept says not checked yet and gives the command', () => {
  const card = healthCard({ state: 'none' }, []);
  assert.deepEqual([card.note, card.command, CHECK_COMMAND], ['Not checked yet.', 'elevate dukovo-check', 'elevate dukovo-check']);
  assert.equal(healthCard({ state: 'error' }, []).note, 'The checks could not be read.');
  assert.equal(healthCard({ state: 'other' }, []).note, 'The checks could not be read.');
  assert.equal(healthCard({ state: 'error' }, []).command, null);
});

test('the words for how many things there are', () => {
  assert.equal(lookWords(0), 'Nothing to look at');
  assert.equal(lookWords(1), '1 thing to look at');
  assert.equal(lookWords(4), '4 things to look at');
});

test('when the checks ran, today, yesterday or a day', () => {
  assert.equal(checkedWords(utc(2026, 11, 14, 4, 0), time()), 'Checked today at 4:00 AM');
  assert.equal(checkedWords(utc(2026, 11, 13, 4, 0), time()), 'Checked yesterday at 4:00 AM');
  assert.equal(checkedWords(utc(2026, 11, 2, 4, 0), time()), 'Checked Nov 2 4:00 AM');
  assert.equal(checkedWords(utc(2026, 11, 14, 4, 0), time('America/Denver')), 'Checked yesterday at 9:00 PM');
  assert.equal(checkedWords(null, time()), null);
});

// ---- What changed ----------------------------------------------------------------------------

test('the heading says since the last good start, or the last 7 days', () => {
  assert.deepEqual(changedHeading(about, time(), NOW), { more: 'since the last good start, 6:12 AM', since: about.last_good_start });
  assert.deepEqual(changedHeading({ last_good_start: null }, time(), NOW), { more: 'in the last 7 days', since: NOW - 7 * 86400 });
  assert.deepEqual(changedHeading(null, time(), NOW).more, 'in the last 7 days');
});

test('who made a change is said by how it was made', () => {
  assert.equal(byWords({ who: 'alice', how: 'console' }), 'By alice in this console');
  assert.equal(byWords({ who: 'chase', how: 'token ci' }), 'By chase with the API token ci');
  assert.equal(byWords({ who: 'chase', how: 'client desk of chase' }), 'By chase, asked for by the AI client desk');
  assert.equal(byWords({ who: 'alice', how: 'by hand' }), 'alice edited it by hand');
  assert.equal(byWords({ who: 'outside the console', how: 'by hand' }), 'Someone edited it by hand');
  assert.equal(byWords({ who: 'Spark', how: 'undone' }), 'Spark undid it');
});

test('a hand edit is a row of its own words with a warning dot until it is kept', () => {
  const [hand, console, , kept] = changes.changes;
  assert.deepEqual(changeRow(hand, time()), {
    id: 12, time: '7:42 AM', title: 'blog.conf changed outside this console', sub: 'alice edited it by hand', dot: 'warn', hand: true, unseen: true,
  });
  const keptRow = changeRow(kept, time());
  assert.deepEqual([keptRow.dot, keptRow.unseen, keptRow.sub], ['ok', false, 'alice edited it by hand · kept by chase']);
  const row = changeRow(console, time());
  assert.deepEqual([row.title, row.sub, row.dot, row.hand, row.time], ['Restart a service', 'By chase in this console', 'info', false, 'Yesterday 8:12 AM']);
});

test('a change through the console says how many files it touched and whether it worked', () => {
  const base = { id: 1, at: NOW, who: 'a', how: 'console', action: 'x.y', title: null, note: null, files: [{}, {}] };
  assert.equal(changeRow(base, time()).title, 'x.y', 'the action when there is no title');
  assert.equal(changeRow({ ...base, action: null }, time()).title, 'A change');
  assert.equal(changeRow(base, time()).sub, 'By a in this console · 2 files');
  assert.equal(changeRow({ ...base, files: [{}] }, time()).sub, 'By a in this console · 1 file');
  const failed = changeRow({ ...base, status: 1, note: 'the command ended with status 1' }, time());
  assert.deepEqual([failed.dot, failed.sub], ['bad', 'By a in this console · 2 files · it did not work · the command ended with status 1']);
  assert.equal(changeRow({ ...base, status: 0, files: undefined }, time()).sub, 'By a in this console');
  const undone = { ...base, who: 'Spark', how: 'undone', action: null, note: 'it was not kept in time', files: [{ path: '/system/settings/network' }] };
  assert.deepEqual(
    [changeRow(undone, time()).title, changeRow(undone, time()).sub, changeRow(undone, time()).dot],
    ['network put back: it was being tried', 'Spark undid it · 1 file · it was not kept in time', 'info'],
  );
});

// ---- This server ---------------------------------------------------------------------------

const context = (zone = 'UTC') => ({ time: time(zone), serverTime: time(about.zone), now: NOW });

test('the machine as one line, whatever it says', () => {
  assert.equal(machineWords(about), 'Dell Inc. OptiPlex 7040 · Intel Core i5-6500 · 16 GB');
  assert.equal(machineWords({ machine: { maker: null, model: null }, processor: null, memory: null }), null);
  assert.equal(machineWords({ machine: { maker: '', model: 'X' }, processor: 'P', memory: 0 }), 'X · P');
  assert.equal(machineWords({}), null);
});

test('the time row says the zone, the clock there and whether it is in sync', () => {
  assert.equal(clockWords(about, time('UTC'), NOW), 'UTC · 10:12 AM there · in sync with a time source');
  assert.equal(clockWords({ zone: 'Zone/Unknown', clock: { synchronized: false } }, null, NOW), 'Zone/Unknown · not in sync with a time source');
});

test('the rows of This server are in the order of the board, with what a machine does not say left out', () => {
  const rows = serverRows(about, context());
  assert.deepEqual(rows.map((row) => row.label), ['Name', 'System', 'Machine', 'Up for', 'Time', 'Last sign-in', 'SSH host key']);
  const by = Object.fromEntries(rows.map((row) => [row.label, row]));
  assert.equal(by.Name.value, 'duke-nas');
  assert.equal(by.System.value, 'Dukovo Server 1.0.0-alpha.1');
  assert.equal(by['Up for'].value, '4 h 52 min');
  assert.equal(by['Last sign-in'].value, '9:22 AM from 192.168.1.25 (console)');
  assert.equal(by['Last sign-in'].more, 'Before this: Yesterday 6:12 AM from 10.8.0.2 (pts/0)');
  assert.equal(by['SSH host key'].mono, true);
  const bare = serverRows({ host: 'x', zone: '', sign_ins: null, up: null, release: null, machine: {} }, context());
  assert.deepEqual(bare.map((row) => row.label), ['Name']);
});

test('a start that fell back to the other version says so, and a sign-in at the computer says that', () => {
  const rows = serverRows({ ...about, fallback: true, sign_ins: [{ at: NOW, terminal: '', from: null }] }, context());
  assert.equal(rows.find((row) => row.label === 'System').value, 'Dukovo Server 1.0.0-alpha.1 (the earlier version: the new one did not start)');
  const last = rows.find((row) => row.label === 'Last sign-in');
  assert.equal(last.value, '10:12 AM at the computer');
  assert.equal(last.more, undefined);
});

test('a part adds a row where its order puts it, and one with no order goes last', () => {
  const rows = serverRows(about, context(), [
    { label: 'Profile', value: 'Balanced', order: 55 },
    { label: 'Protected', value: 'Snapshots hourly', order: 120 },
    { label: 'Watched by', value: 'duke-pi' },
    { label: 'No words' },
    null,
  ]);
  assert.deepEqual(rows.map((row) => row.label), ['Name', 'System', 'Machine', 'Up for', 'Time', 'Profile', 'Last sign-in', 'SSH host key', 'Protected', 'Watched by']);
  assert.equal(ROW_ORDER.signIn, 100);
});

// ---- Using the most memory -------------------------------------------------------------------

test('the five services with the most memory, their CPU and what they are', () => {
  const found = topServices(metricsNow, services);
  assert.deepEqual(found.map((one) => [one.name, one.memory, one.cpu, one.description]), [
    ['postgres', '1.9 GB', '3.1%', 'PostgreSQL database'],
    ['nginx', '210 MB', '0.4%', 'Web server'],
    ['php-fpm', '184 MB', '1.2%', 'PHP pages for photos.example.org'],
    ['valkey', '96 MB', '0.2%', 'Valkey cache'],
    ['admind', '22 MB', '0.1%', 'This console'],
  ]);
});

test('a service that holds no memory is left out, and one with no CPU reading has a dash', () => {
  assert.equal(topServices(metricsNow, services).some((one) => one.name === 'backup-sync'), false);
  const found = topServices({ 'service.memory{a}': 10, 'service.memory{b}': 20, 'service.cpu{a}': 1 }, null, 1);
  assert.deepEqual(found.map((one) => [one.name, one.cpu, one.description]), [['b', '–', '']]);
  assert.equal(topServices({ 'service.memory{a}': 10, 'service.cpu{a}': 1 }, null)[0].cpu, '1%');
  for (const bad of [null, undefined, {}, { cpu: 3 }, { 'service.memory{a}': 'x' }]) assert.deepEqual(topServices(bad, services), []);
  assert.deepEqual(topServices({ 'service.memory{a}': 5, 'service.memory{b}': 5 }, null).map((one) => one.name), ['a', 'b'], 'equal ones by name');
});

// ---- Notices ----------------------------------------------------------------------------------

test('a notice is shown with its kind, and hidden by its name and its words', () => {
  const [first] = visibleNotices(notices.notices, []);
  assert.deepEqual([first.name, first.kind, first.key.startsWith('failed-start:')], ['failed-start', 'warn', true]);
  assert.equal(visibleNotices([{ name: 'other', lines: ['x'], at: null }], [])[0].kind, 'info');
  assert.deepEqual(visibleNotices(notices.notices, [first.key]), []);
  const reworded = [{ ...notices.notices[0], lines: ['Other words'] }];
  assert.equal(visibleNotices(reworded, [first.key]).length, 1, 'the same notice with other words comes back');
  assert.notEqual(noticeKey(reworded[0]), first.key);
});

test('a notice that is not one is left out', () => {
  for (const bad of [null, undefined, {}, { notices: [] }, 'x']) assert.deepEqual(visibleNotices(bad, []), []);
  assert.deepEqual(visibleNotices([{ name: 'a', lines: [] }, { name: 3, lines: ['x'] }, null, { name: 'b' }], []), []);
});

test('hiding a notice keeps it once, and forgets the oldest past a hundred', () => {
  assert.deepEqual(hide(['a'], 'b'), ['a', 'b']);
  assert.deepEqual(hide(['a', 'b'], 'a'), ['b', 'a']);
  const many = Array.from({ length: HIDDEN_KEPT }, (_, index) => `k${index}`);
  const next = hide(many, 'new');
  assert.equal(next.length, HIDDEN_KEPT);
  assert.deepEqual([next[0], next[HIDDEN_KEPT - 1]], ['k1', 'new']);
});

// ---- Usage ----------------------------------------------------------------------------------------

test('with no numbers the card says whether the program that writes them is running', () => {
  const show = (state) => ({ services: [{ name: 'metricsd', state }] });
  assert.equal(noNumbersWords(show('failed')), 'No numbers yet: metricsd is not running (failed).');
  assert.equal(noNumbersWords(show('stopped')), 'No numbers yet: metricsd is not running (stopped).');
  assert.equal(noNumbersWords(show('running')), 'No numbers yet. The first ones come within 10 minutes of metricsd starting.');
  assert.equal(noNumbersWords({ services: [] }), 'No numbers yet. The first ones come within 10 minutes of metricsd starting.');
  assert.equal(noNumbersWords(null), 'No numbers yet. The first ones come within 10 minutes of metricsd starting.');
});

test('the line the phone shows under the charts is how long the machine has been up and what it is', async () => {
  const { upWords, SEVERITY_WORDS } = await import('../pages/overview/model.js');
  assert.equal(upWords(about), 'Up for 4 h 52 min · Dell Inc. OptiPlex 7040');
  assert.equal(upWords({ up: 90, machine: {} }), 'Up for 2 min');
  assert.equal(upWords({ machine: { maker: 'A', model: null } }), 'A');
  assert.equal(upWords({}), null);
  assert.equal(upWords(null), null);
  assert.deepEqual(SEVERITY_WORDS, { bad: 'Needs you: ', warn: 'Worth a look: ', info: '' });
});
