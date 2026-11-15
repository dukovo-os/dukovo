// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/action-model.js: the request an action makes, who may run it, and the
// words of each step.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  changeDone, findAction, gotNoAnswer, LOST_ANSWER, lowerFirst, needsAdministrator, readerProblem, requestFor,
  taskStarted, wordsFor,
} from '../lib/action-model.js';

const restart = { id: 'service.restart', title: 'Restart a service', method: 'POST', path: '/api/1/services/{name}/restart', runs: 'as-administrator' };
const list = { id: 'service.list', title: 'List the services', method: 'GET', path: '/api/1/services', runs: 'as-the-person' };

test('an action is found by its id', () => {
  assert.equal(findAction([list, restart], 'service.restart'), restart);
  assert.equal(findAction([list], 'nothing'), undefined);
});

test('the path takes its arguments, escaped, and the rest go in the body', () => {
  assert.deepEqual(requestFor(restart, { name: 'my service/1' }), {
    method: 'POST', path: '/api/1/services/my%20service%2F1/restart', body: {},
  });
  assert.deepEqual(requestFor({ ...restart, path: '/api/1/things/{id}' }, { id: 7, force: true }), {
    method: 'POST', path: '/api/1/things/7', body: { force: true },
  });
});

test('a read puts the other arguments in the query', () => {
  assert.deepEqual(requestFor(list), { method: 'GET', path: '/api/1/services', body: undefined });
  assert.equal(requestFor(list, { state: 'failed', n: 2 }).path, '/api/1/services?state=failed&n=2');
});

test('an action that runs as an administrator needs one', () => {
  assert.equal(needsAdministrator(restart), true);
  assert.equal(needsAdministrator(list), false);
});

test('a reader is told in words, with the ID that places the error in a banner', () => {
  const problem = readerProblem({ name: 'alice' });
  assert.equal(problem.id, 'ADM-122');
  assert.match(problem.title, /alice's account can look but not change/);
  assert.match(problem.fix, /administrator/);
});

test('the words come from the page, over the defaults of the action\'s title', () => {
  assert.deepEqual(wordsFor(restart), {
    unlock: 'restart a service', running: 'Restart a service', done: 'Restart a service: done.',
    unchanged: 'Restart a service: nothing changed.', busy: 'Restart a service…',
  });
  const own = wordsFor(restart, { unlock: 'restart nginx', running: 'Restarting nginx', done: 'nginx restarted.', unchanged: 'nginx was already running: nothing changed.', busy: 'Restarting…' });
  assert.equal(own.unlock, 'restart nginx');
  assert.equal(taskStarted(own, 42), 'Restarting nginx: running as task 42');
  assert.equal(changeDone(own, { done: true, changed: true }), 'nginx restarted.');
  assert.equal(changeDone(own, { done: true, changed: false }), 'nginx was already running: nothing changed.');
});

test('a lost answer says to look in What changed and Tasks, and what has no HTTP answer is told apart', () => {
  assert.match(LOST_ANSWER, /What changed and Tasks/);
  assert.equal(gotNoAnswer({ status: 0, id: null }), true);
  assert.equal(gotNoAnswer({ status: 500, id: 'ADM-123' }), false);
  assert.equal(lowerFirst('Restart'), 'restart');
  assert.equal(lowerFirst(''), '');
});
