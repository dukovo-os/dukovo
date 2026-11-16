// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Alerts card of the Security page: a row for each kind of alert with the
// channels it goes to, the line for a server with no channel, a test alert
// with what it did, the words for a reader and a locked console, and the
// button that the finding SEC-170 has.

import assert from 'node:assert/strict';
import test from 'node:test';

import { fixesFor } from '../pages/security/model.js';
import { emailChannel, ntfyChannel, tested } from './alerts-fixtures.mjs';
import { NOW, time } from './security-fixtures.mjs';
import {
  buttonOf, openSecurity, refusal, settle,
} from './security-view.mjs';

const text = (page) => page.card('alerts').textContent;
/** The rows of the card: the kind and the words that follow it. */
const rowsOf = (page) => {
  const card = page.card('alerts');
  const terms = card.querySelectorAll('dt');
  return card.querySelectorAll('dd').map((one, at) => [terms[at].textContent, one.textContent]);
};

/** Lets the page load what a press asks for, which is more than a promise's turn. */
const loaded = async () => {
  await new Promise((resolve) => setTimeout(resolve, 50));
  await settle();
};
const CHANNELS = { ok: true, result: [emailChannel(), ntfyChannel()] };
const SOME = { 'alerts.channels': CHANNELS };

test('each kind of alert has a row with the channels it goes to and what each tells', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: SOME } });
  assert.deepEqual(rowsOf(page), [
    ['Data at risk', 'email and ntfy (ntfy.sh) (critical only)'],
    ['Something stopped', 'email and ntfy (ntfy.sh) (critical only)'],
    ['Hardware', 'email'],
  ]);
  assert.equal(page.ran('alerts.channels').length, 1);
});

test('a kind that no channel is told of is a warning, in words as well as colour', async (context) => {
  const only = { ok: true, result: [ntfyChannel({ about: ['data'] })] };
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'alerts.channels': only } } });
  const dd = page.card('alerts').querySelectorAll('dd');
  assert.equal(dd[0].getAttribute('class'), null);
  assert.equal(dd[1].getAttribute('class'), 'warn');
  assert.equal(dd[1].textContent, 'no channel');
  assert.ok(dd[1].querySelector('.ic-warn'));
  assert.equal(rowsOf(page).length, 3);
});

test('a server with no channel says that alerts reach its screens only, and offers to set them up', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'alerts.channels': { ok: true, result: [] } } } });
  assert.match(text(page), /Alerts reach this server's screens only\./);
  const link = page.card('alerts').querySelectorAll('a').find((one) => one.textContent === 'Set up alerts');
  assert.equal(link.getAttribute('href'), '/alerts');
  assert.equal(buttonOf(page.card('alerts'), 'Send a test alert'), undefined);
  assert.equal(page.card('alerts').querySelectorAll('dt').length, 0);
});

test('Alert settings opens the Alerts page', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: SOME } });
  const link = page.card('alerts').querySelectorAll('a').find((one) => one.textContent === 'Alert settings');
  assert.equal(link.getAttribute('href'), '/alerts');
});

test('Send a test alert says which channels took it, and when', async (context) => {
  const page = await openSecurity(context, {
    unlocked: true,
    world: { answers: { ...SOME, 'alerts.test': { ok: true, result: tested([{ channel: 'email', ok: true, error: null, error_id: null }, { channel: 'ntfy (ntfy.sh)', ok: true, error: null, error_id: null }]) } } },
  });
  const button = buttonOf(page.card('alerts'), 'Send a test alert');
  button.click();
  await loaded();
  assert.deepEqual(page.ran('alerts.test'), [{}]);
  const said = page.card('alerts').querySelector('[role="status"]');
  assert.equal(said.textContent, `Last test reached email and ntfy (ntfy.sh) at ${time.clock(Math.floor(NOW / 1000))}.`);
  assert.match(said.getAttribute('class'), /\bok\b/);
});

test('a channel that refused the test is named, as a warning', async (context) => {
  const page = await openSecurity(context, {
    unlocked: true,
    world: { answers: { ...SOME, 'alerts.test': { ok: true, result: tested([{ channel: 'email', ok: true, error: null, error_id: null }, { channel: 'ntfy (ntfy.sh)', ok: false, error: 'ntfy answered 403 Forbidden', error_id: 'ALR-102' }]) } } },
  });
  buttonOf(page.card('alerts'), 'Send a test alert').click();
  await loaded();
  const said = page.card('alerts').querySelector('[role="status"]');
  assert.match(said.textContent, /Last test reached email at .*\. It failed at ntfy \(ntfy\.sh\)\./);
  assert.match(said.getAttribute('class'), /\bwarn\b/);
});

test('a test that gives no deliveries still says that it was sent', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { ...SOME, 'alerts.test': { ok: true, result: { done: true, output: ['alerts: test sent'] } } } } });
  buttonOf(page.card('alerts'), 'Send a test alert').click();
  await loaded();
  assert.equal(page.card('alerts').querySelector('[role="status"]').textContent, 'The test alert was sent.');
});

test('a test that fails says nothing of a result, and the problem is in the card', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { ...SOME, 'alerts.test': refusal('ALR-104', 'alertd is not running', 502) } } });
  buttonOf(page.card('alerts'), 'Send a test alert').click();
  await loaded();
  assert.match(page.card('alerts').querySelector('.sec-errors').textContent, /alertd is not running/);
  assert.equal(page.card('alerts').querySelector('[role="status"]').textContent, '');
});

test('a locked administrator is asked to unlock, a reader is told who sees it, and neither reads the channels', async (context) => {
  const locked = await openSecurity(context, { unlocked: false });
  assert.match(text(locked), /Unlock to see where alerts go\./);
  assert.equal(locked.ran('alerts.channels').length, 0);
  const reader = await openSecurity(context, { session: { admin: false } });
  assert.match(text(reader), /Alerts are for administrators\./);
  assert.equal(reader.ran('alerts.channels').length, 0);
});

test('unlocking reads the channels', async (context) => {
  const page = await openSecurity(context, { unlocked: false, world: { answers: SOME } });
  assert.equal(page.ran('alerts.channels').length, 0);
  const { setUnlockSeconds } = await import('../unlock.js');
  setUnlockSeconds(900);
  page.emit('unlock');
  await settle();
  assert.equal(page.ran('alerts.channels').length, 1);
  assert.match(text(page), /Data at risk/);
});

test('a server without the command says that it comes with a later update', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'alerts.channels': refusal('ADM-129', 'No such command', 404) } } });
  assert.match(text(page), /shows where alerts go with a later update of Dukovo/);
});

test('the finding that no alert reached anyone opens the Alerts page', () => {
  assert.deepEqual(fixesFor({ id: 'SEC-170', detail: 'Email failed.', fix: 'Look at where alerts go.' }, () => true), [{ kind: 'link', label: 'Where alerts go', href: '/alerts' }]);
});

test('the card is between the checks and the sign-in rules', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: SOME } });
  const ids = page.view.main.querySelectorAll('section').map((one) => one.getAttribute('id'));
  assert.ok(ids.indexOf('alerts') > ids.indexOf('checks') && ids.indexOf('alerts') < ids.indexOf('sign-in'), ids.join());
});
