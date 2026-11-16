// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The score and the checks of the Security page (design 8.3 and 8.9): the ring
// and its words, each finding's buttons, Check again, and the states of a card
// that cannot show a score.

import assert from 'node:assert/strict';
import test from 'node:test';

import { check, kept } from './security-fixtures.mjs';
import {
  buttonOf, control, openSecurity, refusal, settle,
} from './security-view.mjs';

const text = (page, id = 'checks') => page.card(id).textContent;

test('the card shows the ring, how many checks pass, when they ran and the findings with their points', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const card = page.card('checks');
  const ring = card.querySelector('[role="img"]');
  assert.match(ring.getAttribute('aria-label'), /^Health score \d+ of 100: /);
  assert.match(text(page), /26 of 29 checks pass, 1 skipped/);
  assert.match(text(page), /Checked today at 5:17 AM/);
  const finds = card.querySelectorAll('li').filter((one) => one.classes.has('sec-find'));
  assert.equal(finds.length, 3);
  assert.match(finds[0].textContent, /alice has no password/);
  assert.match(finds[0].textContent, /takes 20 points/);
  assert.equal(finds[0].querySelector('a.mono').getAttribute('href'), '/help/errors/SEC-120');
  assert.equal(ring.style.getPropertyValue('--v'), ring.getAttribute('aria-label').match(/score (\d+)/)[1]);
});

test('each finding has the button that design 8.3 gives it', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const card = page.card('checks');
  const set = card.querySelectorAll('a').find((one) => one.textContent === 'Set a password…');
  assert.equal(set.getAttribute('href'), '/accounts/alice');
  assert.ok(buttonOf(card, 'Keys only…'), 'passwords over SSH offer the SSH form');
  assert.ok(buttonOf(card, 'Show how'), 'a check with no page of its own says how, from a terminal');
});

test('the findings of a machine whose firewall is off offer to start it, and run service.start', async (context) => {
  const page = await openSecurity(context, {
    unlocked: true,
    world: { reads: { '/api/1/checks/last': kept({ results: [check('SEC-102', 'warning', { detail: 'The firewall is not running' })], counts: { passed: 0, warning: 1, failed: 0, skipped: 0 } }) } },
  });
  await buttonOf(page.card('checks'), 'Start the firewall').click();
  await settle();
  const [call] = page.ranWith('service.start');
  assert.deepEqual(call[1], { name: 'firewall' });
  assert.equal(call[2].words.unlock, 'start the firewall');
  assert.match(call[2].words.done, /Check again to update the score/);
});

test('Show how opens the advice with a command to copy, and a second press closes it', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const card = page.card('checks');
  const how = buttonOf(card, 'Show how');
  const box = card.querySelector('.sec-how');
  assert.equal(box.hasAttribute('hidden'), true);
  how.click();
  assert.equal(box.hasAttribute('hidden'), false);
  assert.equal(how.getAttribute('aria-expanded'), 'true');
  assert.equal(box.querySelector('pre').textContent, 'elevate update apply');
  how.click();
  assert.equal(box.hasAttribute('hidden'), true);
});

test('the checks that passed and were skipped are behind a button', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const card = page.card('checks');
  const list = card.querySelector('.sec-passed');
  assert.equal(list.hasAttribute('hidden'), true);
  const open = buttonOf(card, 'Show the 26 that pass and 1 skipped');
  open.click();
  assert.equal(list.hasAttribute('hidden'), false);
  assert.equal(list.children.length, 27);
  assert.match(list.textContent, /Skipped check SEC-140/);
});

test('a reader is told that the score is for administrators, and nothing is read for them', async (context) => {
  // dukovo-check keeps its result for administrators, so a reader's read would be refused.
  const page = await openSecurity(context, { session: { admin: false } });
  const card = page.card('checks');
  assert.equal(control(card, 'check-again').hasAttribute('hidden'), true);
  assert.equal(card.querySelectorAll('button').filter((one) => !one.hasAttribute('hidden')).length, 0);
  assert.match(card.textContent, /The health score is for administrators\./);
  assert.doesNotMatch(card.textContent, /alice has no password|have not run/);
  assert.equal(page.calls.some((call) => String(call).includes('/api/1/checks/last')), false);
});

test('Check again runs the checks, shows the result that was kept and says what it did', async (context) => {
  const clean = kept({ results: [check('SEC-900', 'passed')], counts: { passed: 1, warning: 0, failed: 0, skipped: 0 } });
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'checks.run': { ok: true, result: clean } } } });
  const again = control(page.card('checks'), 'check-again');
  assert.equal(again.hasAttribute('hidden'), false);
  again.click();
  await settle();
  const [call] = page.ranWith('checks.run');
  assert.deepEqual(call[1], {});
  assert.equal(call[2].button, again);
  assert.equal(call[2].words.done, 'The checks ran.');
  assert.match(text(page), /All 1 checks pass/);
  assert.match(text(page), /Nothing needs you/);
});

test('Check again reads the kept result when the answer has no results', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'checks.run': { ok: true, result: {} } } } });
  const before = page.calls.filter((path) => path === '/api/1/checks/last').length;
  control(page.card('checks'), 'check-again').click();
  await settle();
  assert.equal(page.calls.filter((path) => path === '/api/1/checks/last').length, before + 1);
});

test('a refused run leaves the findings as they were and the refusal under the title', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { answers: { 'checks.run': refusal('ADM-130', 'The checks could not run') } } });
  control(page.card('checks'), 'check-again').click();
  await settle();
  assert.match(text(page), /alice has no password/);
  assert.match(page.card('checks').querySelector('.sec-errors').textContent, /The checks could not run/);
});

test('a server without the checks says so, and offers no way to run them', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/checks/last': refusal('ADM-129', 'No such command', 404) } } });
  assert.match(text(page), /Checks are not on this server yet/);
  assert.equal(control(page.card('checks'), 'check-again').hasAttribute('hidden'), true);
  assert.equal(page.problems.length, 0);
  assert.equal(page.card('checks').hasAttribute('aria-busy'), false);
});

test('a machine that was never checked says so and offers Check again', async (context) => {
  for (const answer of [refusal('ADM-123', 'No result kept', 404), { version: 1, results: [] }]) {
    const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/checks/last': answer } } });
    assert.match(text(page), /The checks have not run on this server yet\. Use Check again to run them\./);
    assert.equal(control(page.card('checks'), 'check-again').hasAttribute('hidden'), false);
  }
});

test('a read that fails says so under the title and keeps the problem for the person to open', async (context) => {
  const page = await openSecurity(context, { unlocked: true, world: { reads: { '/api/1/checks/last': refusal('ADM-135', 'admind could not answer', 500) } } });
  assert.match(text(page), /The checks could not be read\./);
  assert.equal(page.problems.length, 1);
  assert.equal(page.problems[0][1].slot, page.card('checks').querySelector('.sec-errors'));
});

test('a change that the console hears of reads the kept result again', async (context) => {
  const page = await openSecurity(context, { unlocked: true });
  const before = page.calls.filter((path) => path === '/api/1/checks/last').length;
  page.emit('change');
  await settle();
  assert.equal(page.calls.filter((path) => path === '/api/1/checks/last').length, before + 1);
});
