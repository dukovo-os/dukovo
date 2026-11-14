// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Overview's own part (design 3.5), from fixtures of `GET /api/1/tasks`
// and `GET /api/1/changes`: a task of the last 24 hours that failed or was
// stopped, and a hand edit of the last 7 days that nobody marked seen. And
// the lab-only step that /setup shows.

import assert from 'node:assert/strict';
import test from 'node:test';

import { needs, steps } from '../pages/overview/overview.js';
import {
  baseName, editNeeds, EDIT_LIMIT, EDIT_SECONDS, filesWords, isUnseenEdit, taskNeeds, TASK_SECONDS,
} from '../pages/overview/part-model.js';
import { LAB_ACTION } from '../pages/overview/steps.js';
import {
  changes, fakeCx, NOW, tasks,
} from './overview-fixtures.mjs';

test('a task of the last 24 hours that failed or was stopped is a need, and one that runs, is done or is older is not', () => {
  assert.equal(TASK_SECONDS, 86400);
  const found = taskNeeds(tasks, NOW);
  assert.deepEqual(found.map((one) => [one.id, one.severity, one.title]), [
    ['task-41', 'bad', 'Renew a certificate · blog did not work'],
    ['task-40', 'warn', 'Restart a service · nginx was stopped'],
  ]);
});

test('a task\'s words are how it ended, and they send the person to the Tasks drawer, which no link reaches', () => {
  const [failed, stopped] = taskNeeds(tasks, NOW);
  assert.equal(failed.detail, 'Failed: the authority did not answer. Open Tasks to see what it said.');
  assert.equal(stopped.detail, 'Stopped. Open Tasks to see what it said.');
  assert.equal(failed.href, null);
  assert.equal(failed.at, NOW - 5400);
  assert.equal(failed.icon, 'tasks');
});

test('a task with no end is as old as its start, and its end is the end of its day', () => {
  const task = (ended, started) => ({ tasks: [{ id: 1, action: 'a', state: 'failed', ended, started, args: {} }] });
  assert.equal(taskNeeds(task(undefined, NOW - 100), NOW).length, 1);
  assert.equal(taskNeeds(task(undefined, NOW - 86401), NOW).length, 0);
  assert.equal(taskNeeds(task(NOW - 86400, 0), NOW).length, 1, 'exactly 24 hours is still the last 24 hours');
  assert.equal(taskNeeds(task(NOW - 86401, NOW), NOW).length, 0);
  assert.equal(taskNeeds({ tasks: [{ id: 2, action: 'service.stop', state: 'failed', started: NOW }] }, NOW)[0].title, 'service.stop did not work');
});

test('an answer that is not a list of tasks is no tasks', () => {
  for (const answer of [null, undefined, {}, { tasks: null }, []]) assert.deepEqual(taskNeeds(answer, NOW), []);
});

test('a hand edit of the last 7 days that nobody has seen is a need; one that was kept, made through the console or is older is not', () => {
  assert.equal(EDIT_SECONDS, 7 * 86400);
  const found = editNeeds(changes, NOW);
  assert.deepEqual(found.map((one) => [one.id, one.severity, one.title]), [
    ['change-12', 'warn', 'blog.conf changed outside this console'],
    ['change-10', 'warn', 'sshd_config and 1 more file changed outside this console'],
  ]);
});

test('a hand edit says who made it and what to do, and has no link, since its card is on the Overview', () => {
  const [mine, outside] = editNeeds(changes, NOW);
  assert.equal(mine.detail, 'alice edited it by hand. Keep it, or put it back, under What changed.');
  assert.equal(outside.detail, 'Someone edited it by hand. Keep it, or put it back, under What changed.');
  assert.equal(mine.href, null);
  assert.equal(mine.at, NOW - 9000);
});

test('the words for the files of a change', () => {
  const files = (...paths) => ({ files: paths.map((path) => ({ path })) });
  assert.equal(filesWords(files('/a/b/blog.conf')), 'blog.conf');
  assert.equal(filesWords(files('/a/one', '/a/two')), 'one and 1 more file');
  assert.equal(filesWords(files('/a/one', '/a/two', '/a/three')), 'one and 2 more files');
  assert.equal(filesWords({ files: [] }), 'A file');
  assert.equal(filesWords({}), 'A file');
  assert.equal(baseName('/a/b/'), 'b');
  assert.equal(baseName('plain'), 'plain');
  assert.equal(isUnseenEdit({ how: 'by hand', seen: null }), true);
  assert.equal(isUnseenEdit({ how: 'by hand', seen: { by: 'x', at: 1 } }), false);
  assert.equal(isUnseenEdit({ how: 'console', seen: null }), false);
  assert.equal(isUnseenEdit({ how: 'token ci', seen: null }), false);
});

test('an answer that is not a list of changes is no changes', () => {
  for (const answer of [null, undefined, {}, { changes: null }, []]) assert.deepEqual(editNeeds(answer, NOW), []);
});

test('the part reads the tasks and the week\'s changes, and gives the tasks first', async () => {
  const cx = fakeCx({ '/api/1/tasks': tasks, '/api/1/changes': changes });
  const realNow = Date.now;
  Date.now = () => NOW * 1000;
  try {
    const found = await needs(cx);
    assert.deepEqual(found.map((one) => one.id), ['task-41', 'task-40', 'change-12', 'change-10']);
  } finally {
    Date.now = realNow;
  }
  assert.deepEqual(cx.calls, ['/api/1/tasks', `/api/1/changes?since=${NOW - EDIT_SECONDS}&limit=${EDIT_LIMIT}`]);
  await assert.rejects(needs(fakeCx({ '/api/1/tasks': tasks })), /no answer/);
});

test('the lab step is there only where the lab\'s action is offered, and is done once it ran', async () => {
  const [step] = steps;
  assert.equal(steps.length, 1);
  assert.deepEqual([step.id, step.action, step.order, step.points], ['lab-long', LAB_ACTION, 1000, 0]);
  const withLab = fakeCx({ '/api/1/actions': [{ id: 'service.list' }, { id: 'lab.long' }], '/api/1/tasks': tasks });
  assert.equal(await step.available(withLab), true);
  assert.equal(await step.done(withLab), false);
  assert.equal(await step.summary(withLab), 'It has not run yet');
  const ran = fakeCx({ '/api/1/actions': { actions: [{ id: 'lab.long' }] }, '/api/1/tasks': { tasks: [{ action: 'lab.long', state: 'done' }] } });
  assert.equal(await step.available(ran), true);
  assert.equal(await step.done(ran), true);
  assert.equal(await step.summary(ran), 'It ran to its end');
  const shipped = fakeCx({ '/api/1/actions': [{ id: 'service.list' }] });
  assert.equal(await step.available(shipped), false);
  assert.equal(await step.available(fakeCx({ '/api/1/actions': {} })), false);
});
