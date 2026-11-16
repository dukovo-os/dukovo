// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The SSH card of the Security page (design 8.6, 9.1 and 22.5): the row, the
// switch, the form, and the trial that a change which narrows who signs in
// begins, with what to check while it runs.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  at, lastlog, sparkIdle, sparkWith, ssh,
} from './security-fixtures.mjs';
import {
  buttonOf, choose, control, fieldOf, openSecurity, refusal, settle, tick,
} from './security-view.mjs';

const OK = { ok: true, result: {} };
const text = (page) => page.card('ssh').textContent;

/** A world in which `ssh.set` and `ssh.off` begin a trial that the document then shows. */
function trialWorld(more = {}) {
  const state = { spark: sparkIdle() };
  const begin = () => {
    state.spark = sparkWith('ssh');
    return OK;
  };
  return {
    state,
    world: {
      reads: { '/api/1/services': () => state.spark, ...more.reads },
      answers: { 'ssh.set': begin, 'ssh.off': begin, ...more.answers },
    },
  };
}

const openForm = async (page, via = 'ssh-change') => {
  control(page.card('ssh'), via).click();
  await settle();
  return page.dialog();
};

// ---- the row --------------------------------------------------------------------------

test('the row says the port, how people sign in and that root cannot, with the host key', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  assert.match(text(page), /Port 22 · keys and passwords · root cannot sign in/);
  assert.match(text(page), /SHA256:ZkAslGjFiUHdGf/);
  assert.match(text(page), /Forwarding.*yes, gateway ports no \(shown here; the form does not edit them\)/);
  assert.equal(control(page.card('ssh'), 'ssh-switch').getAttribute('aria-checked'), 'true');
});

test('SSH that is off says Off and the switch is off', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/ssh': ssh({ on: false }) } } });
  assert.match(text(page), /SSH\s*Off|SSHOff/);
  assert.equal(control(page.card('ssh'), 'ssh-switch').getAttribute('aria-checked'), 'false');
});

test('a list of who may sign in and the denied names are in the row, and the denied are kept as they are', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/ssh': ssh({ allow_users: ['alice', 'owner'], deny_groups: ['guests'] }) } } });
  assert.match(text(page), /only alice and owner/);
  assert.match(text(page), /groups guests \(kept as they are; the form does not edit them\)/);
});

test('a reader sees the row and neither the switch nor Change', async (context) => {
  const page = await openSecurity(context, { session: { admin: false } });
  assert.match(text(page), /Port 22/);
  assert.equal(control(page.card('ssh'), 'ssh-switch').hasAttribute('hidden'), true);
  assert.equal(control(page.card('ssh'), 'ssh-change').hasAttribute('hidden'), true);
});

test('a file with a mistake says which line and that the form cannot change it', async (context) => {
  const mistakes = [{ line: 7, why: 'unknown keyword', text: 'PasswrdAuthentication no' }];
  const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/ssh': ssh({ mistakes }) } } });
  assert.match(text(page), /sshd_config has a mistake on line 7: unknown keyword \(PasswrdAuthentication no\)\./);
  assert.match(text(page), /the form cannot change the file until it is fixed/);
  assert.equal(control(page.card('ssh'), 'ssh-change').getAttribute('aria-disabled'), 'true');
  control(page.card('ssh'), 'ssh-change').click();
  await settle();
  assert.equal(page.dialog(), null);
});

test('settings that cannot be read say so under the title', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/ssh': refusal('ADM-135', 'admind could not answer', 500) } } });
  assert.match(text(page), /SSH's settings could not be read\./);
  assert.equal(page.problems.length, 1);
});

// ---- the switch ---------------------------------------------------------------------------

test('turning SSH on runs ssh.on at once, since it takes nothing away, and reads the settings again', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/ssh': ssh({ on: false }) }, answers: { 'ssh.on': OK } } });
  const before = page.calls.filter((path) => path === '/api/1/ssh').length;
  control(page.card('ssh'), 'ssh-switch').click();
  await settle();
  const [call] = page.ranWith('ssh.on');
  assert.deepEqual(call[1], {});
  assert.equal(call[2].words.done, 'SSH is on.');
  assert.equal(page.calls.filter((path) => path === '/api/1/ssh').length, before + 1);
  assert.equal(page.dialog(), null);
});

test('turning SSH off opens the form with SSH off, where it can only be tried', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  control(page.card('ssh'), 'ssh-switch').click();
  await settle();
  const dialog = page.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Change SSH\'s settings');
  assert.equal(fieldOf(dialog, 'SSH').checked, false);
  assert.equal(buttonOf(dialog, 'Apply').hidden, true, 'nothing that takes a way in is applied at once');
  assert.equal(buttonOf(dialog, 'Try it').hasAttribute('disabled'), false);
  assert.match(dialog.textContent, /SSH stops\. Nobody can sign in over SSH\./);
  assert.equal(page.ran('ssh.off').length, 0, 'the switch asks first');
});

// ---- the form ------------------------------------------------------------------------------

test('the form starts on the settings now and offers nothing to do until something changes', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const dialog = await openForm(page);
  assert.equal(fieldOf(dialog, 'SSH').checked, true);
  assert.equal(fieldOf(dialog, 'Passwords sign in').checked, true);
  assert.equal(fieldOf(dialog, 'Keys sign in').checked, true);
  assert.equal(buttonOf(dialog, 'Apply').hasAttribute('disabled'), true);
  assert.equal(buttonOf(dialog, 'Try it').hasAttribute('disabled'), true);
});

test('passwords off takes a way in: only Try it is offered, with a window of 5 minutes chosen, and it runs ssh.set with that try', async (context) => {
  const { world } = trialWorld();
  const page = await openSecurity(context, { unlocked: true, world });
  const dialog = await openForm(page);
  tick(fieldOf(dialog, 'Passwords sign in'), false);
  assert.equal(buttonOf(dialog, 'Apply').hidden, true);
  assert.equal(buttonOf(dialog, 'Try it').hasAttribute('disabled'), false);
  assert.match(dialog.textContent, /Passwords stop signing in over SSH\./);
  assert.match(dialog.textContent, /it is undone unless you keep it/);
  const window = fieldOf(dialog, 'Undo it after');
  assert.equal(window.getAttribute('id'), 'sec-ssh-window');
  assert.deepEqual(window.querySelectorAll('option').map((one) => one.getAttribute('value')), ['120', '300', '600']);
  assert.equal(window.querySelectorAll('option').find((one) => one.hasAttribute('selected')).getAttribute('value'), '300');
  buttonOf(dialog, 'Try it').click();
  await settle();
  const [call] = page.ranWith('ssh.set');
  assert.deepEqual(call[1], { passwords: 'no', try: 300 });
  assert.equal(call[2].words.done, 'SSH\'s settings are being tried.');
  assert.equal(page.dialog(), null);
});

test('with sshd on but not running, nobody is on SSH to cut off: the change is applied, with no try', async (context) => {
  const { world } = trialWorld();
  const page = await openSecurity(context, { unlocked: true, world: { ...world, reads: { ...world.reads, '/api/1/ssh': ssh({ running: false }) } } });
  const dialog = await openForm(page);
  tick(fieldOf(dialog, 'Passwords sign in'), false);
  assert.equal(buttonOf(dialog, 'Try it').hidden, true);
  assert.equal(buttonOf(dialog, 'Apply').hidden, false);
  assert.equal(fieldOf(dialog, 'Undo it after').closest('.field').hidden, true);
  assert.match(dialog.textContent, /SSH is not running now, so no connection over it can be cut off: the change is applied\./);
  buttonOf(dialog, 'Apply').click();
  await settle();
  assert.deepEqual(page.ranWith('ssh.set')[0][1], { passwords: 'no' });
});

test('a window that is chosen is the try that is sent', async (context) => {
  const { world } = trialWorld();
  const page = await openSecurity(context, { unlocked: true, world });
  const dialog = await openForm(page);
  tick(fieldOf(dialog, 'Keys sign in'), false);
  const window = fieldOf(dialog, 'Undo it after');
  window.value = '600';
  buttonOf(dialog, 'Try it').click();
  await settle();
  assert.deepEqual(page.ran('ssh.set'), [{ keys: 'no', try: 600 }]);
});

test('a change that only adds a way in can be applied at once, or tried', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/ssh': ssh({ passwords: false }) }, answers: { 'ssh.set': OK } } });
  const dialog = await openForm(page);
  tick(fieldOf(dialog, 'Passwords sign in'), true);
  assert.equal(buttonOf(dialog, 'Apply').hidden, false);
  assert.equal(buttonOf(dialog, 'Apply').hasAttribute('disabled'), false);
  assert.match(dialog.textContent, /This only adds ways in\. Apply it, or try it first\./);
  buttonOf(dialog, 'Apply').click();
  await settle();
  const [call] = page.ranWith('ssh.set');
  assert.deepEqual(call[1], { passwords: 'yes' });
  assert.equal(call[2].words.done, 'SSH\'s settings are changed.');
  assert.equal(page.dialog(), null);
  assert.equal(control(page.card('ssh'), 'ssh-change').getAttribute('aria-disabled'), 'false', 'nothing was tried');
});

test('neither passwords nor keys is refused in words, and nothing can be sent', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const dialog = await openForm(page);
  tick(fieldOf(dialog, 'Passwords sign in'), false);
  tick(fieldOf(dialog, 'Keys sign in'), false);
  assert.match(dialog.textContent, /Nobody could sign in over SSH\. Turn on passwords, keys or both\./);
  assert.equal(buttonOf(dialog, 'Try it').hasAttribute('disabled'), true);
  assert.equal(buttonOf(dialog, 'Apply').hasAttribute('disabled'), true);
});

test('who may sign in: these accounts shows the chips, none chosen is refused, and a name is a try that sends the list', async (context) => {
  const { world } = trialWorld();
  const page = await openSecurity(context, { unlocked: true, world });
  const dialog = await openForm(page);
  choose(dialog, 'users');
  assert.match(dialog.textContent, /Choose at least one account, or let everyone sign in\./);
  assert.equal(buttonOf(dialog, 'Try it').hasAttribute('disabled'), true);
  tick(fieldOf(dialog, 'alice'), true);
  assert.equal(buttonOf(dialog, 'Try it').hasAttribute('disabled'), false);
  assert.match(dialog.textContent, /Over SSH, only alice may sign in\./);
  buttonOf(dialog, 'Try it').click();
  await settle();
  assert.deepEqual(page.ran('ssh.set'), [{ allow_users: 'alice', try: 300 }]);
});

test('a list the file has that names an account the machine no longer has keeps that name in the form', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/ssh': ssh({ allow_users: ['gone'] }) } } });
  const dialog = await openForm(page);
  assert.equal(fieldOf(dialog, 'gone').checked, true);
});

test('the finding about passwords opens the form with passwords off, and the one about who with the accounts\' choice', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  buttonOf(page.card('checks'), 'Keys only…').click();
  await settle();
  const dialog = page.dialog();
  assert.equal(fieldOf(dialog, 'Passwords sign in').checked, false);
  assert.equal(fieldOf(dialog, 'Keys sign in').checked, true);
  assert.match(dialog.textContent, /Passwords stop signing in over SSH\./);
});

test('a locked console is asked to unlock first, and a refused change stays in the form', async (context) => {
  const locked = await openSecurity(context, { unlocked: false });
  control(locked.card('ssh'), 'ssh-change').click();
  await settle();
  assert.match(locked.dialog().textContent, /change SSH's settings/);
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'ssh.set': refusal('NET-183', 'sshd_config has a mistake', 409) } } });
  const dialog = await openForm(page);
  tick(fieldOf(dialog, 'Passwords sign in'), false);
  buttonOf(dialog, 'Try it').click();
  await settle();
  assert.equal(page.dialog(), dialog);
  assert.match(dialog.querySelector('.sec-errors').textContent, /sshd_config has a mistake/);
  assert.equal(page.card('ssh').querySelectorAll('.try-box').filter((one) => !one.hasAttribute('hidden')).length, 0, 'no trial began');
});

test('turning SSH off from the form is a try of ssh.off', async (context) => {
  const { world } = trialWorld();
  const page = await openSecurity(context, { unlocked: true, world });
  control(page.card('ssh'), 'ssh-switch').click();
  await settle();
  buttonOf(page.dialog(), 'Try it').click();
  await settle();
  assert.deepEqual(page.ran('ssh.off'), [{ try: 300 }]);
});

test('turning SSH on and narrowing in one change runs ssh.on first and then the try', async (context) => {
  const { world } = trialWorld({ answers: { 'ssh.on': OK } });
  const page = await openSecurity(context, { unlocked: true, world: { ...world, reads: { ...world.reads, '/api/1/ssh': ssh({ on: false }) } } });
  const dialog = await openForm(page);
  tick(fieldOf(dialog, 'SSH'), true);
  tick(fieldOf(dialog, 'Passwords sign in'), false);
  buttonOf(dialog, 'Apply').click();
  await settle();
  assert.deepEqual(page.allRan().filter((action) => action.startsWith('ssh.')), ['ssh.on', 'ssh.set']);
});

// ---- the trial ------------------------------------------------------------------------------

async function beginTrial(context, more = {}) {
  const { world, state } = trialWorld(more);
  const page = await openSecurity(context, { unlocked: true, ...more.open, world });
  const dialog = await openForm(page);
  tick(fieldOf(dialog, 'Passwords sign in'), false);
  buttonOf(dialog, 'Try it').click();
  await settle();
  return { page, state };
}

test('a try shows the banner at the top of the card, with what to check and the command to type', async (context) => {
  const { page } = await beginTrial(context);
  const card = page.card('ssh');
  const check = card.querySelector('.sec-ssh-check');
  assert.equal(check.hasAttribute('hidden'), false);
  assert.match(check.textContent, /Open a new SSH connection from your computer now: ssh chase@duke-lab\.local\. Connections that were open stay open, so only a new one tells\./);
  assert.match(card.querySelector('.try-words').textContent, /The change is undone at .* unless it is kept\./);
  assert.equal(card.querySelector('.try-box').hasAttribute('hidden'), false);
  assert.equal(control(card, 'ssh-change').getAttribute('aria-disabled'), 'true', 'a second change waits for the first');
  assert.equal(control(card, 'ssh-switch').getAttribute('aria-disabled'), 'true');
});

test('the trial is one that the browser may keep at once: SSH has no browser to be cut off from', async (context) => {
  const { page } = await beginTrial(context);
  assert.equal(page.calls.includes('/api/1/console/fresh/ssh'), false, 'the console has no such route; the card answers it');
  const keep = buttonOf(page.card('ssh'), 'Keep this change');
  assert.equal(keep.getAttribute('aria-disabled'), 'false');
  keep.click();
  await settle();
  assert.deepEqual(page.ran('ssh.keep'), [{}]);
  assert.equal(page.card('ssh').querySelector('.sec-ssh-check').hasAttribute('hidden'), true);
});

test('Undo now runs ssh.undo and the check goes away', async (context) => {
  const { page } = await beginTrial(context);
  buttonOf(page.card('ssh'), 'Undo now').click();
  await settle();
  assert.deepEqual(page.ran('ssh.undo'), [{}]);
  assert.equal(page.card('ssh').querySelector('.sec-ssh-check').hasAttribute('hidden'), true);
  assert.match(page.card('ssh').textContent, /The change was undone: you undid it\./);
});

test('the sign-ins over SSH since the try are listed, read again every 5 seconds, and counted under Keep', async (context) => {
  const lasts = [[]];
  const { page } = await beginTrial(context, { reads: { '/api/1/sign-ins/last': () => lasts[0] } });
  assert.match(page.card('ssh').querySelector('.sec-ssh-check').textContent, /Nobody has signed in over SSH since the change\./);
  lasts[0] = lastlog();
  await page.tick(5000);
  const check = page.card('ssh').querySelector('.sec-ssh-check');
  assert.match(check.textContent, /owner signed in over SSH at 10:20 AM from 192\.168\.1\.25/);
  assert.match(check.textContent, /1 account signed in over SSH since the change\./);
  assert.doesNotMatch(check.textContent, /alice|sam/, 'a sign-in before the try or over the web is not one');
});

test('a try that the journal dates earlier than this page saw it counts the sign-ins from then', async (context) => {
  const early = [{ name: 'alice', sign_in: { at_ms: at(2026, 11, 14, 9, 30), terminal: 'pts/0', from: '10.8.0.2', via: 'ssh' }, failed: 0, last_failed: null }];
  const row = { action: 'ssh.set', status: 0, argv: ['--try', '300'], at: Math.floor(at(2026, 11, 14, 9, 0) / 1000) };
  const dated = await beginTrial(context, { reads: { '/api/1/changes?limit=100': { changes: [row] }, '/api/1/sign-ins/last': early } });
  assert.match(dated.page.card('ssh').querySelector('.sec-ssh-check').textContent, /alice signed in over SSH at 9:30 AM from 10\.8\.0\.2/);
  const undated = await beginTrial(context, { reads: { '/api/1/sign-ins/last': early } });
  assert.match(undated.page.card('ssh').querySelector('.sec-ssh-check').textContent, /Nobody has signed in over SSH since the change\./);
});

test('a locked console is asked to unlock to see the sign-ins, and a reader is told who sees them', async (context) => {
  const state = { spark: sparkWith('ssh') };
  const locked = await openSecurity(context, { unlocked: false, world: { reads: { '/api/1/services': () => state.spark, '/api/1/ssh': ssh({ trial: { left_s: 100 } }) } } });
  assert.match(locked.card('ssh').querySelector('.sec-ssh-check').textContent, /Unlock to see who signs in over SSH since the change\./);
  assert.equal(locked.calls.includes('/api/1/sign-ins/last'), false);
  const reader = await openSecurity(context, { session: { admin: false }, world: { reads: { '/api/1/services': () => state.spark, '/api/1/ssh': ssh({ trial: { left_s: 100 } }) } } });
  assert.match(reader.card('ssh').querySelector('.sec-ssh-check').textContent, /Administrators see who signs in over SSH since the change\./);
});

test('a try that another session began is shown on load, and its Keep is the person\'s to press', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/services': sparkWith('ssh'), '/api/1/ssh': ssh({ trial: { left_s: 100 } }) } } });
  assert.equal(page.card('ssh').querySelector('.sec-ssh-check').hasAttribute('hidden'), false);
  assert.equal(control(page.card('ssh'), 'ssh-change').getAttribute('aria-disabled'), 'true');
  buttonOf(page.card('ssh'), 'Keep this change').click();
  await settle();
  assert.deepEqual(page.ran('ssh.keep'), [{}]);
});

test('a try that ends on its own is shown as ended and the form is on again', async (context) => {
  const { page, state } = await beginTrial(context);
  state.spark = { ...sparkIdle(), undone: [{ id: 'ssh', why: 'the time ran out' }] };
  await page.tick(5000);
  assert.equal(page.card('ssh').querySelector('.sec-ssh-check').hasAttribute('hidden'), true);
  assert.equal(control(page.card('ssh'), 'ssh-change').getAttribute('aria-disabled'), 'false');
});
