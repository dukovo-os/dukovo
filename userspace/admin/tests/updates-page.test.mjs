// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Updates page on a fake document: what each card says for an
// administrator, a locked one and a reader, which action each button runs,
// and what the page does with a task, a restart and a failed start.

import assert from 'node:assert/strict';
import test, { afterEach, beforeEach, mock } from 'node:test';

import { ApiError } from '../api.js';
import {
  bundles, checkWith, failed, found, last, NOW, running, search, settings, slots,
} from './updates-fixtures.mjs';
import {
  buttonOf, openUpdates, refusal, settle,
} from './updates-view.mjs';

const realObserver = globalThis.MutationObserver;
beforeEach(() => {
  globalThis.MutationObserver = class { observe() {} disconnect() {} };
});
afterEach(() => {
  mock.timers.reset();
  if (realObserver === undefined) delete globalThis.MutationObserver;
  else globalThis.MutationObserver = realObserver;
});

const text = (root) => root.textContent.replace(/\s+/g, ' ');

test('an administrator sees each card, and a read for each thing the page says', async (context) => {
  const world = await openUpdates(context);
  assert.equal(world.view.headed.title, 'Updates');
  assert.equal(world.view.headed.commands, true);
  assert.equal(text(world.view.main).includes('Checked 12 minutes ago by the daily check.'), true);
  assert.match(text(world.view.main), /Each update of the system starts as a try/);
  for (const id of ['system', 'apps', 'security', 'firmware', 'held', 'settings', 'checks', 'history']) assert.ok(world.card(id), id);
  assert.match(text(world.card('system')), /System · 0\.14\.0.*0\.16\.0 is ready · 19 MB to download/);
  assert.ok(buttonOf(world.card('system'), 'Stage it'));
  assert.ok(buttonOf(world.card('system'), 'Hold this version'));
  assert.match(text(world.card('firmware')), /Firmware set 2026\.2 · 2026\.3 is offered/);
  assert.match(text(world.card('apps')), /nginx 1\.2\.0 to 1\.3\.0 new version/);
  assert.match(text(world.card('apps')), /Fixes 1 security advisory, high/);
  assert.ok(buttonOf(world.card('apps'), 'Update'));
  assert.match(text(world.card('security')), /1 advisory applies to this system · 2 flaws in apps/);
  assert.match(text(world.card('held')), /notes 1\.2\.0/);
  assert.match(text(world.card('checks')), /logd, authd, consoled, netd, sshd/);
  for (const path of ['/api/1/updates', '/api/1/updates/settings', '/api/1/updates/checks', '/api/1/updates/failed', '/api/1/apps', '/api/1/updates/slots']) assert.ok(world.calls.includes(path), path);
  assert.equal(world.problems.length, 0);
});

test('the sections come in the order a phone reads them', async (context) => {
  const world = await openUpdates(context);
  const order = world.view.main.querySelectorAll('section').map((one) => one.id);
  assert.deepEqual(order, ['system', 'apps', 'security', 'firmware', 'held', 'settings', 'checks', 'history']);
});

test('the release that runs was withdrawn: its advice, and Go back for a rollback', async (context) => {
  const world = await openUpdates(context);
  assert.match(text(world.card('system')), /The release this system runs, 0\.14\.0, was withdrawn: it loses files\. Go back to the version before\./);
  assert.ok(buttonOf(world.card('system'), 'Go back…'));
});

test('a reader reads every card and finds no button but Check now and Show the advisories', async (context) => {
  const world = await openUpdates(context, { session: { admin: false } });
  assert.match(text(world.view.main), /An administrator can stage updates, hold versions and change these settings\./);
  const names = world.view.main.querySelectorAll('button').map((one) => one.textContent.trim());
  assert.deepEqual(names.sort(), ['Show the advisories', 'Show the history']);
  assert.deepEqual(world.view.headed.buttons.map((one) => one.textContent), ['Check now']);
  assert.match(text(world.card('system')), /An administrator can go back to the version before\./);
  assert.match(text(world.card('settings')), /Look for updates.*Every day, and tell me/);
  assert.equal(world.calls.includes('/api/1/updates/slots'), false, 'a reader does not read the slots');
});

test('a locked administrator does not read the slots, and can ask to see them', async (context) => {
  const world = await openUpdates(context, { unlocked: false });
  assert.equal(world.calls.includes('/api/1/updates/slots'), false);
  assert.match(text(world.card('system')), /Unlock to see the version kept for going back\./);
  assert.ok(buttonOf(world.card('system'), 'Show the slots'));
  assert.ok(buttonOf(world.card('system'), 'Stage it'), 'a change asks to unlock when it is pressed');
});

test('no check kept says so, and every part says it was not checked, never that it is up to date', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': null } } });
  assert.match(text(world.view.main), /Not checked yet\./);
  assert.doesNotMatch(text(world.view.main), /[Uu]p to date/);
  assert.equal(buttonOf(world.card('system'), 'Stage it'), undefined);
});

test('a part that could not be checked says why on its card', async (context) => {
  const doc = checkWith({ system: 'current', firmware: 'current' });
  doc.unchecked = [{ item: 'system', why: 'the update server did not answer (no route to updates.dukovo.org)' }];
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': last(doc) } } });
  assert.match(text(world.card('system')), /Could not check: the update server did not answer \(no route to updates\.dukovo\.org\)\./);
  assert.doesNotMatch(text(world.card('system')), /Up to date/);
  assert.match(text(world.card('firmware')), /up to date/, 'the part that was checked says it');
});

test('Check now asks the update server, and says when it did', async (context) => {
  const answered = { ...checkWith({ system: 'current', firmware: 'current' }) };
  const world = await openUpdates(context, { world: { answers: { 'updates.check': { ok: true, result: answered } } } });
  await world.press({ querySelectorAll: () => world.view.headed.buttons, textContent: '' }, 'Check now');
  assert.deepEqual(world.ran('updates.check'), [{}]);
  assert.match(text(world.view.main), /Checked just now\./);
  assert.match(text(world.card('system')), /Up to date.*The stable channel offers nothing newer\./);
});

test('stage it runs the action for the system, and its task shows the step on the card until it ends', async (context) => {
  const world = await openUpdates(context, { world: { answers: { 'updates.apply': { ok: true, task: 7, result: { task: 7 } } } } });
  await world.press(world.card('system'), 'Stage it');
  assert.deepEqual(world.ran('updates.apply'), [{ item: 'system' }]);
  await world.emit('task', {
    id: 7, action: 'updates.apply', state: 'running', step: 2, steps: 4, words: 'fetching', percent: 41,
  });
  assert.match(text(world.card('system')), /Step 2 of 4: fetching, 41%/);
  assert.doesNotMatch(text(world.card('firmware')), /Step 2 of 4/);
  world.answers['/api/1/updates/slots'] = slots();
  await world.emit('task', { id: 7, action: 'updates.apply', state: 'done' });
  assert.doesNotMatch(text(world.card('system')), /Step 2 of 4/);
  assert.match(text(world.card('system')), /0\.16\.0 is staged in slot B/);
  assert.match(text(world.view.main), /Restart to finish the update/);
});

test('a staged update offers to restart now, at a time, or to take it back', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates/slots': slots() } } });
  const card = world.card('system');
  assert.match(text(card), /0\.16\.0 is staged in slot B.*It starts at the next restart and has 2 tries to prove itself\./);
  assert.ok(buttonOf(card, 'Restart now…'));
  assert.ok(buttonOf(card, 'Take it back'));
  assert.ok(buttonOf(card, 'Restart at that time'), 'without a restart time set, a time is asked for');
  await world.press(card, 'Take it back');
  assert.deepEqual(world.ran('updates.unstage'), [{}]);
  assert.equal(buttonOf(world.card('firmware'), 'Stage it').getAttribute('aria-disabled'), 'true', 'the spare slot is shared');
  assert.match(text(world.card('firmware')), /After the system update has proved itself/);
});

test('a restart time set for the update makes one button', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates/slots': slots(), '/api/1/updates/settings': settings({ unattended: true, restart: '03:00' }) } } });
  await world.press(world.card('system'), 'Restart at 3:00 AM');
  assert.equal(world.dialog(), null, 'nothing is asked of a time that is already set');
  assert.deepEqual(world.ran('power.restart'), [{ at: '03:00' }]);
});

test('Restart now asks first, then runs the restart, and the line that counts the minute is in the System card', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates/slots': slots() } } });
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 60, at_ms: NOW + 60_000 } };
  await world.press(world.card('system'), 'Restart now…');
  assert.equal(world.dialog().querySelector('h2').textContent, 'Restart duke-nas?');
  assert.match(world.dialog().textContent, /To finish the update to 0\.16\.0\./);
  await world.submit();
  assert.deepEqual(world.ran('power.restart'), [{ at: null }]);
  assert.match(text(world.card('system')), /duke-nas restarts in 60 seconds/);
  assert.ok(buttonOf(world.card('system'), 'Cancel'));
});

test('a restart that is already set shows its line when the page opens', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/power': { shutdown: { end: 'reboot', in_seconds: 5400, at_ms: NOW + 5_400_000 } } } } });
  assert.match(text(world.card('system')), /duke-nas restarts at 4:42 PM/);
});

test('Hold and Release change the hold and read again', async (context) => {
  const world = await openUpdates(context);
  await world.press(world.card('system'), 'Hold this version');
  assert.deepEqual(world.ran('updates.hold'), [{ item: 'system' }]);
  const held = checkWith({ system: 'held' }, { system: { held_because: 'fell-back', held_at: '2026-11-14T12:12:00Z' } });
  world.answers['/api/1/updates'] = last(held);
  world.answers['/api/1/updates/failed'] = failed();
  world.answers['/api/1/updates/settings'] = settings({ hold: { system: '0.16.0', firmware: null }, held: { version: '0.16.0', why: 'fell-back', at_ms: NOW, start: 42 } });
  await world.emit('online');
  assert.match(text(world.card('system')), /0\.16\.0 is held.*It was taken back today at 12:12 PM: it failed its check \(BOOT-103\)\./);
  await world.press(world.card('system'), 'Release the hold');
  assert.deepEqual(world.ran('updates.unhold'), [{ item: 'system' }]);
  await world.press(world.card('held'), 'Release');
  assert.deepEqual(world.ran('updates.unhold').at(-1), { item: 'system' });
});

test('hold a version picks from what can be held, and asks why before it holds', async (context) => {
  const world = await openUpdates(context);
  const card = world.card('held');
  assert.match(text(card), /Hold a version/);
  await world.press(card, 'Hold it…');
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Hold the system update 0.16.0');
  assert.match(dialog.textContent, /Why \(optional\)/);
  assert.match(dialog.textContent, /Everyone who can sign in sees the reason, here and with update settings\./);
  assert.equal(dialog.querySelector('input').getAttribute('maxlength'), '200');
  assert.equal(world.ran('updates.hold').length, 0, 'nothing is held until the dialog says so');
  await world.submit();
  assert.deepEqual(world.ran('updates.hold'), [{ item: 'system' }], 'a hold needs no reason');
  assert.equal(world.dialog(), null);
  await world.press(world.card('held'), 'Release');
  assert.deepEqual(world.ran('apps.unhold'), [{ id: 'org.example.notes' }]);
});

test('the reason of a hold goes with it, as one line of at most 200 characters', async (context) => {
  const world = await openUpdates(context);
  await world.press(world.card('held'), 'Hold it…');
  const input = world.dialog().querySelector('input');
  input.value = '  Waiting for the nginx fix in alpha.3 ';
  await world.submit();
  assert.deepEqual(world.ran('updates.hold'), [{ item: 'system', why: 'Waiting for the nginx fix in alpha.3' }]);
  await world.press(world.card('held'), 'Hold it…');
  const again = world.dialog().querySelector('input');
  again.value = 'two\tcolumns';
  await world.submit();
  assert.match(world.dialog().textContent, /The reason is one line/);
  again.value = 'x'.repeat(201);
  await world.submit();
  assert.match(world.dialog().textContent, /The reason is at most 200 characters/);
  assert.equal(world.ran('updates.hold').length, 1, 'a reason that does not fit holds nothing');
  again.value = 'x'.repeat(200);
  await world.submit();
  assert.equal(world.ran('updates.hold').at(-1).why.length, 200);
  assert.equal(world.dialog(), null);
});

test('the reason of an app\'s hold goes to apps.hold, and the dialog says where it is read', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': last(checkWith({ system: 'current', firmware: 'current' })) } } });
  await world.press(world.card('held'), 'Hold it…');
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Hold nginx at 1.2.0');
  assert.match(dialog.textContent, /here and with bundle list/);
  dialog.querySelector('input').value = 'app tests use this version';
  await world.submit();
  assert.deepEqual(world.ran('apps.hold'), [{ id: 'org.nginx.nginx', why: 'app tests use this version' }]);
});

test('each hold says when, by whom and why, and a fallback keeps its own words', async (context) => {
  const reasons = {
    system: { version: '0.16.0', text: 'Waiting for the nginx fix in alpha.3', by: 1000, at_ms: Date.UTC(2026, 10, 2, 3, 6) },
  };
  const apps = bundles();
  apps.bundles[1] = { ...apps.bundles[1], held_reason: 'app tests use this version', held_by: 1000, held_since_ms: Date.UTC(2026, 10, 3, 9, 0) };
  const world = await openUpdates(context, {
    world: {
      reads: {
        '/api/1/updates/settings': settings({ hold: { system: '0.16.0', firmware: null }, hold_reasons: reasons }),
        '/api/1/apps': apps,
      },
    },
  });
  assert.match(text(world.card('held')), /System 0\.16\.0 · held on Nov 2 at 3:06 AM by chase: "Waiting for the nginx fix in alpha\.3"/);
  assert.match(text(world.card('held')), /notes 1\.2\.0 · held on Nov 3 at 9:00 AM by chase: "app tests use this version"/);
  assert.match(text(world.card('apps')), /Held at 1\.2\.0 · app tests use this version/);
  const fell = await openUpdates(context, {
    world: {
      reads: {
        '/api/1/updates/settings': settings({ hold: { system: '0.16.0', firmware: null }, held: { version: '0.16.0', why: 'fell-back', at_ms: Date.UTC(2026, 10, 10, 3, 6), start: 42 } }),
      },
    },
  });
  assert.match(text(fell.card('held')), /System 0\.16\.0 · held after it was taken back on Nov 10 at 3:06 AM \(UPD-105\)/);
  assert.match(text(fell.card('held')), /It was taken back, so it is not staged again until you release it\./);
});

test('an app\'s update runs for the app, and several run together', async (context) => {
  const world = await openUpdates(context);
  await world.press(world.card('apps'), 'Update');
  assert.deepEqual(world.ran('apps.update'), [{ id: 'org.nginx.nginx' }]);
  assert.equal(buttonOf(world.card('apps'), 'Install 1 now'), undefined, 'one app needs no Install now');
  const two = last({ ...check2() });
  const again = await openUpdates(context, { world: { reads: { '/api/1/updates': two } } });
  await again.press(again.card('apps'), 'Install 2 now');
  assert.deepEqual(again.ran('apps.update-all'), [{}]);
});

function check2() {
  const doc = checkWith();
  doc.offers.push(
    { item: 'org.a', from: '8.0.3', to: '8.0.4', state: 'update' },
    { item: 'org.b', from: '1.0.0', to: '2.0.0', state: 'update' },
  );
  return doc;
}

test('an update that asks for more stops with the sheet\'s words, and Update anyway runs it with yes', async (context) => {
  const world = await openUpdates(context, { world: { answers: { 'apps.update': { ok: true, task: 9, result: { task: 9 } } } } });
  await world.press(world.card('apps'), 'Update');
  world.answers['/api/1/tasks/9'] = {
    id: 9, action: 'apps.update', args: { id: 'org.nginx.nginx' }, state: 'running',
  };
  await world.emit('task', { id: 9, action: 'apps.update', state: 'stopped', note: 'It asks for the camera, which 1.2.0 did not.' });
  assert.match(text(world.card('apps')), /It asks for the camera, which 1\.2\.0 did not\./);
  await world.press(world.card('apps'), 'Update anyway');
  assert.deepEqual(world.ran('apps.update').at(-1), { id: 'org.nginx.nginx', yes: true });
});

// ---- Remind me later -------------------------------------------------------------------

const WEEK = 7 * 86_400_000;
const entry = (item, version, more = {}) => ({
  item, version, until_ms: NOW + WEEK, by: 1000, at_ms: NOW, stands: true, ...more,
});
/** The offers of an update ready for the system and the firmware set, and two apps, that fix no advisory. */
const calm = () => last(check2());
const appList = (more = {}) => ({
  bundles: [
    { id: 'org.a', name: 'A', version: '8.0.3', for_everyone: true, held: false, ...more['org.a'] },
    { id: 'org.b', name: 'B', version: '1.0.0', for_everyone: true, held: false, ...more['org.b'] },
  ],
});

test('Later puts the reminder of the system update off for a day or a week, and Remind me now brings it back', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': calm() } } });
  const system = world.card('system');
  assert.ok(buttonOf(system, 'Later ▾'));
  assert.equal(buttonOf(system, 'Later ▾').getAttribute('aria-haspopup'), 'menu');
  await world.press(system, 'Later ▾');
  assert.deepEqual(system.querySelectorAll('.menu-item').map((one) => one.textContent), ['Remind me tomorrow', 'Remind me in a week']);
  world.answers['/api/1/updates/settings'] = settings({ later: [entry('system', '0.16.0')] });
  await world.press(system, 'Remind me in a week');
  assert.deepEqual(world.ran('updates.later'), [{ item: 'system', for: 'week' }]);
  assert.match(text(world.card('system')), /Put off until Nov 21 at 3:12 PM by chase · Remind me now/);
  assert.equal(buttonOf(world.card('system'), 'Later ▾'), undefined, 'a reminder that is put off asks for no more time');
  assert.ok(buttonOf(world.card('system'), 'Stage it'), 'the rest of the card is as before');
  world.answers['/api/1/updates/settings'] = settings();
  await world.press(world.card('system'), 'Remind me now');
  assert.deepEqual(world.ran('updates.later').at(-1), { item: 'system', for: 'off' });
  assert.ok(buttonOf(world.card('system'), 'Later ▾'));
  assert.doesNotMatch(text(world.card('system')), /Put off until/);
  await world.press(world.card('system'), 'Later ▾');
  await world.press(world.card('system'), 'Remind me tomorrow');
  assert.deepEqual(world.ran('updates.later').at(-1), { item: 'system', for: 'day' });
});

test('Later is on the firmware set too, and on each app that is told of', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': calm(), '/api/1/apps': appList() } } });
  assert.ok(buttonOf(world.card('firmware'), 'Later ▾'));
  assert.equal(buttonOf(world.card('apps'), 'Later ▾').getAttribute('aria-label'), 'Later for A');
  await world.press(world.card('firmware'), 'Later ▾');
  await world.press(world.card('firmware'), 'Remind me in a week');
  assert.deepEqual(world.ran('updates.later'), [{ item: 'firmware', for: 'week' }]);
  const row = world.card('apps').querySelectorAll('li')[1];
  await world.press(row, 'Later ▾');
  await world.press(row, 'Remind me tomorrow');
  assert.deepEqual(world.ran('updates.later').at(-1), { item: 'org.b', for: 'day' });
});

test('an app that is put off says until when and by whom, with Remind me now', async (context) => {
  const settled = settings({ later: [entry('org.a', '8.0.4', { by: 1001 })] });
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': calm(), '/api/1/apps': appList(), '/api/1/updates/settings': settled } } });
  const row = world.card('apps').querySelectorAll('li')[0];
  assert.match(text(row), /Put off until Nov 21 at 3:12 PM by account 1001 · Remind me now/);
  assert.equal(buttonOf(row, 'Later ▾'), undefined);
  await world.press(row, 'Remind me now');
  assert.deepEqual(world.ran('updates.later'), [{ item: 'org.a', for: 'off' }]);
});

test('an update that fixes an advisory has no Later, and a line that says why; Hold stays', async (context) => {
  const world = await openUpdates(context);
  assert.match(text(world.card('system')), /It fixes DKSA-2026-0001 \(high\): it is not put off\. Hold it if you must\./);
  assert.equal(buttonOf(world.card('system'), 'Later ▾'), undefined);
  assert.ok(buttonOf(world.card('system'), 'Hold this version'));
  assert.match(text(world.card('apps')), /It fixes DKSA-2026-0002 \(high\): it is not put off\. Hold it if you must\./);
  assert.equal(buttonOf(world.card('apps'), 'Later ▾'), undefined);
  assert.equal(world.ran('updates.later').length, 0);
});

test('an advisory that came after the reminder was put off shows the offer as before, with its advisory', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates/settings': settings({ later: [entry('system', '0.16.0', { stands: false })] }) } } });
  assert.doesNotMatch(text(world.card('system')), /Put off until/);
  assert.match(text(world.card('system')), /It fixes DKSA-2026-0001/);
});

test('an update whose advisories could not be read says it cannot tell, and is not put off', async (context) => {
  const doc = check2();
  doc.unchecked = [{ item: 'security', why: 'the update server did not answer' }];
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': last(doc) } } });
  assert.match(text(world.card('system')), /Whether it fixes an advisory cannot be told, so it is not put off\./);
  assert.equal(buttonOf(world.card('system'), 'Later ▾'), undefined);
});

test('apps that update by themselves have no reminder to put off', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': calm(), '/api/1/apps': appList(), '/api/1/updates/settings': settings({ apps: 'auto' }) } } });
  assert.equal(buttonOf(world.card('apps'), 'Later ▾'), undefined);
  assert.ok(buttonOf(world.card('system'), 'Later ▾'), 'the system is told of whatever the apps do');
  assert.ok(buttonOf(world.card('apps'), 'Tonight'));
});

test('a reader sees what is put off, and has no menu and no Remind me now', async (context) => {
  const settled = settings({ later: [entry('system', '0.16.0')] });
  const world = await openUpdates(context, { session: { admin: false }, world: { reads: { '/api/1/updates': calm(), '/api/1/updates/settings': settled } } });
  assert.match(text(world.card('system')), /Put off until Nov 21 at 3:12 PM by chase/);
  assert.doesNotMatch(text(world.card('system')), /Remind me now/);
  assert.equal(buttonOf(world.card('firmware'), 'Later ▾'), undefined);
  assert.equal(buttonOf(world.card('apps'), 'Tonight'), undefined);
  assert.equal(buttonOf(world.card('apps'), 'Install tonight'), undefined);
});

test('a refused Later says so in the card and changes nothing', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': calm() }, answers: { 'updates.later': refusal('UPD-107', 'It fixes an advisory') } } });
  await world.press(world.card('system'), 'Later ▾');
  await world.press(world.card('system'), 'Remind me tomorrow');
  assert.equal(world.problems.length, 1);
  assert.doesNotMatch(text(world.card('system')), /Put off until/);
});

// ---- Install in tonight's window -------------------------------------------------------

test('Tonight asks for an app\'s update in tonight\'s window, with the words of the window', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': calm(), '/api/1/apps': appList() } } });
  assert.match(text(world.card('apps')), /Installs tonight, between 2:00 and 3:00 AM\./);
  assert.doesNotMatch(text(world.card('apps')), /restart time/);
  const row = world.card('apps').querySelectorAll('li')[0];
  await world.press(row, 'Tonight');
  assert.deepEqual(world.ran('apps.tonight'), [{ id: 'org.a' }]);
});

test('the window is the hour before the restart time of the server', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': calm(), '/api/1/apps': appList(), '/api/1/updates/settings': settings({ unattended: true, restart: '04:30' }) } } });
  assert.match(text(world.card('apps')), /Installs tonight, between 3:30 and 4:30 AM: the hour before this server's restart time\./);
});

test('Install tonight sits beside Install now, and asks for every update', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': calm(), '/api/1/apps': appList() } } });
  const apps = world.card('apps');
  assert.deepEqual(apps.querySelectorAll('.btns button').map((one) => one.textContent).filter((one) => /^Install/.test(one)), ['Install 2 now', 'Install tonight']);
  await world.press(apps, 'Install tonight');
  assert.deepEqual(world.ran('apps.tonight-all'), [{}]);
  assert.equal(world.ran('apps.update-all').length, 0);
});

test('one update has Tonight on its row and no Install tonight', async (context) => {
  const world = await openUpdates(context);
  assert.ok(buttonOf(world.card('apps'), 'Tonight'));
  assert.equal(buttonOf(world.card('apps'), 'Install tonight'), undefined);
});

test('an app that waits for tonight says so, and Not tonight takes it off', async (context) => {
  const asked = { tonight: { to: '8.0.4', asked_ms: NOW - 60_000, by: 1000, missed_ms: null, starts_ms: NOW + 3_600_000 } };
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': calm(), '/api/1/apps': appList({ 'org.a': asked }) } } });
  const row = world.card('apps').querySelectorAll('li')[0];
  assert.match(text(row), /Installs tonight/);
  assert.ok(buttonOf(row, 'Not tonight'));
  assert.equal(buttonOf(row, 'Tonight'), undefined);
  assert.ok(buttonOf(row, 'Update'), 'it can still be taken now');
  assert.ok(buttonOf(world.card('apps'), 'Install tonight'), 'the other app is not asked for yet');
  await world.press(row, 'Not tonight');
  assert.deepEqual(world.ran('apps.not-tonight'), [{ id: 'org.a' }]);
});

test('a window that passed while the machine was off says it installs tonight', async (context) => {
  const missed = { tonight: { to: '8.0.4', asked_ms: NOW - 86_400_000, by: 1000, missed_ms: NOW - 3_600_000, starts_ms: NOW + 3_600_000 } };
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': calm(), '/api/1/apps': appList({ 'org.a': missed }) } } });
  assert.match(text(world.card('apps')), /The window passed while the machine was off: it installs tonight\./);
});

test('an update that asks for more stops with the sheet first, and Install tonight anyway asks with yes', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': calm(), '/api/1/apps': appList() }, answers: { 'apps.tonight': { ok: true, task: 9, result: { task: 9 } } } } });
  const row = () => world.card('apps').querySelectorAll('li')[0];
  await world.press(row(), 'Tonight');
  world.answers['/api/1/tasks/9'] = {
    id: 9,
    action: 'apps.tonight',
    args: { id: 'org.a' },
    state: 'done',
    status: 1,
    log: ['A 8.0.4 asks for more than 8.0.3 did:', '  - it may listen on all addresses', 'bundle: not queued: it asks for more than 8.0.3 did: network; --yes queues it anyway'],
  };
  await world.emit('task', { id: 9, action: 'apps.tonight', state: 'done' });
  assert.match(text(row()), /It asks for more than the version before\./);
  assert.match(row().querySelector('pre').textContent, /A 8\.0\.4 asks for more than 8\.0\.3 did:\n {2}- it may listen on all addresses/);
  assert.doesNotMatch(row().querySelector('pre').textContent, /yes queues it anyway/);
  assert.equal(buttonOf(row(), 'Tonight'), undefined);
  await world.press(row(), 'Install tonight anyway');
  assert.deepEqual(world.ran('apps.tonight').at(-1), { id: 'org.a', yes: true });
  assert.ok(buttonOf(row(), 'Update anyway'));
});

test('an update that asks for more ends done with status 1, and its sheet is read from the task', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': calm(), '/api/1/apps': appList() }, answers: { 'apps.update': { ok: true, task: 8, result: { task: 8 } } } } });
  await world.press(world.card('apps').querySelectorAll('li')[0], 'Update');
  world.answers['/api/1/tasks/8'] = {
    id: 8, action: 'apps.update', args: { id: 'org.a' }, state: 'done', status: 1, log: ['A 8.0.4 asks for more:', 'Install it? [y/N] ', 'bundle: not installed: nobody was there to say yes'],
  };
  await world.emit('task', { id: 8, action: 'apps.update', state: 'done' });
  const row = world.card('apps').querySelectorAll('li')[0];
  assert.equal(row.querySelector('pre').textContent, 'A 8.0.4 asks for more:');
  assert.ok(buttonOf(row, 'Update anyway'));
  assert.ok(buttonOf(row, 'Install tonight anyway'));
});

test('an update that ended well leaves no sheet', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': calm(), '/api/1/apps': appList() }, answers: { 'apps.update': { ok: true, task: 7, result: { task: 7 } } } } });
  await world.press(world.card('apps').querySelectorAll('li')[0], 'Update');
  world.answers['/api/1/tasks/7'] = {
    id: 7, action: 'apps.update', args: { id: 'org.a' }, state: 'done', status: 0, log: ['A is updated'],
  };
  await world.emit('task', { id: 7, action: 'apps.update', state: 'done' });
  assert.equal(world.card('apps').querySelector('pre'), null);
  assert.ok(buttonOf(world.card('apps').querySelectorAll('li')[0], 'Update'));
});

test('a flaw fixed by an update is read from the advisories only when apps have flaws and updates', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates': last(checkWith()) } } });
  assert.equal(world.calls.includes('/api/1/updates/advisories'), false);
});

test('Show the advisories reads them in full and lists them under the card', async (context) => {
  const world = await openUpdates(context);
  await world.press(world.card('security'), 'Show the advisories');
  assert.match(text(world.card('security')), /DKSA-2026-0001 · high · The system/);
  assert.match(text(world.card('security')), /DKSA-2026-0002 · high · org\.nginx\.nginx/);
});

test('the settings are saved as the arguments of what moved, and can be put back', async (context) => {
  const world = await openUpdates(context);
  const card = world.card('settings');
  const radios = card.querySelectorAll('input').filter((one) => one.getAttribute('type') === 'radio');
  const pick = (name, value) => {
    for (const one of radios.filter((input) => input.getAttribute('name') === `upd-${name}`)) one.checked = one.getAttribute('value') === value;
    radios.find((input) => input.getAttribute('name') === `upd-${name}` && input.getAttribute('value') === value).fire('change');
  };
  pick('check', 'off');
  assert.match(text(card), /Nobody is told of security fixes\. SEC-131 warns after 7 days\./);
  pick('channel', 'beta');
  await world.press(card, 'Save');
  assert.deepEqual(world.ran('updates.set'), [{ check: 'off' }]);
  assert.deepEqual(world.ran('updates.channel'), [{ channel: 'beta' }]);
  assert.match(text(card), /Saved\./);
  await world.press(card, 'Undo');
  assert.deepEqual(world.ran('updates.set').at(-1), { check: 'notify' });
  assert.deepEqual(world.ran('updates.channel').at(-1), { channel: 'stable' });
  assert.match(text(card), /Put back as it was\./);
});

test('saving nothing says so, and a restart time that is wrong runs nothing', async (context) => {
  const world = await openUpdates(context);
  const card = world.card('settings');
  await world.press(card, 'Save');
  assert.match(text(card), /Nothing to change\./);
  for (const one of card.querySelectorAll('input').filter((input) => input.getAttribute('name') === 'upd-system')) one.checked = one.getAttribute('value') === 'auto';
  card.querySelectorAll('input').find((one) => one.getAttribute('name') === 'upd-system').fire('change');
  card.querySelector('input[type="time"]').value = '25:00';
  await world.press(card, 'Save');
  assert.match(text(card), /A time is written HH:MM/);
  assert.equal(world.ran('updates.set').length, 0);
});

test('a refusal of Save is shown in the card and nothing is offered to undo', async (context) => {
  const world = await openUpdates(context, { world: { answers: { 'updates.set': refusal('ADM-142', 'A trial runs') } } });
  const card = world.card('settings');
  const off = card.querySelectorAll('input').find((one) => one.getAttribute('name') === 'upd-check' && one.getAttribute('value') === 'off');
  card.querySelectorAll('input').filter((one) => one.getAttribute('name') === 'upd-check').forEach((one) => { one.checked = false; });
  off.checked = true;
  await world.press(card, 'Save');
  assert.equal(world.problems.length, 1);
  assert.equal(world.problems[0][0].id, 'ADM-142');
  assert.equal(buttonOf(card, 'Undo'), undefined);
});

test('the checks: remove a service, a required check and a wanted one', async (context) => {
  const world = await openUpdates(context);
  const card = world.card('checks');
  assert.match(text(card), /nginx · https:\/\/localhost\/ answers within 30 s/);
  assert.match(text(card), /Seen through after 120 seconds with no restart/);
  await world.press(card, 'Remove');
  assert.deepEqual(world.ran('updates.essential'), [{ service: 'logd', on: false }]);
  const removes = world.card('checks').querySelectorAll('button').filter((one) => one.textContent === 'Remove');
  assert.equal(removes.length, 5 + 1 + 2, 'five services, one required check and two wanted');
  removes[5].click();
  await settle();
  removes[7].click();
  await settle();
  assert.deepEqual(world.ran('updates.check-remove'), [{ name: 'nginx' }, { name: 'backup place' }]);
});

test('Add a check opens the question, refuses a bad answer in words, and adds a good one', async (context) => {
  const world = await openUpdates(context);
  await world.press(world.card('checks'), 'Add a check…');
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Add a check');
  await world.submit();
  assert.equal(world.ran('updates.check-add').length, 0);
  assert.match(dialog.textContent, /Give the check a name/);
  const inputs = dialog.querySelectorAll('input');
  const [target, name] = inputs;
  dialog.querySelector('select').value = 'url';
  target.value = 'https://localhost/';
  name.value = 'web site';
  await world.submit();
  assert.deepEqual(world.ran('updates.check-add'), [{ name: 'web site', url: 'https://localhost/', required: false, timeout: 30 }]);
  assert.equal(world.dialog(), null);
});

test('Add a service lists the services that are not checked yet', async (context) => {
  const world = await openUpdates(context);
  await world.press(world.card('checks'), 'Add a service…');
  const options = world.dialog().querySelectorAll('option').map((one) => one.textContent);
  assert.deepEqual(options, ['nginx', 'postgres']);
  world.dialog().querySelector('select').value = 'postgres';
  await world.submit();
  assert.deepEqual(world.ran('updates.essential'), [{ service: 'postgres', on: true }]);
});

test('Go back asks which settings, runs the going back, then asks to restart', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates/slots': slots({ staged: null, previous: 'a', running: 'b', default: 'b' }) } } });
  assert.match(text(world.card('system')), /Kept for going back: 0\.15\.0 in slot A/);
  await world.press(world.card('system'), 'Go back to it…');
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Go back to 0.15.0?');
  assert.match(dialog.textContent, /The settings from before the update to 0\.14\.0/);
  await world.submit();
  assert.deepEqual(world.ran('updates.undo'), [{ item: 'system', settings: 'before' }]);
  assert.equal(world.dialog().querySelector('h2').textContent, 'Restart duke-nas?');
  assert.match(world.dialog().textContent, /To go back to 0\.15\.0\./);
});

test('History is read when it is asked for, newest first', async (context) => {
  const world = await openUpdates(context);
  assert.equal(world.calls.includes('/api/1/updates/history'), false);
  await world.press(world.card('history'), 'Show the history');
  assert.match(text(world.card('history')), /System 0\.14\.0 to 0\.15\.0/);
  assert.match(text(world.card('history')), /UPD-101/);
  assert.match(text(world.card('history')), /org\.example\.notes.*Held.*chase/);
  assert.match(text(world.card('history')), /org\.nginx\.nginx.*Updated.*by itself/);
});

test('what happened shows for a start that failed, with the search that can find the change', async (context) => {
  const record = failed({ failed: { ...failed().failed, result: 'failed' }, since_good: failed().since_good });
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates/failed': record, '/api/1/updates/bisect': search() } } });
  const card = world.card('happened');
  assert.match(text(card), /The start today at 12:12 PM failed/);
  assert.match(text(card), /Start 42 today at 12:12 PM, release 0\.16\.0, slot B/);
  assert.equal(card.querySelector('a[href="/help/errors/BOOT-103"]').textContent, 'BOOT-103');
  assert.match(text(card), /nginx failed a required check \(BOOT-103\)\./);
  assert.match(text(card), /Likely cause: its certificate is not readable/);
  assert.match(text(card), /org\.nginx\.nginx 1\.30\.6, installed/);
  assert.equal(card.querySelector('a[href="/report?about=failure"]').textContent, 'Report it');
  assert.match(text(card), /4 changes take at most 2 restarts/);
  assert.match(text(card), /enp1s0: the machine's hardware, which no restart changes/);
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 60, at_ms: NOW + 60_000 } };
  world.answers['/api/1/updates/bisect'] = running();
  await world.press(card, 'Start the search');
  assert.deepEqual(world.ran('updates.bisect'), [{}]);
  assert.match(text(world.card('system')), /duke-nas restarts in 60 seconds/);
  assert.match(text(world.card('happened')), /Try 1: left out system, boot options/);
  assert.ok(buttonOf(world.card('happened'), 'Stop the search'));
  assert.equal(world.timers.at(-1)[0], 5000);
  world.answers['/api/1/updates/bisect'] = found(false);
  await world.timers.at(-1)[1]();
  await settle();
  assert.match(text(world.card('happened')), /Found: org\.nginx\.nginx 1\.30\.6, by elimination/);
});

test('after an update was taken back, the search is not offered, and Report it is', async (context) => {
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates/failed': failed(), '/api/1/updates/slots': slots({ staged: null, running: 'a', default: 'a' }) } } });
  const card = world.card('happened');
  assert.match(text(card), /The update to 0\.16\.0 was taken back/);
  assert.match(text(card), /The machine went back to 0\.14\.0 with today's settings\./);
  assert.match(text(card), /not offered after an update was taken back/);
  assert.equal(buttonOf(card, 'Start the search'), undefined);
  assert.ok(card.querySelector('a[href="/report?about=failure"]'));
});

test('the result of a search says what it found, how to undo it, and Make sure when it was by elimination', async (context) => {
  const record = failed({ failed: { ...failed().failed, result: 'failed' } });
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates/failed': record, '/api/1/updates/bisect': found(false) } } });
  const card = world.card('happened');
  assert.match(text(card), /Found: org\.nginx\.nginx 1\.30\.6, by elimination/);
  assert.match(text(card), /elevate bundle rollback org\.nginx\.nginx --for-everyone/);
  assert.match(text(card), /This start is a try of the search\./);
  await world.press(card, 'Make sure');
  assert.equal(world.ran('updates.bisect-confirm').length, 1);
  const sure = await openUpdates(context, { world: { reads: { '/api/1/updates/failed': record, '/api/1/updates/bisect': found(true) } } });
  assert.equal(buttonOf(sure.card('happened'), 'Make sure'), undefined);
});

test('a search that waits can be stopped', async (context) => {
  const record = failed({ failed: { ...failed().failed, result: 'failed' } });
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates/failed': record, '/api/1/updates/bisect': running() } } });
  assert.match(text(world.card('happened')), /The next try starts in 45 seconds\./);
  assert.match(text(world.card('happened')), /Try 1: left out system, boot options/);
  await world.press(world.card('happened'), 'Stop the search');
  assert.equal(world.ran('updates.bisect-stop').length, 1);
});

test('a reader sees what happened and cannot start a search', async (context) => {
  const record = failed({ failed: { ...failed().failed, result: 'failed' } });
  const world = await openUpdates(context, { session: { admin: false }, world: { reads: { '/api/1/updates/failed': record } } });
  assert.match(text(world.card('happened')), /4 changes take at most 2 restarts/);
  assert.equal(buttonOf(world.card('happened'), 'Start the search'), undefined);
});

test('a read that fails is said by the shell, and the page still draws what it has', async (context) => {
  const error = new ApiError({ id: 'ADM-129', title: 'update is not installed', detail: '', fix: '', status: 501 });
  const world = await openUpdates(context, { world: { reads: { '/api/1/updates/settings': error, '/api/1/updates/checks': error } } });
  assert.match(text(world.card('settings')), /could not be read/);
  assert.match(text(world.card('checks')), /could not be read/);
  assert.ok(world.card('system'));
});

test('leaving the page stops the read that a change waits for', async (context) => {
  const world = await openUpdates(context);
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const reads = world.calls.length;
  await world.emit('change');
  world.view.signal.dispatchEvent(new Event('abort'));
  context.mock.timers.tick(1000);
  await settle();
  assert.equal(world.calls.length, reads);
});
