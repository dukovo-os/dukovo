// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the buttons of the Apps pages run and how the tasks they begin are
// kept: the calls with their arguments, the words each says on the way, and
// the tasks that run on an app, from the list of tasks and from its events.

import { listOf } from './apps-model.js';

/** The calls the buttons make: the action and its arguments. */
export const updateAllCall = () => ({ action: 'apps.update-all', args: {} });
export const updateCall = (id, { yes = false } = {}) => ({ action: 'apps.update', args: yes ? { id, yes: true } : { id } });
export const rollbackCall = (id) => ({ action: 'apps.rollback', args: { id } });
export const holdCall = (id, held) => ({ action: held ? 'apps.unhold' : 'apps.hold', args: { id } });
export const removeCall = (id, data) => ({ action: 'apps.remove', args: { id, data } });

/** What the button that updates every app says on the way. */
export const UPDATE_ALL_WORDS = Object.freeze({
  unlock: 'update every app', running: 'Updating every app', done: 'Every app is updated.', busy: 'Starting…',
});

/** What each button says on the way: the unlock dialog's, the notice's. */
export function runWords(kind, name, version = null) {
  const words = {
    update: { unlock: `update ${name}`, running: `Updating ${name}${version ? ` to ${version}` : ''}`, done: `${name} is updated.`, busy: 'Starting…' },
    rollback: { unlock: `go back on ${name}`, running: `Going back on ${name}${version ? ` to ${version}` : ''}`, done: `${name} is back.`, busy: 'Starting…' },
    hold: { unlock: `hold ${name}`, done: `${name} is held at ${version}.`, busy: 'Holding…' },
    unhold: { unlock: `release ${name}`, done: `${name} is released.`, busy: 'Releasing…' },
    remove: { unlock: `remove ${name}`, running: `Removing ${name}`, done: `${name} is removed.`, busy: 'Starting…' },
  };
  return words[kind];
}

/** The actions that run as tasks on an app, and what each does; `apps.update-all` has no app of its own. */
const TASK_KINDS = Object.freeze({
  'apps.update': 'update', 'apps.update-all': 'update', 'apps.rollback': 'rollback', 'apps.remove': 'remove',
});

const running = (task) => task.state === 'waiting' || task.state === 'running';
const percentOf = (task) => (Number.isInteger(task.percent) ? Math.min(100, Math.max(0, task.percent)) : null);
const stepOf = (task) => (Number.isInteger(task.step) && Number.isInteger(task.steps) && task.steps > 1 ? { step: task.step, steps: task.steps } : null);

/** What the page keeps of a task: `{id, kind, app, percent, step}`; `app` is null for the task that updates every app. */
const jobOf = (task) => ({ id: task.id, kind: TASK_KINDS[task.action], app: task.args?.id ?? null, percent: percentOf(task), step: stepOf(task) });

/**
 * What the page knows of the tasks that run on apps. `learn(tasks)` takes
 * those of `GET /api/1/tasks`; `started(id, action, args)` one this page
 * began; `event(data)` one `event: task`, which holds no arguments, and gives
 * `{ended: job, state}` for one that ended, `{learn: id}` for one it does not
 * know yet, which is then read whole and given to `put`, else `{}`. `of(id,
 * updates)` is the task that runs on the app `id`; the one that updates every
 * app counts for an app that has an update (`updates` true).
 */
export function makeJobs() {
  const byId = new Map();
  const put = (task) => {
    if (TASK_KINDS[task.action] && running(task)) byId.set(task.id, jobOf(task));
  };
  return {
    learn(tasks) {
      byId.clear();
      for (const task of listOf(tasks)) put(task);
    },
    started(id, action, args) {
      byId.set(id, { id, kind: TASK_KINDS[action], app: args?.id ?? null, percent: null, step: null });
    },
    event(data) {
      if (data?.action !== undefined && !TASK_KINDS[data.action]) return {};
      const before = byId.get(data?.id);
      if (!before) return TASK_KINDS[data?.action] && running(data) ? { learn: data.id } : {};
      if (!running(data)) {
        byId.delete(data.id);
        return { ended: before, state: data.state };
      }
      byId.set(data.id, { ...before, percent: percentOf(data) ?? before.percent, step: stepOf(data) ?? before.step });
      return {};
    },
    put,
    any: () => byId.size > 0,
    of: (id, updates = false) => [...byId.values()].find((job) => job.app === id || (job.app === null && updates)) ?? null,
  };
}

/** The first words of a job. */
const JOB_WORDS = Object.freeze({ update: 'Updating…', rollback: 'Going back…', remove: 'Removing…' });

/** What a job says of how far it is: "Updating… 41%", "Updating… step 2 of 4". */
export function jobWords(job) {
  const first = JOB_WORDS[job.kind] ?? 'Working…';
  if (job.percent !== null) return `${first} ${job.percent}%`;
  return job.step ? `${first} step ${job.step.step} of ${job.step.steps}` : first;
}
