// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The steps of a flow (lib/steps.js) on a fake `api.js`: in order, a task
// step that waits for its task, the first failure that stops the rest and is
// drawn with its error link, and what is never drawn.

import assert from 'node:assert/strict';
import test from 'node:test';

import { ApiError } from '../api.js';
import { LOST_ANSWER } from '../lib/action-model.js';
import { errorView } from '../lib/problem.js';
import { runSteps } from '../lib/steps.js';
import { usePage } from './fake-dom.mjs';

/** Lets what is waiting on a promise run, as a click does. */
const settle = async () => {
  for (let at = 0; at < 20; at += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** A promise that the test settles by hand, as a call that has not answered yet. */
function later() {
  const hold = {};
  hold.promise = new Promise((resolve) => { hold.resolve = resolve; });
  return hold;
}

/** The fields of the shell's `cx` that the error view reads. */
const cx = { path: '/databases/add', time: { full: () => 'now' }, hasPage: () => false };

/**
 * A view whose actions answer as `runs` says (an action id to what its call
 * gives, or a function of the arguments), whose task reads answer from
 * `tasks` (an id to a list: each read takes the next, the last one stays),
 * and which keeps what a page would need to look at.
 */
function openView(context, { runs = {}, tasks = {} } = {}) {
  const page = usePage(context);
  const controller = new AbortController();
  context.after(() => controller.abort());
  const listeners = [];
  const timers = [];
  const sent = [];
  const reads = [];
  const view = {
    signal: controller.signal,
    on: (event, listener) => {
      const entry = [event, listener];
      listeners.push(entry);
      return () => listeners.splice(listeners.indexOf(entry), 1);
    },
    every: (ms, fn) => {
      const entry = [ms, fn];
      timers.push(entry);
      return () => timers.splice(timers.indexOf(entry), 1);
    },
    problem: (error, where = {}) => errorView(error, where, cx, where.slot),
    openTask: (id) => view.opened.push(id),
    opened: [],
    api: {
      run: async (action, args, options = {}) => {
        sent.push([action, args, options]);
        const way = runs[action];
        const answer = typeof way === 'function' ? await way(args, options) : way;
        return answer ?? { ok: true, result: { done: true, changed: true, output: [`${action} is done`] } };
      },
      get: async (path) => {
        reads.push(path);
        const list = tasks[Number(path.split('/').pop())];
        if (!list) throw new Error('no such task');
        return list.length > 1 ? list.shift() : list[0];
      },
    },
  };
  const emit = async (event, data) => {
    listeners.filter(([name]) => name === event).forEach(([, listener]) => listener(data));
    await settle();
  };
  const start = (steps) => {
    const run = runSteps(view, steps);
    page.document.body.append(run.element);
    return run;
  };
  return {
    ...page, view, controller, listeners, timers, sent, reads, emit, start,
  };
}

const step = (title, action, more = {}) => ({
  title, action, args: {}, wait: 'answer', words: {}, ...more,
});

/** The words of each item's state, in order. */
const statesOf = (list) => list.querySelectorAll('li').map((item) => item.querySelector('.flow-state').textContent);

test('steps run one after the other, each saying its state in words and its last line', async (context) => {
  const first = later();
  const world = openView(context, {
    runs: {
      'a.first': () => first.promise,
      'a.second': { ok: true, result: { done: true, output: ['Making it', 'The second is made'] } },
    },
  });
  const run = world.start([step('First', 'a.first'), step('Second', 'a.second'), step('Third', 'a.third')]);
  await settle();
  assert.deepEqual(statesOf(run.element), ['Running', 'Waiting', 'Waiting']);
  assert.deepEqual(world.sent.map(([action]) => action), ['a.first']);

  first.resolve({ ok: true, result: { done: true, output: ['The first is made', ''] } });
  const outcome = await run.finished;
  assert.deepEqual(statesOf(run.element), ['Done', 'Done', 'Done']);
  assert.deepEqual(world.sent.map(([action]) => action), ['a.first', 'a.second', 'a.third']);
  const lines = run.element.querySelectorAll('.flow-line').map((line) => line.textContent);
  assert.deepEqual(lines, ['The first is made', 'The second is made', 'a.third is done']);
  assert.equal(outcome.done, true);
  assert.equal(outcome.failed, null);
  assert.equal(outcome.left, false);
  assert.equal(outcome.answers.length, 3);
  assert.deepEqual(outcome.answers[1].output, ['Making it', 'The second is made']);
});

test('a step is sent with its arguments and words, and its problem is drawn in its own row', async (context) => {
  const world = openView(context);
  const words = { unlock: 'Unlock to make the database', done: 'Made' };
  const run = world.start([step('Make it', 'databases.add', { args: { name: 'shop' }, words })]);
  await run.finished;
  const [action, args, options] = world.sent[0];
  assert.equal(action, 'databases.add');
  assert.deepEqual(args, { name: 'shop' });
  assert.equal(options.words, words);
  assert.equal(options.where.slot, run.element.querySelector('.flow-problem'));
});

test('each item is in a list whose line is a polite status and says its state with a mark and words', async (context) => {
  const world = openView(context);
  const run = world.start([step('Make it', 'a.first'), step('Use it', 'a.second')]);
  assert.equal(run.element.tag, 'ol');
  assert.ok(run.element.classes.has('chk-list'));
  const [one, two] = run.element.querySelectorAll('li');
  assert.equal(one.getAttribute('data-state'), 'running');
  assert.equal(two.getAttribute('data-state'), 'waiting');
  assert.equal(one.querySelector('.flow-line').getAttribute('role'), 'status');
  assert.ok(one.querySelector('.tick').classes.has('run'));
  await run.finished;
  assert.ok(one.querySelector('.tick').classes.has('done'));
  assert.equal(one.querySelector('.tick .ic-check') !== null, true);
});

test('a task step is done when its task ends, and the next step waits for that', async (context) => {
  const world = openView(context, {
    runs: { 'a.build': { ok: true, task: 7, result: { task: 7 } } },
    tasks: { 7: [{ id: 7, state: 'running', step: 1, steps: 3, words: 'copying' }] },
  });
  const run = world.start([step('Build', 'a.build', { wait: 'task' }), step('Use', 'a.use')]);
  await settle();
  assert.deepEqual(statesOf(run.element), ['Running', 'Waiting']);
  assert.equal(run.element.querySelector('.flow-line').textContent, 'Step 1 of 3: copying');
  assert.deepEqual(world.sent.map(([action]) => action), ['a.build']);

  // Another task's end is not this one's.
  await world.emit('task', { id: 9, state: 'done' });
  assert.deepEqual(statesOf(run.element), ['Running', 'Waiting']);

  await world.emit('task', { id: 7, state: 'running', step: 2, steps: 3, words: 'checking' });
  assert.equal(run.element.querySelector('.flow-line').textContent, 'Step 2 of 3: checking');
  assert.deepEqual(world.sent.map(([action]) => action), ['a.build']);

  world.reads.length = 0;
  world.timers.length = 0;
  // The end is read whole, since an event holds no log.
  const ended = { id: 7, state: 'done', log: ['Copying', 'The files are in place'] };
  world.view.api.get = async (path) => {
    world.reads.push(path);
    return ended;
  };
  await world.emit('task', { id: 7, state: 'done' });
  const outcome = await run.finished;
  assert.deepEqual(world.reads.slice(0, 1), ['/api/1/tasks/7']);
  assert.deepEqual(statesOf(run.element), ['Done', 'Done']);
  assert.equal(run.element.querySelector('.flow-line').textContent, 'The files are in place');
  assert.deepEqual(world.sent.map(([action]) => action), ['a.build', 'a.use']);
  assert.equal(outcome.answers[0], ended);
  assert.equal(world.listeners.length, 0, 'the task listener is taken off');
  assert.equal(world.timers.length, 0, 'the timer is stopped');
});

test('a task that ended before it was heard is read at once', async (context) => {
  const world = openView(context, {
    runs: { 'a.build': { ok: true, task: 3, result: { task: 3 } } },
    tasks: { 3: [{ id: 3, state: 'done', log: ['Built'] }] },
  });
  const run = world.start([step('Build', 'a.build', { wait: 'task' })]);
  const outcome = await run.finished;
  assert.equal(outcome.done, true);
  assert.deepEqual(statesOf(run.element), ['Done']);
});

test('a task whose end was missed is found by the timer', async (context) => {
  const world = openView(context, {
    runs: { 'a.build': { ok: true, task: 3, result: { task: 3 } } },
    tasks: { 3: [{ id: 3, state: 'running' }, { id: 3, state: 'done', log: ['Built'] }] },
  });
  const run = world.start([step('Build', 'a.build', { wait: 'task' })]);
  await settle();
  assert.deepEqual(statesOf(run.element), ['Running']);
  assert.equal(run.element.querySelector('.flow-line').textContent, 'Running');
  assert.equal(world.timers.length, 1);
  world.timers[0][1]();
  const outcome = await run.finished;
  assert.equal(outcome.done, true);
});

test('a task that fails stops the rest, and its error has the page of its ID', async (context) => {
  const world = openView(context, {
    runs: { 'a.build': { ok: true, task: 7, result: { task: 7 } } },
    tasks: { 7: [{ id: 7, state: 'failed', status: 1, log: ['Making it', 'createdb: shop exists (DB-103)'] }] },
  });
  const words = { failed: 'The database was not made', stands: 'The account shop was made. The database was not.' };
  const run = world.start([
    step('Account', 'a.account'),
    step('Build', 'a.build', { wait: 'task', words }),
    step('Use', 'a.use'),
    step('Check', 'a.check'),
  ]);
  const outcome = await run.finished;
  assert.deepEqual(statesOf(run.element), ['Done', 'Failed', 'Not run', 'Not run']);
  assert.deepEqual(world.sent.map(([action]) => action), ['a.account', 'a.build']);
  assert.equal(outcome.done, false);
  assert.equal(outcome.failed, 1);
  const [, failed] = run.element.querySelectorAll('li');
  const shown = failed.querySelector('.flow-problem');
  assert.match(shown.textContent, /The database was not made/);
  assert.match(shown.textContent, /createdb: shop exists/);
  assert.doesNotMatch(shown.querySelector('dl').textContent, /\(DB-103\)/);
  const link = shown.querySelectorAll('a').find((one) => one.getAttribute('href') === '/help/errors/DB-103');
  assert.ok(link, 'the link to the error page');
  const stands = failed.querySelector('.flow-stands');
  assert.equal(stands.hidden, false);
  assert.equal(stands.textContent, words.stands);
  assert.equal(world.document.activeElement, shown, 'focus is on the message');
  // The rows of steps that were not run say nothing of what stands.
  assert.equal(run.element.querySelectorAll('li')[2].querySelector('.flow-stands').hidden, true);
});

test('the failed task can be opened from its error', async (context) => {
  const world = openView(context, {
    runs: { 'a.build': { ok: true, task: 7, result: { task: 7 } } },
    tasks: { 7: [{ id: 7, state: 'failed', note: 'out of room', log: [] }] },
  });
  const run = world.start([step('Build', 'a.build', { wait: 'task' })]);
  await run.finished;
  const shown = run.element.querySelector('.flow-problem');
  assert.match(shown.textContent, /Failed: out of room/);
  const button = shown.querySelectorAll('button').find((one) => one.textContent === 'Open the task');
  button.click();
  assert.deepEqual(world.view.opened, [7]);
});

test('a refusal stops the rest, and what the runner drew stays', async (context) => {
  const world = openView(context, {
    runs: {
      'a.one': (args, options) => {
        world.view.problem(new ApiError({ id: 'DB-101', title: 'The name is taken', status: 409 }), options.where);
        return { ok: false };
      },
    },
  });
  const run = world.start([step('One', 'a.one'), step('Two', 'a.two')]);
  const outcome = await run.finished;
  assert.deepEqual(statesOf(run.element), ['Failed', 'Not run']);
  assert.deepEqual(world.sent.map(([action]) => action), ['a.one']);
  assert.equal(outcome.failed, 0);
  const shown = run.element.querySelector('.flow-problem');
  assert.match(shown.textContent, /The name is taken/);
  assert.ok(shown.querySelectorAll('a').some((one) => one.getAttribute('href') === '/help/errors/DB-101'));
});

test('a call that got no answer says so, and one that was not started says nothing was changed', async (context) => {
  const lost = openView(context, { runs: { 'a.one': { ok: false, lost: true } } });
  const first = lost.start([step('One', 'a.one')]);
  await first.finished;
  assert.equal(first.element.querySelector('.flow-problem').textContent, LOST_ANSWER);
  assert.deepEqual(statesOf(first.element), ['Failed']);

  const cancelled = openView(context, { runs: { 'a.one': { ok: false, cancelled: true } } });
  const second = cancelled.start([step('One', 'a.one'), step('Two', 'a.two')]);
  await second.finished;
  assert.match(second.element.querySelector('.flow-problem').textContent, /nothing was changed/);
  assert.deepEqual(statesOf(second.element), ['Failed', 'Not run']);
});

test('an answer that is not the one the step needs fails the step', async (context) => {
  const world = openView(context, { runs: { 'a.one': { ok: true, result: { done: true, output: ['{}'] } } } });
  const run = world.start([step('One', 'a.one', { ok: (answer) => answer.output[0] === '{"ready":true}' }), step('Two', 'a.two')]);
  const outcome = await run.finished;
  assert.equal(outcome.failed, 0);
  assert.deepEqual(statesOf(run.element), ['Failed', 'Not run']);
  assert.match(run.element.querySelector('.flow-problem').textContent, /One did not give the answer it should/);
});

test('a secret step keeps its answer to itself: it is given back, and never drawn', async (context) => {
  const answer = { done: true, changed: true, output: ['{"password":"hunter2-hunter2"}'] };
  const world = openView(context, { runs: { 'a.new': { ok: true, result: answer } } });
  const run = world.start([step('New password', 'a.new', { secret: true })]);
  const outcome = await run.finished;
  assert.equal(outcome.answers[0], answer);
  assert.equal(run.element.textContent.includes('hunter2'), false);
  assert.equal(run.element.querySelector('.flow-line').textContent, '');
});

test('leaving the page ends the wait and runs nothing more', async (context) => {
  const world = openView(context, {
    runs: { 'a.build': { ok: true, task: 7, result: { task: 7 } } },
    tasks: { 7: [{ id: 7, state: 'running' }] },
  });
  const run = world.start([step('Build', 'a.build', { wait: 'task' }), step('Use', 'a.use')]);
  await settle();
  world.controller.abort();
  const outcome = await run.finished;
  assert.equal(outcome.left, true);
  assert.equal(outcome.done, false);
  assert.equal(outcome.failed, null);
  assert.deepEqual(world.sent.map(([action]) => action), ['a.build']);
  assert.equal(world.listeners.length, 0);
});

test('a task step that was answered at once is done with that answer', async (context) => {
  const world = openView(context, { runs: { 'a.build': { ok: true, result: { done: true, output: ['Quick'] } } } });
  const run = world.start([step('Build', 'a.build', { wait: 'task' })]);
  const outcome = await run.finished;
  assert.equal(outcome.done, true);
  assert.equal(run.element.querySelector('.flow-line').textContent, 'Quick');
  assert.deepEqual(world.reads, []);
});
