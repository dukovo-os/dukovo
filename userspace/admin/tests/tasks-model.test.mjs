// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/tasks-model.js and lib/tasks-words.js: the tasks the console knows,
// the drawer's groups, the header's count and the words on a card.

import assert from 'node:assert/strict';
import test from 'node:test';

import { isRunning, lookIds, makeTasks, needsLook, noticeWords, pillOf, subjectOf, titleOf } from '../lib/tasks-model.js';
import { canStop, endWords, groupsOf, logDue, percentOf, startedBy, startedWords, stepWords } from '../lib/tasks-words.js';

const task = (more) => ({
  id: 1, action: 'service.restart', title: 'Restart a service', args: { name: 'nginx' }, who: 'alice', how: 'console',
  started: 1000, state: 'running', step: 0, steps: 0, words: '', percent: null, ended: null, status: null, note: null,
  changed: null, ...more,
});

test('a list replaces what was known, and a task put in adds to what is known of it', () => {
  const tasks = makeTasks();
  tasks.replace([task({ id: 1 }), task({ id: 2 })]);
  assert.deepEqual(tasks.all().map((each) => each.id), [1, 2]);
  tasks.replace([task({ id: 3 })]);
  assert.deepEqual(tasks.all().map((each) => each.id), [3]);
  tasks.put({ id: 3, state: 'done', ended: 2000 });
  assert.equal(tasks.get(3).title, 'Restart a service', 'what the list said stays');
  assert.equal(tasks.get(3).state, 'done');
  tasks.put(task({ id: 4 }));
  assert.equal(tasks.get(4).who, 'alice');
  assert.equal(tasks.get(99), undefined);
});

test('an event for a task that is not known says so, and one for a known task updates it without its title or arguments', () => {
  const tasks = makeTasks();
  tasks.replace([task({ id: 7, state: 'running', step: 1, steps: 4 })]);
  assert.deepEqual(tasks.event({ id: 8, state: 'running' }), { known: false });
  const moved = tasks.event({ id: 7, action: 'service.restart', state: 'running', step: 2, steps: 4, words: 'copying', percent: 40 });
  assert.equal(moved.known, true);
  assert.equal(moved.ended, false);
  assert.deepEqual([moved.task.step, moved.task.words, moved.task.title, moved.task.who], [2, 'copying', 'Restart a service', 'alice']);
});

test('a task that has ended does not run again when a summary written before its end comes after it', () => {
  const tasks = makeTasks();
  tasks.replace([task({ id: 6, step: 3, steps: 4 })]);
  tasks.event({ id: 6, state: 'stopped', ended: 1640, status: 0, note: 'stopped by bob' });
  // The answer to Stop says the task as it was when the stop was asked.
  tasks.put(task({ id: 6, step: 3, steps: 4, percent: 18 }));
  assert.deepEqual([tasks.get(6).state, tasks.get(6).note, tasks.get(6).ended], ['stopped', 'stopped by bob', 1640]);
  assert.equal(tasks.event({ id: 6, state: 'running' }).task.state, 'stopped');
  tasks.put({ id: 6, state: 'stopped', note: 'stopped by bob', words: 'Stopped' });
  assert.equal(tasks.get(6).words, 'Stopped', 'a later word of an ended task is taken');
});

test('the event that ends a task is told once', () => {
  const tasks = makeTasks();
  tasks.replace([task({ id: 7 }), task({ id: 8, state: 'waiting' })]);
  assert.equal(tasks.event({ id: 7, state: 'done', ended: 1500 }).ended, true);
  assert.equal(tasks.event({ id: 7, state: 'done', ended: 1500 }).ended, false, 'an event that repeats it');
  assert.equal(tasks.event({ id: 8, state: 'running' }).ended, false, 'waiting to running is no end');
  assert.equal(tasks.event({ id: 8, state: 'failed', status: 1, ended: 1600 }).ended, true);
});

test('the three groups hold what runs, what needs a look and what finished since midnight, newest first', () => {
  const day = 86400;
  const tasks = [
    task({ id: 1, state: 'running', started: 100 }),
    task({ id: 2, state: 'waiting', started: 300 }),
    task({ id: 3, state: 'failed', started: 50, ended: 400 }),
    task({ id: 4, state: 'stopped', started: 60, ended: 500 }),
    task({ id: 5, state: 'done', started: 10, ended: day + 10 }),
    task({ id: 6, state: 'done', started: 20, ended: day + 20 }),
    task({ id: 7, state: 'done', started: 5, ended: day - 5 }),
  ];
  const groups = groupsOf(tasks, day);
  assert.deepEqual(groups.running.map((each) => each.id), [2, 1]);
  assert.deepEqual(groups.needsLook.map((each) => each.id), [4, 3]);
  assert.deepEqual(groups.finishedToday.map((each) => each.id), [6, 5]);
  assert.deepEqual(groupsOf([], 0), { running: [], needsLook: [], finishedToday: [] });
});

test('a failed task that has no end time is placed by when it started', () => {
  const groups = groupsOf([task({ id: 1, state: 'failed', started: 10, ended: null }), task({ id: 2, state: 'failed', started: 20, ended: null })], 0);
  assert.deepEqual(groups.needsLook.map((each) => each.id), [2, 1]);
});

test('which states run and which need a look', () => {
  assert.deepEqual(['waiting', 'running', 'done', 'failed', 'stopped'].map((state) => isRunning({ state })), [true, true, false, false, false]);
  assert.deepEqual(['waiting', 'running', 'done', 'failed', 'stopped'].map((state) => needsLook({ state })), [false, false, false, true, true]);
});

test('the button says how many run, in words for a screen reader, and none when none do', () => {
  assert.deepEqual(pillOf([], []), { count: 0, warn: false, label: 'Tasks, none running' });
  const two = [task({ id: 1 }), task({ id: 2, state: 'waiting' }), task({ id: 3, state: 'done' })];
  assert.deepEqual(pillOf(two, []), { count: 2, warn: false, label: 'Tasks, 2 running' });
});

test('a task that needs a look takes the button until it is opened, and a seen one no longer does', () => {
  const failed = task({ id: 5, state: 'failed', ended: 20 });
  assert.deepEqual(pillOf([failed], []), { count: 1, warn: true, label: 'Tasks, 1 needs a look' });
  assert.deepEqual(pillOf([failed], [5]), { count: 0, warn: false, label: 'Tasks, none running' });
  const mixed = [failed, task({ id: 6 }), task({ id: 7, state: 'stopped', ended: 30 })];
  assert.deepEqual(pillOf(mixed, []), { count: 2, warn: true, label: 'Tasks, 2 need a look, 1 running' });
  assert.deepEqual(pillOf(mixed, [5, 7]), { count: 1, warn: false, label: 'Tasks, 1 running' });
  assert.deepEqual(lookIds(mixed), [5, 7]);
});

test('a person may stop a running task that is theirs, and an administrator any', () => {
  const mine = task({ who: 'alice' });
  assert.equal(canStop(mine, { name: 'alice', admin: false }), true);
  assert.equal(canStop(mine, { name: 'bob', admin: false }), false);
  assert.equal(canStop(mine, { name: 'bob', admin: true }), true);
  assert.equal(canStop(task({ state: 'waiting' }), { name: 'alice', admin: false }), true);
  assert.equal(canStop(task({ state: 'done' }), { name: 'alice', admin: true }), false);
});

test('a task\'s title is the action\'s, with what it is about after it', () => {
  assert.equal(titleOf(task({})), 'Restart a service · nginx');
  assert.equal(titleOf(task({ args: {} })), 'Restart a service');
  assert.equal(titleOf(task({ args: { name: 'blog.example.org', port: 443, extra: true, more: 'x' } })), 'Restart a service · blog.example.org, 443');
  assert.equal(titleOf(task({ title: undefined, args: undefined })), 'service.restart');
  assert.equal(subjectOf(task({ args: { empty: '', flag: false, list: ['a'] } })), '');
});

test('who started a task, and when, in words', () => {
  const clock = (seconds) => (seconds === 1000 ? '2:00 AM' : '?');
  assert.equal(startedBy(task({})), 'alice');
  assert.equal(startedBy(task({ how: 'token ci' })), 'alice with the API token ci');
  assert.equal(startedBy(task({ how: 'client desk of alice' })), 'alice, asked for by the AI client desk');
  assert.equal(startedBy(task({ how: 'client desk of chase' })), 'alice, asked for by chase\'s AI client desk');
  assert.equal(startedWords(task({ id: 42 }), clock), 'Started by alice at 2:00 AM · task 42');
});

test('where a running task is: its step and words, or what is known', () => {
  assert.equal(stepWords(task({ state: 'running', step: 2, steps: 4, words: 'copying the files' })), 'Step 2 of 4: copying the files');
  assert.equal(stepWords(task({ state: 'running', step: 1, steps: 4 })), 'Step 1 of 4');
  assert.equal(stepWords(task({ state: 'running', words: 'checking' })), 'checking');
  assert.equal(stepWords(task({ state: 'running' })), 'Running');
  assert.equal(stepWords(task({ state: 'waiting' })), 'Waiting for its turn');
});

test('how a task ended, in words', () => {
  assert.equal(endWords(task({ state: 'done', changed: true })), 'Finished');
  assert.equal(endWords(task({ state: 'done', changed: false })), 'Finished; nothing changed');
  assert.equal(endWords(task({ state: 'stopped', note: 'stopped by bob' })), 'Stopped: stopped by bob');
  assert.equal(endWords(task({ state: 'stopped' })), 'Stopped');
  assert.equal(endWords(task({ state: 'failed', note: 'the command did not start' })), 'Failed: the command did not start');
  assert.equal(endWords(task({ state: 'failed', status: 3 })), 'Failed: its command ended with status 3');
  assert.equal(endWords(task({ state: 'failed' })), 'Failed');
});

test('the notice for a task that ended names its number and its action', () => {
  assert.equal(noticeWords(task({ id: 42, state: 'done' })), 'Task 42, Restart a service, finished.');
  assert.equal(noticeWords(task({ id: 42, state: 'failed' })), 'Task 42, Restart a service, did not work.');
  assert.equal(noticeWords(task({ id: 42, state: 'stopped' })), 'Task 42, Restart a service, was stopped.');
});

test('an open log is read at most every two seconds', () => {
  assert.equal(logDue(null, 5000), true);
  assert.equal(logDue(4000, 5999), false);
  assert.equal(logDue(4000, 6000), true);
});

test('a progress bar has a value only when the command gives a percent', () => {
  assert.equal(percentOf(task({ percent: 60 })), 60);
  assert.equal(percentOf(task({ percent: 0 })), 0);
  assert.equal(percentOf(task({ percent: null })), null);
  assert.equal(percentOf(task({ percent: 140 })), 100);
  assert.equal(percentOf(task({ percent: 'a lot' })), null);
});
