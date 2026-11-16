// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Who may reach the console (design 8.7 and 9.2): the row, the two dialogs,
// the try that begins a trial of `console`, and the case in which the change
// would cut this browser off.

import assert from 'node:assert/strict';
import test from 'node:test';

import { settings, sparkIdle, sparkWith } from './security-fixtures.mjs';
import {
  buttonOf, control, fieldOf, openSecurity, refusal, settle, tick,
} from './security-view.mjs';

const OK = { ok: true, result: {} };
const text = (page) => page.card('reach').textContent;
const FRESH = '/api/1/console/fresh/console';
const reached = { fresh: true, from: '192.168.1.25', via: { address: '192.168.1.10', port: 9090, card: 'net0' } };
const publicConsole = settings({ console: { listen: 9090, reach: 'any', certificate: 'this-machine' } });

/** A world in which `console.reach` with a try begins a trial that the document then shows. */
function trialWorld(reads = {}) {
  const state = { spark: sparkIdle() };
  return {
    state,
    world: {
      reads: { '/api/1/services': () => state.spark, [FRESH]: reached, ...reads },
      answers: {
        'console.reach': (args) => {
          if (args.try !== undefined) state.spark = sparkWith('console');
          return OK;
        },
        'security.sign-in-set': OK,
      },
    },
  };
}

const openForm = async (page) => {
  control(page.card('reach'), 'reach-change').click();
  await settle();
  return page.dialog();
};

test('the row says the port and who is let in, and the button offers the other', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  assert.match(text(page), /ConsolePort 9090 · this network and WireGuard only/);
  assert.equal(control(page.card('reach'), 'reach-change').textContent, 'Make the console public…');
  const open = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/security/sign-in': publicConsole } } });
  assert.match(text(open), /Port 9090 · anyone who can reach this server/);
  assert.equal(control(open.card('reach'), 'reach-change').textContent, 'Only this network…');
});

test('a reader sees the row and no button', async (context) => {
  const page = await openSecurity(context, { session: { admin: false } });
  assert.match(text(page), /Port 9090/);
  assert.equal(control(page.card('reach'), 'reach-change').hasAttribute('hidden'), true);
});

test('settings that cannot be read say so under the title', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/security/sign-in': refusal('ADM-135', 'admind could not answer', 500) } } });
  assert.match(text(page), /The console's settings could not be read\./);
});

test('making the console public says what admin.toml asks, offers a second step first, and only tries', async (context) => {
  const { world } = trialWorld({ '/api/1/security/sign-in': settings({ sign_in: { second_step: 'if-set', session_hours: 12, idle_minutes: 30 } }) });
  const page = await openSecurity(context, { unlocked: true, world });
  const dialog = await openForm(page);
  assert.equal(dialog.querySelector('h2').textContent, 'Make the console public');
  assert.match(dialog.textContent, /Anyone who can reach this server can then load the console's sign-in page\. admin\.toml\(5\) asks for care/);
  assert.match(dialog.textContent, /The rule now asks for a second step only of those who have added one\./);
  assert.equal(fieldOf(dialog, 'Ask administrators for a second step first').checked, true);
  assert.equal(buttonOf(dialog, 'Apply'), undefined, 'a change that is not kept goes back');
  buttonOf(dialog, 'Try it').click();
  await settle();
  assert.deepEqual(page.allRan().filter((action) => action === 'security.sign-in-set' || action === 'console.reach'), ['security.sign-in-set', 'console.reach']);
  assert.deepEqual(page.ran('security.sign-in-set'), [{ second_step: 'administrators' }]);
  assert.deepEqual(page.ran('console.reach'), [{ reach: 'any', try: 60 }], 'the window of a change that cuts nobody off is 60 seconds');
  assert.equal(page.dialog(), null);
});

test('the second step is not offered when administrators have it already, and is not asked when it is turned off', async (context) => {
  const { world } = trialWorld();
  const page = await openSecurity(context, { unlocked: true, world });
  const dialog = await openForm(page);
  assert.equal(fieldOf(dialog, 'Ask administrators for a second step first'), null);
  const { world: other } = trialWorld({ '/api/1/security/sign-in': settings({ sign_in: { second_step: 'if-set', session_hours: 12, idle_minutes: 30 } }) });
  const again = await openSecurity(context, { unlocked: true, world: other });
  const form = await openForm(again);
  tick(fieldOf(form, 'Ask administrators for a second step first'), false);
  buttonOf(form, 'Try it').click();
  await settle();
  assert.equal(again.ran('security.sign-in-set').length, 0);
  assert.equal(again.ran('console.reach').length, 1);
});

test('a refused rule stops the change before the reach is touched', async (context) => {
  const { world } = trialWorld({ '/api/1/security/sign-in': settings({ sign_in: { second_step: 'if-set', session_hours: 12, idle_minutes: 30 } }) });
  world.answers['security.sign-in-set'] = refusal('ADM-140', 'The rule was refused');
  const page = await openSecurity(context, { unlocked: true, world });
  const dialog = await openForm(page);
  buttonOf(dialog, 'Try it').click();
  await settle();
  assert.equal(page.ran('console.reach').length, 0);
  assert.equal(page.dialog(), dialog);
});

test('a try shows the banner, asks whether this browser reaches the console, and Keep runs console.keep', async (context) => {
  const { world } = trialWorld();
  const page = await openSecurity(context, { unlocked: true, world });
  const dialog = await openForm(page);
  buttonOf(dialog, 'Try it').click();
  await settle();
  const card = page.card('reach');
  assert.equal(card.querySelector('.try-box').hasAttribute('hidden'), false);
  assert.match(card.querySelector('.try-words').textContent, /The change is undone at .* unless it is kept\./);
  assert.ok(page.calls.includes(FRESH), 'the console\'s own fresh route is asked');
  assert.equal(control(card, 'reach-change').getAttribute('aria-disabled'), 'true');
  const keep = buttonOf(card, 'Keep this change');
  assert.equal(keep.getAttribute('aria-disabled'), 'false');
  keep.click();
  await settle();
  assert.deepEqual(page.ran('console.keep'), [{}]);
});

test('Keep stays off while this browser has not reached the console over the new settings', async (context) => {
  const { world } = trialWorld({ [FRESH]: { fresh: false } });
  const page = await openSecurity(context, { unlocked: true, world });
  const dialog = await openForm(page);
  buttonOf(dialog, 'Try it').click();
  await settle();
  assert.equal(buttonOf(page.card('reach'), 'Keep this change').getAttribute('aria-disabled'), 'true');
  assert.match(page.card('reach').querySelector('.try-reason').textContent, /This browser cannot reach the console over the new settings/);
});

test('Undo now runs console.undo', async (context) => {
  const { world } = trialWorld();
  const page = await openSecurity(context, { unlocked: true, world });
  buttonOf(await openForm(page), 'Try it').click();
  await settle();
  buttonOf(page.card('reach'), 'Undo now').click();
  await settle();
  assert.deepEqual(page.ran('console.undo'), [{}]);
});

test('the rules are read again when the try ends, so that the row says what the console does now', async (context) => {
  const { world } = trialWorld();
  const page = await openSecurity(context, { unlocked: true, world });
  buttonOf(await openForm(page), 'Try it').click();
  await settle();
  const before = page.calls.filter((path) => path === '/api/1/security/sign-in').length;
  buttonOf(page.card('reach'), 'Undo now').click();
  await settle();
  assert.equal(page.calls.filter((path) => path === '/api/1/security/sign-in').length, before + 1);
});

// ---- only this network --------------------------------------------------------------------

test('a browser on the machine\'s network may apply the change at once, or try it', async (context) => {
  const { world } = trialWorld({ '/api/1/security/sign-in': publicConsole });
  const page = await openSecurity(context, { unlocked: true, world });
  const dialog = await openForm(page);
  assert.equal(dialog.querySelector('h2').textContent, 'Only this network');
  assert.equal(dialog.querySelectorAll('.banner').filter((one) => !one.hasAttribute('hidden')).length, 0);
  buttonOf(dialog, 'Apply').click();
  await settle();
  const [call] = page.ranWith('console.reach');
  assert.deepEqual(call[1], { reach: 'private' });
  assert.equal(call[2].words.done, 'The console now answers this network only.');
  assert.equal(page.dialog(), null);
});

test('a browser that comes over WireGuard is on a private range and is not cut off', async (context) => {
  const { world } = trialWorld({ '/api/1/security/sign-in': publicConsole });
  const page = await openSecurity(context, { unlocked: true, session: { from: '10.8.0.2' }, world });
  assert.ok(buttonOf(await openForm(page), 'Apply'));
});

test('a browser from outside is told it will be cut off, with the time, and is offered Try it only (22.7)', async (context) => {
  const { world } = trialWorld({ '/api/1/security/sign-in': publicConsole });
  const page = await openSecurity(context, { unlocked: true, session: { from: '203.0.113.9' }, world });
  const dialog = await openForm(page);
  const warning = dialog.querySelectorAll('.banner').find((one) => !one.hasAttribute('hidden'));
  assert.match(warning.textContent, /^This browser comes from 203\.0\.113\.9, which is outside\. After this change it cannot reach the console: the change is undone at 10:17 AM unless it is kept from a browser on this network\.$/);
  assert.equal(buttonOf(dialog, 'Apply'), undefined);
  assert.ok(buttonOf(dialog, 'Try it'));
  fieldOf(dialog, 'Undo it after').value = '600';
  fieldOf(dialog, 'Undo it after').fire('change');
  assert.match(dialog.textContent, /the change is undone at 10:22 AM/);
  buttonOf(dialog, 'Try it').click();
  await settle();
  assert.deepEqual(page.ran('console.reach'), [{ reach: 'private', try: 600 }]);
});

test('a browser that is on one of the machine\'s own networks, though not private, is not cut off', async (context) => {
  const { world } = trialWorld({
    '/api/1/security/sign-in': publicConsole,
    '/api/1/network': { cards: [{ name: 'net0', addresses: [{ address: '203.0.113.2', prefix: 24 }] }] },
  });
  const page = await openSecurity(context, { unlocked: true, session: { from: '203.0.113.9' }, world });
  assert.ok(buttonOf(await openForm(page), 'Apply'));
});

test('the machine\'s networks that cannot be read leave the question to the private ranges', async (context) => {
  const { world } = trialWorld({ '/api/1/security/sign-in': publicConsole, '/api/1/network': refusal('ADM-135', 'no network', 500) });
  const page = await openSecurity(context, { unlocked: true, session: { from: '203.0.113.9' }, world });
  const dialog = await openForm(page);
  assert.equal(buttonOf(dialog, 'Apply'), undefined);
});

test('a locked console is asked to unlock first', async (context) => {
  const page = await openSecurity(context, { unlocked: false });
  control(page.card('reach'), 'reach-change').click();
  await settle();
  assert.match(page.dialog().textContent, /make the console public/);
});
