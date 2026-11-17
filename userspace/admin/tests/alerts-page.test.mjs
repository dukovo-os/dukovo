// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Alerts page on a fake document: what each card says for an
// administrator, a locked one and a console whose alertd is not running, and
// which action each button runs.

import assert from 'node:assert/strict';
import test from 'node:test';

import { emailChannel, ntfyChannel, pushChannel, tested } from './alerts-fixtures.mjs';
import {
  openAlerts, refusal, useStorage,
} from './alerts-view.mjs';

/** What `node` says, a space between its parts as the page's layout puts. */
const words = (node) => (node.children ? node.children.map(words).join(' ') : node.text);
const text = (root) => words(root).replace(/\s+/g, ' ').trim();

test('an administrator sees the page named, with its lead and the test button', async (context) => {
  useStorage(context);
  const world = await openAlerts(context);
  assert.equal(world.view.headed.title, 'Alerts');
  assert.equal(world.view.headed.commands, true);
  assert.match(world.view.headed.lead, /One message per change, with this server's name/);
  assert.equal(world.view.headed.buttons[0].textContent, 'Send a test alert');
  for (const id of ['active', 'browser', 'channels', 'email', 'recent', 'phone']) assert.ok(world.card(id), id);
  assert.deepEqual(world.calls.slice(0, 3).sort(), ['/api/1/alerts', '/api/1/alerts/channels', '/api/1/alerts/history']);
});

test('Active lists the critical alert first with its page, and the others after', async (context) => {
  useStorage(context);
  const world = await openAlerts(context);
  const card = text(world.card('active'));
  assert.match(card, /Critical A disk is failing since Yesterday 10:02 AM · diskhealth Copy its data off\. Open Storage/);
  assert.ok(card.indexOf('A disk is failing') < card.indexOf('/users is 93% full'));
  const links = world.card('active').querySelectorAll('a').map((one) => one.getAttribute('href'));
  assert.deepEqual(links, ['/storage', '/storage']);
});

test('Active says that no alert is active', async (context) => {
  useStorage(context);
  const world = await openAlerts(context, { world: { reads: { '/api/1/alerts': [] } } });
  assert.match(text(world.card('active')), /No alerts are active\./);
});

test('Recent lists the last changes and links to the rest', async (context) => {
  useStorage(context);
  const world = await openAlerts(context);
  const card = text(world.card('recent'));
  assert.match(card, /\/data is 93% full Warning · Cleared · 9:30 AM · metricsd/);
  assert.match(card, /A test alert Warning · Test · 7:20 AM · alertd/);
  assert.equal(world.card('recent').querySelectorAll('a').at(-1).getAttribute('href'), '/reliability#alerts');
});

test('Where alerts go has the screens first and a row for each channel with what it is told and how it did', async (context) => {
  useStorage(context);
  const world = await openAlerts(context);
  const card = text(world.card('channels'));
  assert.match(card, /On this server's screens and at the next sign-in/);
  assert.match(card, /email to ops@example\.org and 1 more Told of: every kind · every alert, and when one clears Last sent 7:20 AM/);
  assert.match(card, /ntfy \(ntfy\.sh\) Told of: data at risk, something stopped · critical only Last failed 2:00 AM: ntfy answered 403 Forbidden: forbidden ALR-102 2 waiting/);
  const failed = world.card('channels').querySelectorAll('li').find((one) => one.classes.has('bad'));
  assert.ok(failed, 'the failing row is in the error colour');
  assert.ok(buttonOfRow(failed, 'Send a test alert'));
  assert.ok(buttonOfRow(failed, 'Edit what it is told'));
  assert.ok(buttonOfRow(failed, 'Remove'));
});

function buttonOfRow(row, words) {
  return row.querySelectorAll('button').find((one) => one.textContent.trim() === words);
}

test('a kind that no channel is told of is one warning line, with Edit on the email row', async (context) => {
  useStorage(context);
  const world = await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': [emailChannel({ about: ['data', 'stopped'] }), ntfyChannel()] } } });
  assert.match(text(world.card('channels')), /Hardware alerts reach no channel: only this server's screens\./);
  assert.ok(world.card('channels').querySelector('.banner').querySelectorAll('button').some((one) => one.textContent === 'Edit what email is told'));
  const every = await openAlerts(context);
  assert.doesNotMatch(text(every.card('channels')), /reach no channel/, 'email is told of every kind');
  const covered = await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': [pushChannel({ about: null })] } } });
  assert.doesNotMatch(text(covered.card('channels')), /reach no channel/);
});

test('with no channel the card says so, and the email form is there to fill', async (context) => {
  useStorage(context);
  const world = await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': [] } } });
  assert.match(text(world.card('channels')), /Alerts reach this server's screens only\./);
  assert.match(text(world.card('email')), /Send to/);
  assert.ok(world.card('email').querySelector('textarea'));
});

test('Send a test alert shows each channel\'s line, and the rows are read again', async (context) => {
  useStorage(context);
  const world = await openAlerts(context, {
    world: {
      answers: {
        'alerts.test': { ok: true, result: tested([{ channel: 'email', ok: true, error: null, error_id: null }, { channel: 'ntfy (ntfy.sh)', ok: false, error: 'ntfy answered 403 Forbidden: forbidden', error_id: 'ALR-102' }]) },
      },
    },
  });
  const before = world.calls.filter((path) => path === '/api/1/alerts/channels').length;
  world.view.headed.buttons[0].click();
  await world.emit('none');
  const lines = world.card('channels').querySelectorAll('.al-test li').map((one) => text(one));
  assert.deepEqual(lines, ['email: sent', 'ntfy (ntfy.sh): failed: ntfy answered 403 Forbidden: forbidden ALR-102']);
  assert.ok(world.card('channels').querySelector('.al-test li').classes.has('al-ok'));
  // A list that is said as it comes: a role would take the list's away from its items.
  const results = world.card('channels').querySelector('.al-test');
  assert.equal(results.tag, 'ul');
  assert.equal(results.getAttribute('role'), null);
  assert.equal(results.getAttribute('aria-live'), 'polite');
  assert.ok(world.calls.filter((path) => path === '/api/1/alerts/channels').length > before);
  assert.ok(world.ran('alerts.test').length >= 1);
});

test('a test with no channel says that alerts go to the screens only', async (context) => {
  useStorage(context);
  const world = await openAlerts(context, { world: { reads: { '/api/1/alerts/channels': [] }, answers: { 'alerts.test': { ok: true, result: tested([]) } } } });
  world.view.headed.buttons[0].click();
  await world.emit('none');
  assert.match(text(world.card('channels').querySelector('.al-test')), /No channel is set up: alerts go to this server's screens only\./);
});

test('a locked console gives one line that says to unlock, and no card', async (context) => {
  useStorage(context);
  const world = await openAlerts(context, { unlocked: false });
  assert.match(text(world.view.main), /Unlock to see the alerts\./);
  assert.equal(world.card('active').parent.hidden, true);
  assert.deepEqual(world.calls.filter((path) => path.startsWith('/api/1/alerts')), [], 'no administrator read is made while the console is locked');
});

test('the console being unlocked reads what was not read', async (context) => {
  useStorage(context);
  const world = await openAlerts(context, { unlocked: false });
  const { setUnlockSeconds } = await import('../unlock.js');
  setUnlockSeconds(900);
  await world.emit('unlock');
  assert.equal(world.card('active').parent.hidden, false);
  assert.match(text(world.card('active')), /A disk is failing/);
});

test('alertd that is not running is said with the command that starts it', async (context) => {
  useStorage(context);
  const down = refusal('ADM-120', 'The command failed', 500, { command: { argv: ['alerts', 'status'], status: 1, stderr: 'alerts: alertd is not running; try service alertd start' } });
  const world = await openAlerts(context, { world: { reads: { '/api/1/alerts': down, '/api/1/alerts/channels': down, '/api/1/alerts/history': down } } });
  const banner = text(world.view.main.querySelector('.banner'));
  assert.match(banner, /alertd is not running, so nothing is told: elevate service alertd start/);
  assert.equal(world.view.main.querySelector('.banner a').getAttribute('href'), '/services/alertd');
  assert.match(text(world.card('email')), /Send to/, 'email can be set up all the same');
});

test('Remove asks first, and then runs the action of the channel\'s kind', async (context) => {
  useStorage(context);
  const world = await openAlerts(context);
  const row = world.card('channels').querySelectorAll('li').find((one) => text(one).startsWith('ntfy'));
  buttonOfRow(row, 'Remove').click();
  await world.emit('none');
  assert.match(text(world.dialog()), /Remove ntfy \(ntfy\.sh\)\?/);
  world.dialog().querySelector('form').fire('submit');
  await world.emit('none');
  assert.deepEqual(world.ran('alerts.webhook-remove'), [{ number: 2 }]);
});

test('Edit what it is told saves the kinds that are checked', async (context) => {
  useStorage(context);
  const world = await openAlerts(context);
  const row = world.card('channels').querySelectorAll('li').find((one) => text(one).startsWith('ntfy'));
  buttonOfRow(row, 'Edit what it is told').click();
  await world.emit('none');
  const boxes = world.dialog().querySelectorAll('input');
  assert.deepEqual(boxes.map((one) => one.checked), [true, true, false]);
  boxes[2].checked = true;
  boxes[0].checked = false;
  world.dialog().querySelector('form').fire('submit');
  await world.emit('none');
  assert.deepEqual(world.ran('alerts.channel-about'), [{ channel: 'webhook:2', about: 'stopped,hardware' }]);
});

test('a reader is told that alerts are for administrators', async (context) => {
  useStorage(context);
  const page = (await import('./fake-dom.mjs')).usePage(context);
  const { reader } = await import('../pages/alerts/page.js');
  const main = page.document.createElement('main');
  let headed = null;
  await reader({ main, head: (options) => { headed = options; }, session: { name: 'sam', admin: false } });
  assert.equal(headed.title, 'Alerts');
  assert.match(text(main), /Alerts are for administrators\. sam's account can look but not change\./);
  assert.equal(main.querySelector('a').getAttribute('href'), '/accounts');
});
