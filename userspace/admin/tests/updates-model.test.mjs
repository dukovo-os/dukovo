// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The models of the Updates page, with no DOM: each state of the System card,
// the Firmware and Apps cards, a part that could not be checked, the holds,
// the settings form and the arguments it makes, the checks, the history, and
// What happened with the search, from the real documents of the commands.

import assert from 'node:assert/strict';
import test from 'node:test';

import { advisoryRows } from '../pages/updates/advisories.js';
import { addArgs, CONSOLE_CHECK } from '../pages/updates/check-form-model.js';
import {
  beforeWords, canStart, FELL_BACK_WORDS, happenedHeading, happenedOf, leverWords, mostTries, nextWords, searchOf, tryWords,
} from '../pages/updates/happened-model.js';
import { appRows as appHistory, startRows } from '../pages/updates/history-model.js';
import {
  appOffers, appRows, appsHead, atWords, checkedWords, offerOf, checkOf, clockWords, couldNot, firmwareCard, fixesFor, heldRows, holdChoices, keptWords,
  readyCount, runningOf, securityCard, systemCard, whyUnchecked, withdrawnWords, withHolds, worstOf,
} from '../pages/updates/model.js';
import {
  argsOf, channelNote, checksCard, formOf, noteOf, whatCheck,
} from '../pages/updates/settings-model.js';
import {
  advisories, bundles, check, checks, checkWith, failed, found, history, idle, last, NOW, running, search, settings, slots, time,
} from './updates-fixtures.mjs';

const card = (states, more = {}) => systemCard({
  check: checkOf(last(checkWith(states, more.offers ?? {}))), slots: more.slots ?? null, settings: settings(), failed: more.failed?.failed, running: '0.14.0', time,
});

// ---- the last check -------------------------------------------------------------

test('a check is read from what the daily check kept and from a check made now', () => {
  const kept = checkOf(last());
  assert.equal(kept.checkedMs, NOW - 12 * 60_000);
  assert.equal(kept.by, 'updated');
  assert.equal(kept.offers.length, 4);
  assert.equal(kept.withdrawn.version, '0.14.0');
  assert.equal(kept.appFlaws, 2);
  const made = checkOf(check(), NOW);
  assert.equal(made.checkedMs, NOW);
  assert.equal(made.by, null);
  assert.equal(checkOf(null), null, 'no check was kept (exit 3)');
  assert.equal(checkOf({}), null);
  assert.equal(checkOf({ updates: { offers: 'no' } }), null);
});

test('the line under the head says when, and by whom', () => {
  assert.equal(checkedWords(checkOf(last()), time), 'Checked 12 minutes ago by the daily check.');
  assert.equal(checkedWords(checkOf(check(), NOW), time), 'Checked just now.');
  assert.equal(checkedWords(checkOf(last(check(), { by: 'update check' })), time), 'Checked 12 minutes ago by update check.');
  assert.equal(checkedWords(null, time), 'Not checked yet.');
  assert.equal(checkedWords(checkOf(last(check(), { checked_ms: null })), time), 'Checked at a time that was not kept.');
});

test('a time is said as the person reads a clock', () => {
  assert.equal(clockWords('03:00'), '3:00 AM');
  assert.equal(clockWords('00:05'), '12:05 AM');
  assert.equal(clockWords('12:30'), '12:30 PM');
  assert.equal(clockWords('23:59'), '11:59 PM');
  assert.equal(atWords(NOW - 60_000, time), 'today at 3:11 PM');
  assert.equal(atWords(NOW - 86_400_000, time), 'yesterday at 3:12 PM');
  assert.equal(atWords(NOW - 2 * 86_400_000, time), 'on Nov 12 at 3:12 PM');
});

test('the version a server runs is the last word of its release', () => {
  assert.equal(runningOf({ release: 'Dukovo Server 1.0.0-alpha.2' }), '1.0.0-alpha.2');
  assert.equal(runningOf({}), null);
  assert.equal(runningOf(null), null);
});

// ---- the System card, one state at a time -------------------------------------------

test('current: up to date, in the channel\'s words', () => {
  const say = card({ system: 'current' });
  assert.equal(say.state, 'current');
  assert.equal(say.title, 'Up to date');
  assert.deepEqual(say.lines, ['The stable channel offers nothing newer.']);
  assert.deepEqual(say.buttons, []);
  assert.deepEqual(say.pill, { kind: 'ok', word: 'Up to date' });
  assert.equal(say.version, '0.14.0');
});

test('update: what is ready, how much it fetches, what it fixes, and the buttons', () => {
  const say = systemCard({ check: checkOf(last()), slots: null, settings: settings(), running: null, time });
  assert.equal(say.state, 'update');
  assert.equal(say.title, '0.16.0 is ready · 19 MB to download');
  assert.deepEqual(say.lines, ['Fixes DKSA-2026-0001, high: A local user can read another user\'s files', 'It goes through 0.15.1 first.']);
  assert.deepEqual(say.buttons, ['stage', 'hold']);
});

test('staged: the slot and the tries it has left, with restart and take it back', () => {
  const say = card({ system: 'update' }, { slots: slots() });
  assert.equal(say.state, 'staged');
  assert.equal(say.title, '0.16.0 is staged in slot B');
  assert.deepEqual(say.lines, ['It starts at the next restart and has 2 tries to prove itself.']);
  assert.deepEqual(say.buttons, ['restart', 'unstage']);
  assert.equal(card({}, { slots: slots({ staged: { slot: 'b', tries_left: 1 } }) }).lines[0], 'It starts at the next restart and has 1 try to prove itself.');
});

test('waiting: the day of the waves, and why they are paused', () => {
  const say = card({ system: 'waiting' }, { offers: { system: { wave_day: 3, waves_held: 'a disk check' } } });
  assert.equal(say.title, '0.16.0 reaches this machine on day 3 of its waves');
  assert.deepEqual(say.lines, ['Its waves are paused: a disk check.']);
  assert.deepEqual(say.buttons, ['hold']);
  assert.deepEqual(card({ system: 'waiting' }, { offers: { system: { wave_day: 3 } } }).lines, []);
});

test('held after a fallback: when it was taken back and why, with What happened and Release', () => {
  const offers = { system: { held_because: 'fell-back', held_at: new Date(NOW - 3 * 3_600_000).toISOString() } };
  const say = card({ system: 'held' }, { offers, failed: failed() });
  assert.equal(say.state, 'held');
  assert.equal(say.title, '0.16.0 is held');
  assert.deepEqual(say.lines, ['It was taken back today at 12:12 PM: it failed its check (BOOT-103). It is not staged again until you release it.']);
  assert.deepEqual(say.buttons, ['happened', 'release']);
  assert.deepEqual(say.pill, { kind: 'warn', word: 'Held' });
});

test('held because it was asked: no What happened', () => {
  const say = card({ system: 'held' }, { offers: { system: { held_because: 'asked', held_at: null } } });
  assert.deepEqual(say.lines, ['It is held until you release it. A held version gets no security fixes.']);
  assert.deepEqual(say.buttons, ['release']);
});

test('withdrawn: the offer is withdrawn, and nothing can be done about it', () => {
  const say = card({ system: 'withdrawn' }, { offers: { system: { withdrawn: 'it loses files' } } });
  assert.equal(say.title, '0.16.0 was withdrawn: it loses files.');
  assert.deepEqual(say.lines, []);
  assert.equal(card({ system: 'withdrawn' }).title, '0.16.0 was withdrawn.');
  assert.deepEqual(say.buttons, []);
});

test('the release this system runs was withdrawn: the advice, and Go back for a rollback', () => {
  assert.deepEqual(withdrawnWords(checkOf(last())), {
    lines: ['The release this system runs, 0.14.0, was withdrawn: it loses files. Go back to the version before.'],
    buttons: ['goback'],
  });
  const stay = checkOf(last({ ...check(), running_withdrawn: { version: '0.14.0', reason: 'a bad key', advice: 'stay' } }));
  assert.deepEqual(withdrawnWords(stay), { lines: ['The release this system runs, 0.14.0, was withdrawn: a bad key. Stay on it until the next release.'], buttons: [] });
  assert.equal(withdrawnWords(checkOf(last(checkWith()))), null);
});

test('a part that could not be checked never says it is up to date', () => {
  const doc = checkWith({ system: 'current', firmware: 'current' });
  doc.unchecked = [{ item: 'system', why: 'the update server did not answer (no route to updates.dukovo.org)' }, { item: 'firmware', why: '' }];
  const one = checkOf(last(doc));
  const system = systemCard({ check: one, slots: null, settings: settings(), running: '0.14.0', time });
  assert.equal(system.state, 'unchecked');
  assert.equal(system.title, 'Could not check: the update server did not answer (no route to updates.dukovo.org).');
  assert.deepEqual(system.buttons, []);
  assert.doesNotMatch(JSON.stringify(system), /[Uu]p to date/);
  const firmware = firmwareCard({ check: one, slots: null, running: '2026.2' });
  assert.equal(firmware.title, 'Could not check.');
  assert.doesNotMatch(JSON.stringify(firmware), /up to date/);
  assert.equal(whyUnchecked(one, 'system'), 'the update server did not answer (no route to updates.dukovo.org)');
  assert.equal(whyUnchecked(one, 'apps'), null);
  assert.equal(couldNot('no route'), 'Could not check: no route.');
  assert.equal(couldNot(''), 'Could not check.');
});

test('no check kept says "Not checked yet" on each card', () => {
  const none = systemCard({ check: null, slots: null, settings: settings(), running: '0.14.0', time });
  assert.equal(none.state, 'none');
  assert.equal(none.title, 'Not checked yet.');
  assert.equal(none.version, '0.14.0');
  assert.equal(firmwareCard({ check: null, slots: null, running: null }).title, 'Not checked yet.');
  assert.equal(securityCard(null).title, 'Not checked yet.');
  assert.equal(appsHead(null, []).word, 'Not checked yet');
});

test('a staged update is told from the slots even when the check says nothing', () => {
  const say = systemCard({ check: null, slots: slots(), settings: settings(), running: '0.15.0', time });
  assert.equal(say.state, 'staged');
});

test('what is kept for going back, from the slots', () => {
  assert.deepEqual(keptWords(idle()), { version: '0.15.0', text: 'Kept for going back: 0.15.0 in slot A' });
  assert.equal(keptWords(slots()), null);
  assert.equal(keptWords(null), null);
});

// ---- Firmware, Apps, Security ----------------------------------------------------------

test('the Firmware card: up to date, offered, offered while a system update is staged, held', () => {
  const doc = (state) => checkOf(last(checkWith({ firmware: state })));
  assert.equal(firmwareCard({ check: doc('current'), slots: null }).title, 'Firmware set 2026.2 · up to date');
  const offered = firmwareCard({ check: doc('update'), slots: null });
  assert.equal(offered.title, 'Firmware set 2026.2 · 2026.3 is offered');
  assert.deepEqual(offered.buttons, ['stage']);
  assert.equal(offered.reason, null);
  assert.equal(firmwareCard({ check: doc('update'), slots: slots() }).reason, 'After the system update has proved itself');
  const held = firmwareCard({ check: doc('held'), slots: null });
  assert.equal(held.title, 'Firmware set 2026.2 · 2026.3 is held');
  assert.deepEqual(held.buttons, ['release']);
});

test('the Apps card: each update with its kind, what it fixes, and held ones', () => {
  const one = checkOf(last());
  assert.equal(appOffers(one).length, 2);
  const rows = appRows(one, bundles().bundles, advisories().apps);
  assert.deepEqual(rows.map((row) => [row.name, row.pair, row.kind, row.state, row.lines]), [
    ['nginx', '1.2.0 to 1.3.0', 'new version', 'update', ['Fixes 1 security advisory, high']],
    ['notes', '1.2.0 to 1.3.0', 'new version', 'held', ['Held at 1.2.0']],
  ]);
  assert.deepEqual(appsHead(one, rows), { kind: 'warn', word: 'Could not check', lines: ['Could not check: bundled does not answer.'] });
  const clean = checkOf(last(checkWith()));
  assert.deepEqual(appsHead(clean, []), { kind: 'ok', word: 'Up to date', lines: [] });
  assert.deepEqual(appsHead(checkOf(last({ ...checkWith(), offers: check().offers.slice(2, 3), unchecked: [] })), [{ state: 'update' }, { state: 'update' }, { state: 'update' }]), { kind: 'info', word: '3 waiting', lines: [] });
});

test('what an update fixes is told only for a flaw the offered version fixes', () => {
  const flaws = advisories().apps;
  assert.equal(fixesFor('org.nginx.nginx', '1.3.0', flaws), 'Fixes 1 security advisory, high');
  assert.equal(fixesFor('org.nginx.nginx', '1.4.0', flaws), null, 'another version does not fix it');
  assert.equal(fixesFor('org.nginx.nginx', '1.3.0', null), null);
  assert.equal(fixesFor('org.example.notes', '9.9.9', flaws), 'Fixes 1 security advisory, medium');
  const two = [...flaws, { ...flaws[0], id: 'DKSA-2026-0003', severity: 'critical' }];
  assert.equal(fixesFor('org.nginx.nginx', '1.3.0', two), 'Fixes 2 security advisories, critical');
  assert.equal(worstOf([{ severity: 'low' }, { severity: 'unknown' }]), 'low');
  assert.equal(worstOf([]), null);
});

test('the Security card: the advisories that apply, the flaws in apps, and none', () => {
  const say = securityCard(checkOf(last()));
  assert.equal(say.title, '1 advisory applies to this system · 2 flaws in apps');
  assert.deepEqual(say.rows, [{
    id: 'DKSA-2026-0001', severity: 'high', line: 'DKSA-2026-0001, high: A local user can read another user\'s files', fix: '0.16.0 has the fix and is offered.',
  }]);
  assert.equal(securityCard(checkOf(last(checkWith()))).title, 'No advisory applies to this system.');
  const doc = checkWith();
  doc.unchecked = [{ item: 'security', why: 'the list was not read' }];
  assert.equal(securityCard(checkOf(last(doc))).title, 'Could not check: the list was not read.');
});

test('the advisories in full: the system\'s, then each flaw of an app, with the fix each has', () => {
  const rows = advisoryRows(advisories());
  assert.deepEqual(rows.map((row) => [row.id, row.where, row.severity, row.fix]), [
    ['DKSA-2026-0001', 'The system', 'high', '1.4.2 has the fix and is offered.'],
    ['DKSA-2026-0002', 'org.nginx.nginx', 'high', '1.3.0 has the fix and is offered.'],
    ['RUSTSEC-2026-0001', 'org.example.notes', 'medium', '9.9.9 has the fix and is offered.'],
  ]);
  assert.equal(rows[0].action, 'Update to Dukovo 1.4.2 with `update apply`, then restart.');
  assert.equal(rows[1].action, null);
  assert.deepEqual(advisoryRows({ system: [], apps: [{ id: 'X', item: 'a', fixed_by_update: null }] })[0].fix, 'No version of the app has the fix yet.');
  assert.deepEqual(advisoryRows(null), []);
});

// ---- Held ------------------------------------------------------------------------------------

test('what is held: the system with why, the firmware set, and each app held for everyone', () => {
  const held = { version: '0.16.0', why: 'fell-back', at_ms: NOW, start: 42 };
  const rows = heldRows(settings({ hold: { system: '0.16.0', firmware: '2026.3' }, held }), bundles().bundles);
  assert.deepEqual(rows.map((row) => [row.kind, row.item, row.title]), [
    ['system', 'system', 'System 0.16.0'],
    ['firmware', 'firmware', 'Firmware set 2026.3'],
    ['app', 'org.example.notes', 'notes 1.2.0'],
  ]);
  assert.match(rows[0].line, /taken back, so it is not staged again until you release it/);
  assert.equal(rows[1].line, 'It is not staged until you release it.');
  assert.deepEqual(heldRows(settings(), []), []);
  assert.equal(heldRows(settings({ hold: { system: '0.16.0', firmware: null } }), [])[0].line, 'It is not staged until you release it.');
});

test('what can be held: the offered system, the firmware set, and each app that is not held', () => {
  const one = checkOf(last());
  assert.deepEqual(holdChoices(one, settings(), bundles().bundles).map((choice) => [choice.kind, choice.item, choice.label]), [
    ['system', 'system', 'The system, 0.16.0'],
    ['firmware', 'firmware', 'The firmware set, 2026.3'],
    ['app', 'org.nginx.nginx', 'nginx, held at 1.2.0'],
  ]);
  assert.deepEqual(holdChoices(one, settings({ hold: { system: '0.16.0', firmware: '2026.3' } }), []).map((choice) => choice.item), [], 'what is held is not offered again');
});

test('what the navigation counts is the parts with an update ready', () => {
  assert.equal(readyCount(checkOf(last())), 3);
  assert.equal(readyCount(checkOf(last(checkWith({ system: 'current', firmware: 'held' })))), 0);
  assert.equal(readyCount(null), 0);
});

// ---- the settings form --------------------------------------------------------------------

test('the form is the settings in the form\'s own words', () => {
  assert.deepEqual(formOf(settings()), {
    check: 'notify', system: 'manual', restart: '03:00', apps: 'notify', metered: 'wait', channel: 'stable',
  });
  assert.deepEqual(formOf(settings({
    check: 'download', unattended: true, restart: '04:30', apps: 'auto', metered: 'download', channel: 'beta',
  })), {
    check: 'download', system: 'auto', restart: '04:30', apps: 'auto', metered: 'download', channel: 'beta',
  });
  assert.equal(formOf(null).check, 'notify');
});

test('what moved makes the arguments of update set, and nothing that did not', () => {
  const before = formOf(settings());
  assert.deepEqual(argsOf(before, before), { set: null, channel: null, problem: null });
  assert.deepEqual(argsOf(before, { ...before, check: 'off' }), { set: { check: 'off' }, channel: null, problem: null });
  assert.deepEqual(argsOf(before, { ...before, apps: 'auto', metered: 'download' }).set, { apps: 'auto', metered: 'download' });
  assert.deepEqual(argsOf(before, { ...before, channel: 'beta' }), { set: null, channel: 'beta', problem: null });
  // The restart time does not move a system that is told when I say.
  assert.equal(argsOf(before, { ...before, restart: '05:00' }).set, null);
});

test('the system set to update by itself takes its time, and set back clears it', () => {
  const before = formOf(settings());
  const auto = { ...before, system: 'auto', restart: '04:30' };
  assert.deepEqual(argsOf(before, auto).set, { unattended: true, restart: '04:30' });
  assert.deepEqual(argsOf(auto, before).set, { unattended: false, restart: null });
  assert.deepEqual(argsOf(auto, { ...auto, restart: '02:00' }).set, { restart: '02:00' }, 'a new time while it stays so');
  const wrong = argsOf(before, { ...auto, restart: '25:00' });
  assert.equal(wrong.set, null);
  assert.match(wrong.problem, /HH:MM/);
  assert.match(argsOf(before, { ...auto, restart: '' }).problem, /HH:MM/);
});

test('Save and its undo are the same arguments the other way', () => {
  const before = formOf(settings({ check: 'download', channel: 'beta' }));
  const after = { ...before, check: 'off', system: 'auto', restart: '03:00', channel: 'stable' };
  const save = argsOf(before, after);
  assert.deepEqual(save, { set: { check: 'off', unattended: true, restart: '03:00' }, channel: 'stable', problem: null });
  assert.deepEqual(argsOf(after, before), { set: { check: 'download', unattended: false, restart: null }, channel: 'beta', problem: null });
});

test('the notes under the questions', () => {
  const form = formOf(settings());
  assert.equal(noteOf('check', { ...form, check: 'off' }), 'Nobody is told of security fixes. SEC-131 warns after 7 days.');
  assert.equal(noteOf('check', form), null);
  assert.equal(noteOf('system', { ...form, system: 'auto' }), 'Outside 3:00 AM nothing restarts on its own; an update staged by itself waits for that time.');
  assert.equal(noteOf('system', { ...form, system: 'auto', restart: '04:30' }).slice(0, 18), 'Outside 4:30 AM no');
  assert.equal(noteOf('system', form), null);
  assert.match(noteOf('apps', form, { disagree: true }), /Saving makes the two agree/);
  assert.equal(noteOf('apps', form), null);
});

test('moving to an older channel says the system waits and does not go back', () => {
  assert.equal(channelNote('beta', 'stable', '1.0.0-alpha.1'), 'This system does not go back to an older version through a channel: it waits until Stable passes 1.0.0-alpha.1.');
  assert.equal(channelNote('nightly', 'beta', null), 'This system does not go back to an older version through a channel: it waits until Beta passes the version it runs.');
  assert.equal(channelNote('stable', 'beta', '1'), null);
  assert.equal(channelNote('beta', 'beta', '1'), null);
});

// ---- the checks after an update --------------------------------------------------------------

test('the checks card: the services that run again, required and wanted checks, and the time they have', () => {
  const say = checksCard(checks());
  assert.deepEqual(say.essential, ['logd', 'authd', 'consoled', 'netd', 'sshd']);
  assert.deepEqual(say.required, [{ name: 'nginx', line: 'nginx · https://localhost/ answers within 30 s' }]);
  assert.deepEqual(say.wanted.map((one) => one.line), ['postgres · postgres is running within 5 s', 'backup place · runs backup check nas within 60 s']);
  assert.equal(say.seen, 'Seen through after 120 seconds with no restart; an update has 600 seconds to start them all.');
  assert.deepEqual(checksCard(null), {
    essential: [], required: [], wanted: [], seen: null,
  });
  assert.equal(whatCheck({ kind: 'command', command: [] }), 'runs a command');
});

test('a check is added from what the dialog holds, or refused in words', () => {
  const good = {
    name: 'web site', what: 'url', target: 'https://localhost/', required: true, timeout: '',
  };
  assert.deepEqual(addArgs(good), { args: { name: 'web site', url: 'https://localhost/', required: true, timeout: 30 } });
  assert.deepEqual(addArgs({
    name: 'db', what: 'service', target: 'postgres', required: false, timeout: '5',
  }), { args: { name: 'db', service: 'postgres', required: false, timeout: 5 } });
  assert.deepEqual(addArgs({ ...good, insecure: true }).args.insecure, true);
  assert.equal(addArgs({ ...good, what: 'service', target: 'x', insecure: true }).args.insecure, undefined, 'only a web address is insecure');
  for (const [bad, field] of [[{ name: '' }, 'name'], [{ name: '-x' }, 'name'], [{ target: 'ftp://a' }, 'target'], [{ target: 'https://' }, 'target'], [{ timeout: '0' }, 'timeout'], [{ timeout: '601' }, 'timeout'], [{ timeout: '1.5' }, 'timeout']]) {
    const made = addArgs({ ...good, ...bad });
    assert.equal(made.field, field, JSON.stringify(bad));
    assert.ok(made.problem);
  }
  assert.equal(addArgs({ ...good, what: 'service', target: '../x' }).field, 'target');
  const ready = addArgs({ ...CONSOLE_CHECK, timeout: '' });
  assert.deepEqual(ready.args, { name: 'console', url: 'https://localhost:9090/', insecure: true, required: false, timeout: 30 });
});

// ---- the history ---------------------------------------------------------------------------------

test('the history has one row for each start, newest first, with both parts said', () => {
  const rows = startRows(history());
  assert.deepEqual(rows.map((row) => [row.start, row.words, row.outcome, row.error]), [
    [11, 'System 0.14.0 to 0.15.0', 'unfinished', null],
    [10, 'System 0.15.0 to 0.14.0', 'good', null],
    [9, 'System 0.14.0 to 0.15.0; Firmware set 2026.2', 'failed', 'UPD-101'],
  ]);
  assert.equal(rows[0].ms, Date.parse('2026-11-06T21:20:00Z'));
  assert.deepEqual(startRows(null), []);
});

test('what bundled did is said by itself, by an account\'s name, or by its number', () => {
  const rows = appHistory(history(), [{ uid: 1000, name: 'chase' }]);
  assert.deepEqual(rows.map((row) => [row.app, row.what, row.pair, row.who, row.note]), [
    ['org.example.notes', 'Held', '1.2.0', 'chase', null],
    ['org.nginx.nginx', 'Updated', '1.2.0 to 1.3.0', 'by itself', 'from catalog dukovo'],
  ]);
  assert.equal(appHistory(history(), [])[0].who, 'account 1000');
});

// ---- What happened -------------------------------------------------------------------------------

test('the heading says an update was taken back, or when another start failed', () => {
  assert.equal(happenedHeading(failed().failed, time), 'The update to 0.16.0 was taken back');
  assert.equal(happenedHeading({ ...failed().failed, result: 'failed' }, time), 'The start today at 12:12 PM failed');
  assert.equal(happenedHeading({ start: 5, result: 'failed' }, time), 'The start number 5 failed');
});

test('what happened: when, the check, what changed, and what the machine did', () => {
  const lines = ['nginx failed a required check (BOOT-103).', 'Likely cause: its certificate is not readable.'];
  const info = happenedOf(failed().failed, { notice: lines, slots: idle(), time });
  assert.deepEqual(info, {
    when: 'Start 42 today at 12:12 PM, release 0.16.0, slot B',
    check: 'BOOT-103',
    failure: lines,
    since: [],
    did: 'The machine went back to 0.15.0 with today\'s settings.',
  });
  const full = happenedOf({ ...failed().failed, since_good: failed().since_good }, { notice: [], slots: idle({ last_update: { from: '0.15.0', to: '0.16.0', slot: 'b', settings_restored: true } }), time });
  assert.equal(full.did, 'The machine went back to 0.15.0 with the settings from before the update. Nothing was lost.');
  const reader = happenedOf(failed().failed, { notice: undefined, slots: null, time });
  assert.equal(reader.did, null, 'a reader cannot read the slots');
  assert.deepEqual(reader.failure, []);
  const since = failed();
  assert.deepEqual(happenedOf({ ...since.failed, since_good: since.since_good }, { slots: null, time }).since, ['org.nginx.nginx 1.30.6, installed Nov 12 3:12 PM', 'Dukovo 0.16.0']);
});

// ---- the search -------------------------------------------------------------------------------------

test('four changes take at most two restarts', () => {
  assert.deepEqual([0, 1, 2, 3, 4, 5, 8, 9].map(mostTries), [0, 0, 1, 2, 2, 3, 3, 4]);
  assert.match(beforeWords(search().levers), /^Each try is one restart that leaves some of the changes out\. 4 changes take at most 2 restarts\. The machine restarts 60 seconds after each try's result, and you can stop the search between tries\.$/);
  assert.match(beforeWords(search().levers.slice(0, 2)), /2 changes take at most 1 restart\./);
});

test('the changes it can try apart, and when the search can be started', () => {
  assert.deepEqual(search().levers.map(leverWords), ['the system 0.16.0', 'the boot options quiet nowatchdog', 'app org.nginx.nginx 1.30.6', 'app org.python.python 3.14.1']);
  const stopped = { ...failed().failed, result: 'failed' };
  assert.equal(canStart(search(), stopped), true);
  assert.equal(canStart(search(), failed().failed), false, 'not after an update was taken back');
  assert.equal(canStart(search({ levers: search().levers.slice(0, 1) }), stopped), false, 'one change needs no search');
  assert.equal(canStart(running(), stopped), false);
  assert.equal(canStart(found(true), stopped), false);
  assert.equal(canStart(null, stopped), false);
  assert.equal(canStart(search(), null), false);
  assert.match(FELL_BACK_WORDS, /not offered after an update was taken back/);
});

test('each try is said by what it left out and how it ended', () => {
  const tries = running().tries;
  assert.deepEqual(tryWords(tries[0], 2), { try: 1, words: 'Try 1: left out system, boot options', verdict: 'Try 1 of 2 started well: the cause is in the half it left out.' });
  assert.deepEqual(tryWords(tries[1], 2).verdict, 'Try 2 of 2 is waiting for its start.');
  const verdicts = { failed: 'failed as the first start did: the cause is in the half it kept', unfinished: 'did not finish starting', otherwise: 'failed in another way', 'not-started': 'did not start' };
  for (const [verdict, words] of Object.entries(verdicts)) assert.ok(tryWords({ try: 1, left_out: ['a'], verdict }, 3).verdict.includes(words), verdict);
  assert.equal(tryWords({ try: 3, left_out: ['x'], confirm: true, verdict: 'good' }, 3).words, 'Confirming try: left out x');
  assert.equal(tryWords({ try: 1, left_out: [], verdict: 'good' }, null).words, 'Try 1: left nothing out');
  assert.equal(tryWords({ try: 1, left_out: [], verdict: 'good' }, null).verdict, 'Try 1 started well: the cause is in the half it left out.');
});

test('the next try starts in seconds, or within a minute while it is not set ahead', () => {
  assert.equal(nextWords(NOW + 45_000, NOW), 'The next try starts in 45 seconds.');
  assert.equal(nextWords(NOW + 1000, NOW), 'The next try starts in 1 second.');
  assert.equal(nextWords(NOW - 5000, NOW), 'The next try starts now.');
  assert.equal(nextWords(null, NOW), 'The next try starts within a minute.');
});

test('a search that waits says the last try, what it left out, and when the next starts', () => {
  const say = searchOf(running(), NOW);
  assert.equal(say.kind, 'running');
  assert.deepEqual(say.lines, ['Try 2 of 2 is waiting for its start.', 'The next try starts in 45 seconds.']);
  assert.equal(say.tries.length, 2);
  assert.equal(say.result, null);
  const between = searchOf(running({ tries: [running().tries[0]], next_ms: null }), NOW);
  assert.deepEqual(between.lines, ['Try 1 of 2 started well: the cause is in the half it left out.', 'The next try starts within a minute.']);
  assert.equal(searchOf(running({ tries: [] }), NOW).lines[0], 'Try 2 of 2 starts at the next restart.');
});

test('a search with no result and none running says nothing', () => {
  assert.deepEqual(searchOf(search(), NOW), {
    kind: 'idle', lines: [], tries: [], result: null,
  });
  assert.equal(searchOf(null, NOW).kind, 'idle');
});

test('a search that found a change says it, how to undo it, and offers to make sure only when it was by elimination', () => {
  const sure = searchOf(found(true), NOW);
  assert.equal(sure.kind, 'ended');
  assert.deepEqual(sure.result.lines, ['Found: org.nginx.nginx 1.30.6, confirmed']);
  assert.deepEqual(sure.result.undo, ['elevate bundle rollback org.nginx.nginx --for-everyone']);
  assert.equal(sure.result.makeSure, false);
  assert.match(sure.result.left, /^This start is a try of the search\. The next usual start is the failed one again until the fix is taken\.$/);
  const unsure = searchOf(found(false), NOW);
  assert.equal(unsure.result.makeSure, true);
  assert.deepEqual(unsure.result.lines, ['Found: org.nginx.nginx 1.30.6, by elimination']);
  const own = found(false);
  own.result.words = [];
  assert.deepEqual(searchOf(own, NOW).result.lines, ['Found: org.nginx.nginx 1.30.6, by elimination.'], 'in its own words when the command gave none');
});

test('a search that ended without a change says how', () => {
  const ended = (end) => searchOf(search({ result: { end, what: null, version: null, confirmed: false, undo: [], words: [] } }), NOW).result;
  assert.deepEqual(ended('not-found').lines, ['No single change explains the failure.']);
  assert.deepEqual(ended('stopped').lines, ['The search was stopped.']);
  assert.deepEqual(ended('otherwise').lines, ['A try failed in another way, so the search could not judge.']);
  assert.equal(ended('not-found').makeSure, false);
  assert.deepEqual(ended('nonsense').lines, ['The search could not go on.']);
});

test('a hold the check was made before is laid over it, and a hold that is gone is not', () => {
  const doc = checkOf(last(checkWith({ system: 'update', firmware: 'update' })));
  const held = withHolds(doc, settings({ hold: { system: '0.16.0', firmware: null } }));
  assert.equal(offerOf(held, 'system').state, 'held');
  assert.equal(offerOf(held, 'system').held_because, 'asked');
  assert.equal(offerOf(held, 'firmware').state, 'update');
  const older = withHolds(doc, settings({ hold: { system: '0.15.0', firmware: null } }));
  assert.equal(offerOf(older, 'system').state, 'update', 'a hold of another version holds nothing of this one');
  const gone = withHolds(checkOf(last(checkWith({ system: 'held' }, { system: { held_because: 'fell-back' } }))), settings({ hold: { system: null, firmware: null } }));
  assert.equal(offerOf(gone, 'system').state, 'update');
  const unknown = withHolds(checkOf(last(checkWith({ system: 'held' }))), { channel: 'stable' });
  assert.equal(offerOf(unknown, 'system').state, 'held', 'settings that say nothing of holds change nothing');
  assert.equal(withHolds(null, settings()), null);
});
