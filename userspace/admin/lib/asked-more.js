// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a task of `bundle update` leaves behind when the update asks for more
// than the version it replaces and nobody said yes: the sheet of what it asks
// for, and the question or the line that says it is not taken without a yes.
// Such a task ends with status 1, which the actions of an update answer for,
// so it is `done` and not `failed`. The Apps pages and the Updates page read
// it in the same way.

const lines = (task) => (Array.isArray(task?.log) ? task.log : []);

/** The line that `bundle update` asks its question with, or that says a request for tonight was not queued without a yes. */
const QUESTION = /Install it\?|--yes queues it anyway/;

/** Whether the task `task` (read whole, with its log) ended because the update asks for more than the version it replaces, and waits for a yes. */
export const askedMore = (task) => (task?.state === 'failed' || (task?.state === 'done' && task.status === 1)) && lines(task).some((line) => QUESTION.test(line));

/** The sheet that came before the question, the lines that say what the update asks for. */
export function sheetOf(task) {
  const all = lines(task);
  const asked = all.findIndex((line) => QUESTION.test(line));
  return (asked === -1 ? all : all.slice(0, asked)).join('\n').trim();
}
