// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Tasks drawer's groups and the words on a task's card (design 8), as
// plain values. tasks/cards.js draws them; the model is lib/tasks-model.js.

import { askedBy } from './how.js';
import { isRunning, needsLook } from './tasks-model.js';

/** The least time between two reads of a log that is open, in milliseconds. */
export const LOG_EVERY_MS = 2000;

/**
 * The drawer's three groups of `tasks`, each newest first: those that wait
 * or run; those that failed or were stopped; and those that finished well
 * since `startOfDay`, a Unix time, which is midnight in the person's zone.
 */
export function groupsOf(tasks, startOfDay) {
  const newest = (key) => (one, other) => (other[key] ?? other.started) - (one[key] ?? one.started);
  return {
    running: tasks.filter(isRunning).sort((one, other) => other.started - one.started),
    needsLook: tasks.filter(needsLook).sort(newest('ended')),
    finishedToday: tasks
      .filter((task) => task.state === 'done' && (task.ended ?? task.started) >= startOfDay)
      .sort(newest('ended')),
  };
}

/** Whether `session`'s person may stop `task`: it runs, and it is theirs or they are an administrator. */
export function canStop(task, session) {
  return isRunning(task) && (task.who === session.name || session.admin);
}

/** Who started `task`, in words: a person in the console, with the client that asked for it if one did, or through an API token. */
export function startedBy(task) {
  const token = /^token (.+)$/.exec(task.how ?? '');
  return token ? `${task.who} with the API token ${token[1]}` : `${task.who}${askedBy(task.how, task.who)}`;
}

/** "Started by alice at 2:00 AM · task 42"; `clock` writes a Unix time as the person reads it. */
export function startedWords(task, clock) {
  return `Started by ${startedBy(task)} at ${clock(task.started)} · task ${task.id}`;
}

/** "Step 2 of 4: copying the files", or what is known of where a running task is. */
export function stepWords(task) {
  if (task.state === 'waiting') return 'Waiting for its turn';
  if (task.steps > 0) return `Step ${task.step} of ${task.steps}${task.words ? `: ${task.words}` : ''}`;
  return task.words || 'Running';
}

/** How an ended task ended, in words: its note, or what its state and status say. */
export function endWords(task) {
  if (task.state === 'done') return task.changed === false ? 'Finished; nothing changed' : 'Finished';
  if (task.state === 'stopped') return task.note ? `Stopped: ${task.note}` : 'Stopped';
  if (task.note) return `Failed: ${task.note}`;
  return task.status === null || task.status === undefined ? 'Failed' : `Failed: its command ended with status ${task.status}`;
}

/** Whether a log that was read at `lastAt` may be read again at `now`, both in milliseconds. */
export function logDue(lastAt, now) {
  return lastAt === null || now - lastAt >= LOG_EVERY_MS;
}

/** The value for a progress bar of `task`: its percent, or null when the command gives none. */
export function percentOf(task) {
  return Number.isInteger(task.percent) ? Math.min(100, Math.max(0, task.percent)) : null;
}
