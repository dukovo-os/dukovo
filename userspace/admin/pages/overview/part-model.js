// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Overview's own part (design 3.5) makes of `GET /api/1/tasks` and
// `GET /api/1/changes`: a task of the last 24 hours that failed or was
// stopped, and a file that someone changed by hand and the person has not
// marked as seen in the last 7 days.

import { titleOf } from '../../lib/tasks-model.js';
import { endWords } from '../../lib/tasks-words.js';

/** How far back a task that needs a look is still one, in seconds. */
export const TASK_SECONDS = 24 * 3600;

/** How far back a hand edit that was not marked seen is still one, in seconds. */
export const EDIT_SECONDS = 7 * 24 * 3600;

/** The most changes read for the week: the route's own limit. */
export const EDIT_LIMIT = 200;

/** The file name at the end of `path`. */
export const baseName = (path) => String(path).split('/').filter(Boolean).pop() ?? String(path);

/**
 * The needs for the tasks of `answer` (`{tasks: [...]}`) that failed or were
 * stopped in the 24 hours before `now`, in Unix seconds. A task has no page,
 * so it has no `href`: its words send a person to the Tasks drawer.
 */
export function taskNeeds(answer, now) {
  const tasks = Array.isArray(answer?.tasks) ? answer.tasks : [];
  return tasks
    .filter((task) => (task.state === 'failed' || task.state === 'stopped') && (task.ended ?? task.started) >= now - TASK_SECONDS)
    .map((task) => ({
      id: `task-${task.id}`,
      severity: task.state === 'failed' ? 'bad' : 'warn',
      title: `${titleOf(task)} ${task.state === 'failed' ? 'did not work' : 'was stopped'}`,
      detail: `${endWords(task)}. Open Tasks to see what it said.`,
      href: null,
      icon: 'tasks',
      at: task.ended ?? task.started,
    }));
}

/** "blog.conf", "blog.conf and 1 more file", "blog.conf and 3 more files": what a change touched. */
export function filesWords(change) {
  const files = Array.isArray(change.files) ? change.files : [];
  if (files.length === 0) return 'A file';
  const first = baseName(files[0].path);
  if (files.length === 1) return first;
  return `${first} and ${files.length - 1} more ${files.length === 2 ? 'file' : 'files'}`;
}

/** Whether `change` was made by hand and nobody has marked it seen. */
export const isUnseenEdit = (change) => change.how === 'by hand' && !change.seen;

/**
 * The needs for the changes of `answer` (`{changes: [...]}`) that were made
 * by hand in the 7 days before `now` and are not marked seen. Each is a
 * warning until someone chooses Keep it or Put back on the Overview.
 */
export function editNeeds(answer, now) {
  const changes = Array.isArray(answer?.changes) ? answer.changes : [];
  return changes.filter((change) => isUnseenEdit(change) && change.at >= now - EDIT_SECONDS).map((change) => ({
    id: `change-${change.id}`,
    severity: 'warn',
    title: `${filesWords(change)} changed outside this console`,
    detail: `${change.who === 'outside the console' ? 'Someone' : change.who} edited it by hand. Keep it, or put it back, under What changed.`,
    href: null,
    icon: 'info',
    at: change.at,
  }));
}
