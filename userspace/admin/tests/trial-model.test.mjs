// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A tried change (design 8): what Spark's document says of it, the words of
// the countdown, who began it, how it ended, the windows on offer, and the
// machine that asks whether the browser still reaches the console. The
// machine is run against fakes of the call and of the timer. The header's
// chip (C5 design 3.4) has its words here too.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  announceAt, ASKS_AT_ONCE, begunWords, chipWords, clockWords, endedHow, endsWords, heldTrials, leftWords, makeReach, momentWords,
  OVERDUE_MS, percentLeft, reachRound, reachWords, stillShown, trialOf, triedBy, undoneAt, windowsFor,
} from '../lib/trial-model.js';
import { makeTime } from '../lib/time.js';

/** Spark's document with a network trial and the last undone ones. */
const DOCUMENT = {
  now_ms: 700_000,
  services: [],
  trials: [
    { id: 'network', files: ['/system/settings/network'], services: ['netd'], user: 1000, left_ms: 41_000 },
    { id: 'service.nginx', files: [], services: ['nginx'], user: 1000, left_ms: 5000 },
  ],
  undone: [
    { id: 'firewall', why: 'it was not kept', at_ms: 100_000 },
    { id: 'network', why: 'it was not kept', at_ms: 200_000 },
    { id: 'network', why: 'the service failed its check', at_ms: 300_000 },
  ],
};

test('a trial is found in the document by its id, and a document without it has none', () => {
  assert.equal(trialOf(DOCUMENT, 'network').left_ms, 41_000);
  assert.equal(trialOf(DOCUMENT, 'firewall'), null);
  assert.equal(trialOf(DOCUMENT, 'nginx'), null, 'a service\'s trial is service.NAME');
  assert.equal(trialOf({}, 'network'), null);
  assert.equal(trialOf(null, 'network'), null);
  assert.equal(trialOf({ trials: [{ id: 'network' }] }, 'network'), null, 'a trial with no time left is not one');
});

test('what Spark undid is read by the times, and a trial that ended is undone if a new one is among them', () => {
  assert.deepEqual(undoneAt(DOCUMENT, 'network'), [200_000, 300_000]);
  assert.deepEqual(undoneAt({}, 'network'), []);
  assert.deepEqual(endedHow(DOCUMENT, 'network', [200_000]), { how: 'undone', why: 'the service failed its check' });
  assert.deepEqual(endedHow(DOCUMENT, 'network', [200_000, 300_000]), { how: 'kept' });
  assert.deepEqual(endedHow({ undone: [{ id: 'network', at_ms: 1 }] }, 'network', []), { how: 'undone', why: 'it was not kept' });
  assert.deepEqual(endedHow({}, 'network', []), { how: 'kept' });
});

test('the time left is a clock, rounded up, and in words', () => {
  assert.equal(clockWords(287_000), '4:47');
  assert.equal(clockWords(286_100), '4:47');
  assert.equal(clockWords(60_000), '1:00');
  assert.equal(clockWords(9500), '0:10');
  assert.equal(clockWords(0), '0:00');
  assert.equal(clockWords(-5), '0:00');
  assert.equal(leftWords(287_000), '4 minutes 47 seconds');
  assert.equal(leftWords(61_000), '1 minute 1 second');
  assert.equal(leftWords(60_000), '1 minute');
  assert.equal(leftWords(120_000), '2 minutes');
  assert.equal(leftWords(30_000), '30 seconds');
  assert.equal(leftWords(1), '1 second');
  assert.equal(leftWords(0), '0 seconds');
});

test('the meter shows the share of the window left, in whole steps with less motion', () => {
  assert.equal(percentLeft(30_000, 60_000), 50);
  assert.equal(percentLeft(70_000, 60_000), 100);
  assert.equal(percentLeft(-1, 60_000), 0);
  assert.equal(percentLeft(1000, 0), 0);
  assert.equal(percentLeft(47_000, 60_000, true), 80);
  assert.equal(percentLeft(1000, 60_000, true), 10);
  assert.equal(percentLeft(0, 60_000, true), 0);
});

test('the polite region speaks at 1 minute, 30 seconds and 10 seconds, each once the time has come to it', () => {
  assert.equal(announceAt(120_000), null);
  assert.equal(announceAt(60_001), null);
  assert.equal(announceAt(60_000), 60_000);
  assert.equal(announceAt(45_000), 60_000);
  assert.equal(announceAt(30_000), 30_000);
  assert.equal(announceAt(29_000), 30_000);
  assert.equal(announceAt(10_000), 10_000);
  assert.equal(announceAt(1000), 10_000);
  assert.equal(announceAt(0), 10_000);
  assert.equal(momentWords(60_000), '1 minute left to keep the change.');
  assert.equal(momentWords(30_000), '30 seconds left to keep the change.');
  assert.equal(momentWords(10_000), '10 seconds left to keep the change.');
});

test('the windows: an address that is taken away leaves 5 and 10 minutes, a name 2, 5 and 10, anything else all four', () => {
  assert.deepEqual(windowsFor(true, '192.168.1.10'), { choices: [{ seconds: 300, label: '5 minutes' }, { seconds: 600, label: '10 minutes' }], chosen: 300 });
  assert.deepEqual(windowsFor(true, '[2001:db8::1]').choices.map((one) => one.seconds), [300, 600]);
  assert.deepEqual(windowsFor(true, 'duke-nas.local'), {
    choices: [{ seconds: 120, label: '2 minutes' }, { seconds: 300, label: '5 minutes' }, { seconds: 600, label: '10 minutes' }], chosen: 300,
  });
  const all = windowsFor(false, '192.168.1.10');
  assert.deepEqual(all.choices.map((one) => one.label), ['60 seconds', '2 minutes', '5 minutes', '10 minutes']);
  assert.equal(all.chosen, 60);
  assert.equal(windowsFor(false, 'duke-nas.local').chosen, 60);
});

/** A journal row, newest first in the lists below. */
const row = (id, action, over = {}) => ({
  id, at: 1_763_100_000 + id, who: 'alice', how: 'console', action, argv: null, status: 0, files: [], ...over,
});

test('who began a trial is the newest row that tried the file, unless a later change of it says otherwise', () => {
  const tried = row(7, 'network.set', { argv: ['network', 'set', 'net0', 'fixed', '10.0.2.15/24', '--try', '120'] });
  assert.deepEqual(triedBy([tried, row(6, 'service.restart')], 'network'), { who: 'alice', how: 'console', seconds: 120, card: 'net0' });
  assert.equal(triedBy([row(9, 'network.keep'), tried], 'network'), null, 'kept: a trial now running began elsewhere');
  assert.equal(triedBy([row(9, 'network.undo'), tried], 'network'), null);
  assert.equal(triedBy([row(9, null, { how: 'by hand', who: 'outside the console', files: [{ path: '/system/settings/network' }] }), tried], 'network'), null);
  assert.equal(triedBy([row(9, null, { how: 'undone', who: 'Spark', files: [{ path: '/system/settings/network' }] }), tried], 'network'), null);
  assert.deepEqual(triedBy([row(9, null, { how: 'by hand', files: [{ path: '/system/settings/ssh/sshd_config' }] }), tried], 'network')?.seconds, 120, 'a change of another file');
  assert.equal(triedBy([row(8, 'network.set', { argv: ['network', 'set', 'net0', 'dhcp'] }), tried], 'network'), null, 'a change that was not tried');
  assert.equal(triedBy([row(8, 'network.set', { status: 1, argv: ['network', 'set', 'net0', 'dhcp', '--try', '60'] }), tried], 'network')?.seconds, 120, 'a refused try changed nothing');
  assert.equal(triedBy([row(9, 'network.keep', { status: 1 }), tried], 'network')?.seconds, 120, 'a keep that was refused');
  assert.equal(triedBy([], 'network'), null);
  assert.equal(triedBy(undefined, 'firewall'), null);
});

test('the firewall\'s add, replace and remove each begin its trial, and its rows are not the network\'s', () => {
  for (const action of ['firewall.add', 'firewall.replace', 'firewall.remove']) {
    const found = triedBy([row(3, action, { who: 'ci', how: 'token ci', argv: ['firewall', 'allow', 'tcp', '7', '--try', '30'] })], 'firewall');
    assert.deepEqual(found, { who: 'ci', how: 'token ci', seconds: 30, card: null }, action);
  }
  const network = row(3, 'network.set', { argv: ['network', 'set', 'net0', 'dhcp', '--try', '30'] });
  assert.equal(triedBy([network], 'firewall'), null);
  assert.equal(triedBy([row(4, 'firewall.keep'), network], 'network')?.seconds, 30, 'the network trial is not the firewall\'s');
});

test('SSH\'s set and off and the console\'s reach begin their trials, and a change of their files by another action ends what the journal says', () => {
  const set = row(5, 'ssh.set', { argv: ['ssh-server', 'set', 'password', 'no', '--try', '60'] });
  assert.deepEqual(triedBy([set], 'ssh'), { who: 'alice', how: 'console', seconds: 60, card: null });
  const off = row(5, 'ssh.off', { argv: ['ssh-server', 'off', '--try', '300'] });
  assert.equal(triedBy([off], 'ssh')?.seconds, 300);
  assert.equal(triedBy([row(6, 'ssh.on'), set], 'ssh'), null, 'on is never tried');
  assert.equal(triedBy([row(6, null, { how: 'by hand', files: [{ path: '/system/settings/spark/services/sshd.toml' }] }), off], 'ssh'), null);
  const reach = row(5, 'console.reach', { argv: ['admin', 'reach', 'any', '--try', '600'] });
  assert.equal(triedBy([reach], 'console')?.seconds, 600);
  assert.equal(triedBy([row(6, 'security.sign-in', { files: [{ path: '/system/settings/admin.toml' }] }), reach], 'console'), null);
  // The sign-in rules are refused while the reach is tried (ADM-138): a refused row changed nothing.
  assert.equal(triedBy([row(6, 'security.sign-in', { status: 1, files: [{ path: '/system/settings/admin.toml' }] }), reach], 'console')?.seconds, 600);
  assert.equal(triedBy([reach], 'ssh'), null, 'the console\'s trial is not SSH\'s');
});

test('a trial the page has no family for is not said to have begun anywhere', () => {
  const restart = row(5, 'service.restart', { argv: ['service', 'nginx', 'restart', '--try', '60'] });
  assert.equal(triedBy([restart], 'service.nginx'), undefined);
  assert.equal(triedBy([], 'toString'), undefined);
});

test('who began it, in words', () => {
  assert.equal(begunWords(null), 'at a terminal');
  assert.equal(begunWords({ who: 'alice', how: 'console' }), 'by alice in this console');
  assert.equal(begunWords({ who: 'ci', how: 'token deploy' }), 'by ci with the API token deploy');
});

// ---- the reach machine -------------------------------------------------------------

/** A timer the test runs by hand: `schedule(fn, ms)` gives what cancels it. */
function fakeTimers() {
  const waiting = [];
  return {
    waiting,
    schedule(fn, ms) {
      const timer = { fn, ms, live: true };
      waiting.push(timer);
      return () => { timer.live = false; };
    },
    /** Runs the timers that were set and are not cancelled, and gives the delays of those. */
    fire() {
      const due = waiting.splice(0).filter((timer) => timer.live);
      for (const timer of due) timer.fn();
      return due.map((timer) => timer.ms);
    },
  };
}

/** A call that gives the next of `answers`: a value resolves, an error rejects, a function runs. */
function fakeAsk(answers) {
  const calls = { count: 0 };
  const ask = () => {
    calls.count += 1;
    const next = answers.shift();
    if (typeof next === 'function') return next();
    return next instanceof Error ? Promise.reject(next) : Promise.resolve(next);
  };
  return { ask, calls };
}

const failure = (status, id = null) => Object.assign(new Error('x'), { status, id });
const OLD = failure(409, 'ADM-133');
const REACHED = { fresh: true, from: '192.168.1.20', via: { address: '192.168.1.10', port: 9090, card: 'net0' } };
const settle = () => new Promise((resolve) => setImmediate(resolve));

test('a fresh connection answers at once: reached, with where the browser came from', async () => {
  const timers = fakeTimers();
  const { ask } = fakeAsk([REACHED]);
  const seen = [];
  const machine = makeReach({ ask, schedule: timers.schedule, onChange: (state) => seen.push(state.kind) });
  await machine.start();
  assert.deepEqual(seen, ['asking', 'reached']);
  assert.deepEqual(machine.state(), { kind: 'reached', from: '192.168.1.20', via: REACHED.via });
  assert.deepEqual(timers.fire(), [], 'nothing is asked again');
});

test('an old connection is asked again at once until a new one answers', async () => {
  const timers = fakeTimers();
  const { ask, calls } = fakeAsk([OLD, OLD, REACHED]);
  const machine = makeReach({ ask, schedule: timers.schedule });
  await machine.start();
  assert.equal(calls.count, 3);
  assert.equal(machine.state().kind, 'reached');
  assert.equal(ASKS_AT_ONCE, 3);
});

test('after as many old connections as it asks at once it cannot tell yet, and asks again in 3 seconds', async () => {
  const timers = fakeTimers();
  const { ask, calls } = fakeAsk([OLD, OLD, OLD, REACHED]);
  const machine = makeReach({ ask, schedule: timers.schedule });
  await machine.start();
  assert.equal(machine.state().kind, 'unsure');
  assert.equal(calls.count, 3);
  assert.deepEqual(timers.waiting.filter((timer) => timer.live).map((timer) => timer.ms), [3000]);
  timers.fire();
  await settle();
  assert.equal(calls.count, 4);
  assert.equal(machine.state().kind, 'reached');
});

test('a failed ask is "cannot reach": it asks again every 3 seconds until the browser reaches the console', async () => {
  const timers = fakeTimers();
  const { ask } = fakeAsk([failure(0), failure(502), failure(0), REACHED]);
  const kinds = [];
  const machine = makeReach({ ask, schedule: timers.schedule, onChange: (state) => kinds.push(state.kind) });
  await machine.start();
  assert.equal(machine.state().kind, 'cannot');
  for (let again = 0; again < 3; again += 1) {
    assert.deepEqual(timers.fire().filter((ms) => ms === 3000), [3000]);
    await settle();
  }
  assert.deepEqual(kinds, ['asking', 'cannot', 'cannot', 'cannot', 'reached']);
});

test('an ask that takes more than 5 seconds is a failed one, and an answer that comes after it is not heard', async () => {
  const timers = fakeTimers();
  let late;
  const { ask } = fakeAsk([() => new Promise((resolve) => { late = resolve; }), REACHED]);
  const machine = makeReach({ ask, schedule: timers.schedule });
  const started = machine.start();
  await settle();
  const limit = timers.waiting.find((timer) => timer.ms === 5000);
  assert.ok(limit, 'a timer of 5 seconds');
  limit.fn();
  await started;
  assert.equal(machine.state().kind, 'cannot');
  late(REACHED);
  await settle();
  assert.equal(machine.state().kind, 'cannot');
  timers.fire();
  await settle();
  assert.equal(machine.state().kind, 'reached');
});

test('nothing tried by that name ends the asking: gone', async () => {
  const timers = fakeTimers();
  const { ask } = fakeAsk([failure(404, 'ADM-135')]);
  const machine = makeReach({ ask, schedule: timers.schedule });
  await machine.start();
  assert.equal(machine.state().kind, 'gone');
  assert.deepEqual(timers.fire(), []);
});

test('the end of the window ends the asking: over', async () => {
  const timers = fakeTimers();
  let over = false;
  const { ask, calls } = fakeAsk([failure(0), failure(0), failure(0)]);
  const machine = makeReach({ ask, schedule: timers.schedule, over: () => over });
  await machine.start();
  over = true;
  timers.fire();
  await settle();
  assert.equal(machine.state().kind, 'over');
  assert.equal(calls.count, 2);
  assert.deepEqual(timers.fire(), []);
});

test('stop ends the asking at once, and an answer that is on its way is not heard', async () => {
  const timers = fakeTimers();
  let answer;
  const { ask } = fakeAsk([() => new Promise((resolve) => { answer = resolve; })]);
  const machine = makeReach({ ask, schedule: timers.schedule });
  const started = machine.start();
  await settle();
  machine.stop();
  answer(REACHED);
  await started;
  assert.equal(machine.state().kind, 'asking');
  assert.deepEqual(timers.fire(), []);
});

test('start again begins a new round, and the old rounds\' timers are cancelled', async () => {
  const timers = fakeTimers();
  const { ask, calls } = fakeAsk([failure(0), REACHED]);
  const machine = makeReach({ ask, schedule: timers.schedule });
  await machine.start();
  await machine.start();
  assert.equal(machine.state().kind, 'reached');
  assert.equal(calls.count, 2);
  assert.deepEqual(timers.fire(), []);
});

test('one round on its own, for the page that did not begin the trial, changes no state', async () => {
  const timers = fakeTimers();
  const { ask } = fakeAsk([OLD, REACHED, failure(0)]);
  const machine = makeReach({ ask, schedule: timers.schedule });
  assert.deepEqual(await machine.check(), { kind: 'reached', from: '192.168.1.20', via: REACHED.via });
  assert.deepEqual(await machine.check(), { kind: 'cannot' });
  assert.equal(machine.state().kind, 'idle');
});

test('a round that gets a 200 that is not fresh, or none of the words it knows, is a failed one', async () => {
  const timers = fakeTimers();
  assert.deepEqual(await reachRound(() => Promise.resolve({ fresh: false }), { schedule: timers.schedule }), { kind: 'cannot' });
  assert.deepEqual(await reachRound(() => Promise.resolve(null), { schedule: timers.schedule }), { kind: 'cannot' });
  assert.deepEqual(await reachRound(() => Promise.reject(failure(500, 'ADM-120')), { schedule: timers.schedule }), { kind: 'cannot' });
  assert.deepEqual(await reachRound(() => Promise.resolve({ fresh: true }), { schedule: timers.schedule }), { kind: 'reached', from: null, via: null });
});

test('the banner\'s sentence for each state, and why Keep is off', () => {
  assert.equal(reachWords({ kind: 'reached' }, '14:32').keep, null);
  assert.equal(reachWords({ kind: 'reached' }, '14:32').words, 'This browser reached the console over the new settings.');
  const cannot = reachWords({ kind: 'cannot' }, '14:32');
  assert.equal(cannot.words, 'This browser cannot reach the console over the new settings. It is undone at 14:32 unless it is kept.');
  assert.equal(cannot.keep, cannot.words);
  assert.match(reachWords({ kind: 'asking' }, '14:32').words, /^Checking that this browser still reaches the console over the new settings/);
  assert.match(reachWords({ kind: 'asking' }, '14:32').keep, /^Keep is on once/);
  assert.match(reachWords({ kind: 'unsure' }, '14:32').words, /^Cannot tell yet/);
  assert.match(reachWords({ kind: 'gone' }, '14:32').words, /^Nothing is being tried/);
  assert.match(reachWords({ kind: 'over' }, '14:32').keep, /over/);
});

// ---- the header's chip -----------------------------------------------------

// 14:28:00 UTC, and the person's clock in Prague, an hour ahead.
const NOW = Date.UTC(2026, 10, 14, 14, 28, 0);
const clock = makeTime('Europe/Prague', () => NOW).clock;

/** The `trial` event's list, as the chip holds it when it came at `NOW`. */
const held = (...trials) => heldTrials(trials, NOW);
const network = { id: 'network', left_ms: 252_000, page: '/networking', what: 'The network settings' };
const firewall = { id: 'firewall', left_ms: 65_000, page: '/firewall', what: 'The firewall\'s rules' };
const ssh = { id: 'ssh', left_ms: 300_000, page: '/security#ssh', what: 'SSH\'s settings' };
const seen = (list, route = '/', now = NOW) => chipWords(list, { route, now, clock });

test('a trial is held with the time it ends by this browser\'s clock, and an entry that is not a trial is left out', () => {
  assert.deepEqual(held(network), [{ page: '/networking', what: 'The network settings', endsMs: NOW + 252_000 }]);
  assert.deepEqual(heldTrials([{ id: 'x', left_ms: 'soon', page: '/x', what: 'X' }, { left_ms: 5, page: 7, what: 'X' }, null, { left_ms: 5, page: '/x' }], NOW), []);
  assert.deepEqual(heldTrials(undefined, NOW), []);
  assert.deepEqual(heldTrials([], NOW), []);
});

test('a trial that ended is shown for a minute more, until admind says so, and then it is gone', () => {
  const list = held(network);
  assert.equal(stillShown(list, NOW + 252_000).length, 1);
  assert.equal(stillShown(list, NOW + 252_000 + OVERDUE_MS - 1).length, 1);
  assert.equal(stillShown(list, NOW + 252_000 + OVERDUE_MS).length, 0);
});

test('one trial is a link to its page, in words, with the time left as a clock and the end in the accessible name', () => {
  assert.deepEqual(seen(held(network)), {
    words: 'Trying: the network settings',
    clock: '4:12',
    label: 'Trying: the network settings, undone at 3:32 PM unless kept',
    href: '/networking',
    items: [{ label: 'The network settings, undone at 3:32 PM', href: '/networking' }],
  });
  assert.equal(endsWords(NOW + 65_000, clock), 'undone at 3:29 PM');
});

test('the words after "Trying:" start with a small letter only where the first word is an ordinary one', () => {
  const words = (what) => seen(held({ left_ms: 1000, page: '/x', what })).words;
  assert.equal(words('The firewall\'s rules'), 'Trying: the firewall\'s rules');
  assert.equal(words('Who may reach the console'), 'Trying: who may reach the console');
  assert.equal(words('A change'), 'Trying: a change');
  assert.equal(words('SSH\'s settings'), 'Trying: SSH\'s settings');
  assert.equal(words('nginx\'s service file'), 'Trying: nginx\'s service file');
  assert.equal(words('PostgreSQL\'s service file'), 'Trying: PostgreSQL\'s service file');
});

test('two trials are one chip of "Trying 2 changes" with the time left of the soonest, and a list of both, the soonest first', () => {
  const both = seen(held(network, firewall));
  assert.equal(both.words, 'Trying 2 changes');
  assert.equal(both.clock, '1:05');
  assert.equal(both.href, null);
  assert.equal(both.label, 'Trying 2 changes, the soonest undone at 3:29 PM unless kept');
  assert.deepEqual(both.items, [
    { label: 'The firewall\'s rules, undone at 3:29 PM', href: '/firewall' },
    { label: 'The network settings, undone at 3:32 PM', href: '/networking' },
  ]);
  assert.equal(seen(held(network, firewall, ssh)).words, 'Trying 3 changes');
});

test('the page that owns a trial does not show it, with or without the place on the page, and the others are shown', () => {
  assert.equal(seen(held(network), '/networking'), null);
  assert.equal(seen(held(ssh), '/security'), null);
  assert.notEqual(seen(held(ssh), '/security/other'), null, 'a page under another is not the owner');
  const rest = seen(held(network, firewall), '/firewall');
  assert.equal(rest.words, 'Trying: the network settings');
  assert.equal(rest.href, '/networking');
  assert.equal(seen([], '/'), null);
});

test('time passing changes the digits and nothing else, and a trial that has run out reads 0:00', () => {
  const list = held(network);
  const first = seen(list, '/', NOW);
  for (const passed of [1000, 59_000, 252_000, 300_000]) {
    const later = seen(list, '/', NOW + passed);
    assert.deepEqual({ ...later, clock: '' }, { ...first, clock: '' });
  }
  assert.equal(seen(list, '/', NOW + 1000).clock, '4:11');
  assert.equal(seen(list, '/', NOW + 252_000).clock, '0:00');
  assert.equal(seen(list, '/', NOW + 300_000).clock, '0:00');
});
